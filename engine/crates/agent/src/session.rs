//! Log segments and the call loop.
//!
//! A segment is opened with its three prefix blocks and then only appended to. Before a
//! flow appends, [`Agent::ensure_segment`] decides whether the current segment can take
//! more: a new user name or a new wire API, too much accumulated material, or a context
//! near the endpoint's window each roll it over (wrap-up on the old segment first).
//!
//! A different model, reasoning value or tool description does not roll: the session
//! keeps appending and only the call after the switch misses the cache.

use std::collections::HashSet;

use api::endpoints::WireApi;
use api::events::{MessageAdded, MessageDelta, QuotaChanged, ToolCalled};
use api::views::ToolAction;
use futures::StreamExt;
use world::log::{self, Role};
use world::quota;
use world::wrapup::Summary;
use world::{
    Actor, ChannelId, ChannelKind, MessageId, SegmentId, Shape, TaskId, bond, channel, message,
};

use crate::agent::{Agent, Event};
use crate::assemble::{self, Ctx, Member};
use crate::endpoint::ChatRole;
use crate::error::{Error, Result};
use crate::persona;
use crate::store::{self, MessageMeta};
use crate::stream::{Decoder, Piece, Reply};
use crate::text::Names;
use crate::wire::{self, Choice};

/// Whose session a channel's log is: which prefix and which tool list it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogKind {
    Person,
    Host,
}

/// One flow's model-facing state on one channel.
pub(crate) struct Turn {
    pub channel: ChannelId,
    pub speaker: Actor,
    pub shape: Shape,
    /// The request this turn spends, if any.
    pub task: Option<TaskId>,
    /// Host only: the user wrote this turn, so the actions that need the user's say-so
    /// may run.
    pub authorized: bool,
    /// The harness's wrap-up turn: the only place `wrapup` may be called.
    pub wrapup: bool,
    pub message: Option<MessageId>,
    pub text: String,
    pub actions: Vec<ToolAction>,
    pub level: Option<index::Level>,
    /// Ids shown to the model during this turn, what a reply may cite.
    pub shown: Vec<String>,
    /// A colleague pulled in, who answers after this turn.
    pub joined: Option<world::task::Joined>,
    /// Requests the host made, run after its turn.
    pub asks: Vec<world::task::Asked>,
    /// The turn's request was answered (`report`) or closed.
    pub closed: bool,
    /// Parsed `wrapup` arguments, in a wrap-up turn.
    pub wrapped: Option<world::wrapup::Args>,
}

impl Turn {
    pub fn new(channel: ChannelId, speaker: Actor, shape: Shape) -> Self {
        Self {
            channel,
            speaker,
            shape,
            task: None,
            authorized: false,
            wrapup: false,
            message: None,
            text: String::new(),
            actions: Vec::new(),
            level: None,
            shown: Vec::new(),
            joined: None,
            asks: Vec::new(),
            closed: false,
            wrapped: None,
        }
    }
}

fn cache_key(segment: SegmentId) -> String {
    format!("dramatis-{segment}")
}

/// The part of a body the next body on the same log starts with: everything before the
/// closing bracket of the item list. The tail never contains one.
pub fn open_prefix(body: &[u8]) -> &[u8] {
    match body.iter().rposition(|b| *b == b']') {
        Some(i) => &body[..i],
        None => body,
    }
}

impl Agent {
    pub(crate) fn log_kind(&self, conn: &rusqlite::Connection, ch: ChannelId) -> Result<LogKind> {
        Ok(if channel::get(conn, ch)?.is_host_channel() {
            LogKind::Host
        } else {
            LogKind::Person
        })
    }

    /// The request shape for a log kind under the current chat role.
    pub(crate) fn shape_for(&self, role: &ChatRole, kind: LogKind, names: &Names) -> wire::Shape {
        let tools = match kind {
            LogKind::Person => api::tools::shared_tools(&self.corpus.descriptions),
            LogKind::Host => api::tools::host_tools(&self.corpus.descriptions),
        };
        let reasoning = match kind {
            LogKind::Person => role
                .reasoning
                .clone()
                .or_else(|| self.config.agent.reasoning.chat.clone()),
            LogKind::Host => self
                .config
                .agent
                .reasoning
                .host
                .clone()
                .or_else(|| role.reasoning.clone()),
        };
        wire::Shape {
            wire: role.profile.wire_api,
            model: role.model.id.clone(),
            reasoning,
            tools: api::tools::to_wire(&tools, role.profile.wire_api).to_string(),
            user: names.user().to_owned(),
        }
    }

    pub(crate) fn ctx<'a>(&'a self, names: &'a Names) -> Ctx<'a> {
        Ctx {
            wording: &self.corpus.wording,
            names,
            scene: (&self.corpus.scene.0, &self.corpus.scene.1),
        }
    }

    /// The prefix blocks of a new segment of `ch`.
    fn prefix(
        &self,
        conn: &rusqlite::Connection,
        ch: ChannelId,
        names: &Names,
        summary: Option<&Summary>,
        tail: Option<Tail>,
    ) -> Result<[String; 3]> {
        let ctx = self.ctx(names);
        let c = channel::get(conn, ch)?;
        // Before the corpus lock below: `recent` takes it too.
        let recent = match tail {
            Some(t) => self.recent(conn, ch, t.before)?,
            None => Vec::new(),
        };
        if c.is_host_channel() {
            let a = assemble::block_a_host(&ctx, &self.corpus.host_name, &self.corpus.host);
            let c_block = assemble::block_c(
                &ctx,
                &assemble::Opening {
                    tones: Vec::new(),
                    topic: None,
                    memories: &[],
                    memory_owner: None,
                    summary: summary.map(|s| s.text.as_str()),
                    recent: &recent,
                },
            );
            return Ok([a, String::new(), c_block]);
        }
        let a = assemble::block_a(&ctx);
        let folio_guard = self.ix();
        let folio = folio_guard.folio();
        // Owner first, then everyone else in id order.
        let mut people = c.persons();
        if let Some(owner) = c.person() {
            people.retain(|p| p != owner);
            people.insert(0, owner.clone());
        }
        people.retain(|p| self.corpus.personas.has_persona(p.as_str()));
        let mut names_of = Vec::new();
        let mut personas = Vec::new();
        for p in &people {
            let display = folio
                .person(p.as_str())?
                .map(|x| x.display)
                .unwrap_or_else(|| p.0.clone());
            let persona =
                persona::person(folio, p.as_str())?.ok_or_else(|| Error::NoPersona(p.clone()))?;
            names_of.push(display);
            personas.push(persona);
        }
        let mut members: Vec<Member<'_>> = names_of
            .iter()
            .zip(&personas)
            .map(|(name, persona)| Member::Person { name, persona })
            .collect();
        if c.has(&Actor::Host) {
            members.push(Member::Host {
                name: &self.corpus.host_name,
                in_world: &self.corpus.host.in_world,
            });
        }
        let b = assemble::block_b(&ctx, &members);

        let tones: Vec<(&str, world::trust::Tone)> = people
            .iter()
            .zip(&names_of)
            .map(|(p, name)| {
                let trust = bond::user_bond(conn, p).map(|b| b.trust).unwrap_or(100);
                (
                    name.as_str(),
                    world::trust::tone(trust, &self.config.world.trust),
                )
            })
            .collect::<Vec<_>>();
        let reader = people.first();
        let mut memories = match reader {
            Some(p) => log::opening_memories(conn, ch, Some(p))?,
            None => Vec::new(),
        };
        let keep = self.config.agent.opening.memories as usize;
        if memories.len() > keep {
            memories.drain(..memories.len() - keep);
        }
        let owner = (people.len() == 1).then(|| names_of[0].as_str());
        let c_block = assemble::block_c(
            &ctx,
            &assemble::Opening {
                tones,
                topic: if c.kind == ChannelKind::Group {
                    c.topic.as_deref()
                } else {
                    None
                },
                memories: &memories,
                memory_owner: owner,
                summary: summary.map(|s| s.text.as_str()),
                recent: &recent,
            },
        );
        Ok([a, b, c_block])
    }

    /// The last `roll.tail` messages with text before `before`, as `(author, text)`.
    fn recent(
        &self,
        conn: &rusqlite::Connection,
        ch: ChannelId,
        before: Option<MessageId>,
    ) -> Result<Vec<(String, String)>> {
        let n = self.config.agent.roll.tail;
        let mut out = Vec::new();
        let msgs = message::history(conn, ch, before, n.saturating_mul(4).max(n))?;
        for m in msgs.into_iter().rev().filter(|m| !m.text.trim().is_empty()) {
            if out.len() as u32 >= n {
                break;
            }
            let who = match &m.author {
                Actor::User => self.names_now(conn)?.user().to_owned(),
                Actor::Host => self.corpus.host_name.clone(),
                Actor::Person(p) => self
                    .ix()
                    .folio()
                    .person(p.as_str())?
                    .map(|x| x.display)
                    .unwrap_or_else(|| p.0.clone()),
            };
            out.push((who, m.text));
        }
        out.reverse();
        Ok(out)
    }

    pub(crate) fn names_now(&self, conn: &rusqlite::Connection) -> Result<Names> {
        let name = world::settings::user_name(conn)?;
        Ok(Names::new(
            &self.corpus.placeholder,
            name.as_deref(),
            &self.corpus.wording,
        ))
    }

    /// Open the next segment of a channel. A rollover passes the old segment's summary
    /// and a [`Tail`]: the last messages go into the opening block.
    pub(crate) fn open_segment(
        &self,
        ch: ChannelId,
        summary: Option<&Summary>,
        tail: Option<Tail>,
    ) -> Result<SegmentId> {
        let (role, _) = self.chat()?;
        let conn = self.db();
        let names = self.names_now(&conn)?;
        let kind = self.log_kind(&conn, ch)?;
        let shape = self.shape_for(&role, kind, &names);
        let [a, b, c] = self.prefix(&conn, ch, &names, summary, tail)?;
        let blocks = [wire::system(&a), wire::system(&b), wire::system(&c)];
        let id = log::open_segment(
            &conn,
            ch,
            [&blocks[0], &blocks[1], &blocks[2]],
            &shape.to_json(),
            summary,
            self.now().ms,
        )?;
        Ok(id)
    }

    /// Why the current segment must roll over, if it must.
    fn must_roll(&self, ch: ChannelId) -> Result<Option<Roll>> {
        let (role, _) = self.chat()?;
        let conn = self.db();
        let Some(seg) = log::current(&conn, ch)? else {
            return Ok(Some(Roll::Fresh));
        };
        let names = self.names_now(&conn)?;
        let stored = wire::Shape::parse(&seg.shape);
        match stored {
            Some(s) if s.wire != role.profile.wire_api => return Ok(Some(Roll::Incompatible)),
            Some(s) if s.user != names.user() => return Ok(Some(Roll::Rename)),
            None => return Ok(Some(Roll::Incompatible)),
            _ => {}
        }
        // Text only: an attached image is not material, and its encoding would dwarf it.
        let entries = log::entries(&conn, seg.id)?;
        let material: usize = entries
            .iter()
            .filter(|e| e.role != Role::Assistant)
            .flat_map(|e| wire::texts(&e.bytes))
            .map(|t| t.len())
            .sum();
        let (c, _) = quota::ratios(role.prices(), &self.config.world.cost);
        let roll = &self.config.agent.roll;
        if roll.tokens(material) as f64 > roll.threshold(c) {
            return Ok(Some(Roll::Material));
        }
        if let Some(window) = role.model.context_window {
            let used = store::last_context_tokens(&conn, ch, seg.opened_at)?.unwrap_or(0);
            if used as f64 >= f64::from(window) * roll.window_margin {
                return Ok(Some(Roll::Window));
            }
        }
        Ok(None)
    }

    /// Make sure the channel has a segment that can take the next turn. `before` is a
    /// message about to be appended, which a rollover's tail must not repeat.
    pub(crate) async fn ensure_segment(
        &self,
        ch: ChannelId,
        before: Option<MessageId>,
    ) -> Result<SegmentId> {
        match self.must_roll(ch)? {
            None => {}
            Some(Roll::Fresh) => {
                self.open_segment(ch, None, None)?;
            }
            Some(Roll::Incompatible) => {
                // Stored items are in another wire format: nothing more can be appended,
                // and no wrap-up can run on them either.
                self.open_segment(ch, None, Some(Tail { before }))?;
            }
            Some(Roll::Rename | Roll::Material | Roll::Window) => {
                self.roll(ch, before).await?;
            }
        }
        Ok(log::current(&self.db(), ch)?
            .ok_or_else(|| Error::Invalid(format!("channel {ch} has no log")))?
            .id)
    }

    /// Roll a channel over: the wrap-up with a summary on the old segment, then a new one.
    pub(crate) async fn roll(&self, ch: ChannelId, before: Option<MessageId>) -> Result<SegmentId> {
        let summary = self.run_wrapup(ch, true).await?;
        self.open_segment(ch, summary.as_ref(), Some(Tail { before }))
    }

    /// The items of a segment in body order, and the ids it already carries in full.
    pub(crate) fn items(&self, seg: SegmentId) -> Result<Vec<Vec<u8>>> {
        let conn = self.db();
        let s = segment(&conn, seg)?;
        let mut out = vec![s.prefix_a, s.prefix_b, s.prefix_c];
        out.extend(log::entries(&conn, seg)?.into_iter().map(|e| e.bytes));
        Ok(out)
    }

    /// Ids a segment has already shown: every `[#id]` the harness wrote into it.
    pub(crate) fn seen(&self, seg: SegmentId) -> Result<HashSet<String>> {
        let conn = self.db();
        let s = segment(&conn, seg)?;
        let mut out = HashSet::new();
        let mut scan = |bytes: &[u8]| {
            for t in wire::texts(bytes) {
                out.extend(assemble::ids_in(&t));
            }
        };
        scan(&s.prefix_c);
        for e in log::entries(&conn, seg)? {
            if e.role != Role::Assistant {
                scan(&e.bytes);
            }
        }
        Ok(out)
    }

    pub(crate) fn append(&self, seg: SegmentId, role: Role, bytes: &[u8]) -> Result<()> {
        log::append(&self.db(), seg, role, bytes, self.now().ms)?;
        Ok(())
    }

    /// Append a harness or user entry.
    pub(crate) fn append_user(
        &self,
        seg: SegmentId,
        text: &str,
        image: Option<&wire::Image>,
    ) -> Result<()> {
        let wire = self.segment_wire(seg)?;
        self.append(seg, Role::User, &wire::user(wire, text, image))
    }

    pub(crate) fn segment_wire(&self, seg: SegmentId) -> Result<WireApi> {
        let s = segment(&self.db(), seg)?;
        Ok(wire::Shape::parse(&s.shape)
            .map(|s| s.wire)
            .unwrap_or(WireApi::Chat))
    }

    /// The segment's names: what its bytes call the user.
    pub(crate) fn segment_names(&self, seg: SegmentId) -> Result<Names> {
        let s = segment(&self.db(), seg)?;
        Ok(match wire::Shape::parse(&s.shape) {
            Some(shape) => Names::fixed(&self.corpus.placeholder, &shape.user),
            None => self.names_now(&self.db())?,
        })
    }

    /// The harness context line when it differs from the last one in this segment.
    pub(crate) fn context_line(
        &self,
        seg: SegmentId,
        turns: Option<u32>,
    ) -> Result<Option<String>> {
        let now = self.now();
        let d = world::clock::in_world_date(now, self.corpus.year_offset);
        let date = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
        let hour = now.local_minute() / 60;
        let key = format!("{date} {hour} {turns:?}");
        let mut contexts = self.contexts.lock().unwrap_or_else(|e| e.into_inner());
        if contexts.get(&seg) == Some(&key) {
            return Ok(None);
        }
        contexts.insert(seg, key);
        drop(contexts);
        let names = self.segment_names(seg)?;
        let time = world::clock::local_time(now);
        let line = match turns {
            Some(n) => self.corpus.wording.fill(
                "harness.context.turns",
                &names,
                &[("date", &date), ("time", &time), ("turns", &n.to_string())],
            ),
            None => self.corpus.wording.fill(
                "harness.context",
                &names,
                &[("date", &date), ("time", &time)],
            ),
        };
        Ok(Some(line))
    }

    /// One model call on the channel's current segment, streamed. The reply is appended
    /// verbatim and its usage metered before anything else happens.
    pub(crate) async fn call(&self, turn: &mut Turn, choice: Choice<'_>) -> Result<Reply> {
        let (role, target) = self.chat()?;
        let seg = log::current(&self.db(), turn.channel)?
            .ok_or_else(|| Error::Invalid(format!("channel {} has no log", turn.channel)))?
            .id;
        let kind = self.log_kind(&self.db(), turn.channel)?;
        let names = self.segment_names(seg)?;
        let shape = self.shape_for(&role, kind, &names);
        let items = self.items(seg)?;
        let refs: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
        let body = wire::body(&shape, &cache_key(seg), &refs, choice);
        let mut events = self.transport.stream(&target, body.bytes).await?;
        let mut decoder = Decoder::new(shape.wire);
        let mut failure = None;
        while let Some(event) = events.next().await {
            let piece = match event.and_then(|v| decoder.push(&v)) {
                Ok(p) => p,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            };
            if let Piece::Text(delta) = piece
                && !turn.wrapup
            {
                let id = self.message_for(turn)?;
                self.emit(Event::MessageDelta(MessageDelta {
                    channel: crate::view::channel_id(turn.channel),
                    message: crate::view::message_id(id),
                    author: crate::view::author(&turn.speaker),
                    delta,
                }));
            }
        }
        drop(events);
        let reply = match failure.map_or_else(|| decoder.finish(), Err) {
            Ok(r) => r,
            Err(e) => {
                // Nothing reached the log; a message row with no text yet goes too.
                if let Some(id) = turn.message
                    && turn.text.is_empty()
                {
                    store::discard_message(&self.db(), id)?;
                    turn.message = None;
                }
                return Err(e);
            }
        };
        let now = self.now();
        {
            let conn = self.db();
            log::append(
                &conn,
                seg,
                Role::Assistant,
                &wire::assistant(shape.wire, &reply.text, &reply.calls),
                now.ms,
            )?;
            quota::record_usage(
                &conn,
                now.ms,
                turn.shape,
                Some(turn.channel),
                &shape.model,
                reply.usage,
                role.prices(),
                &self.config.world.cost,
            )?;
        }
        self.emit_quota()?;
        Ok(reply)
    }

    pub(crate) fn emit_quota(&self) -> Result<()> {
        let now = self.now();
        let conn = self.db();
        let tier = world::settings::intensity(&conn)?;
        let status = quota::status(&conn, tier, now.ms, &self.config.world.quota)?;
        let view = crate::view::quota_status(&conn, &status, now)?;
        drop(conn);
        self.emit(Event::QuotaChanged(QuotaChanged { status: view }));
        Ok(())
    }

    /// The turn's message, created (empty) the first time something is shown under it.
    pub(crate) fn message_for(&self, turn: &mut Turn) -> Result<MessageId> {
        if let Some(id) = turn.message {
            return Ok(id);
        }
        let id = message::append(
            &self.db(),
            turn.channel,
            &turn.speaker,
            "",
            None,
            None,
            self.now().ms,
        )?;
        turn.message = Some(id);
        Ok(id)
    }

    pub(crate) fn show_action(&self, turn: &mut Turn, action: ToolAction) -> Result<()> {
        let id = self.message_for(turn)?;
        self.emit(Event::ToolCalled(ToolCalled {
            channel: crate::view::channel_id(turn.channel),
            message: crate::view::message_id(id),
            author: crate::view::author(&turn.speaker),
            action: action.clone(),
        }));
        turn.actions.push(action);
        Ok(())
    }

    /// Run the model on a turn until it answers in text, a request closes, or the step
    /// budget is spent. The last allowed step offers no tool, so it ends in text.
    pub(crate) async fn run(&self, turn: &mut Turn, max_steps: u32) -> Result<()> {
        let max_steps = max_steps.max(1);
        let mut warned = false;
        for step in 0..max_steps {
            if let Some(task) = turn.task {
                let t = world::task::get(&self.db(), task)?;
                if t.status == world::TaskStatus::Done {
                    turn.closed = true;
                    break;
                }
                match world::task::harness(t.turns_left) {
                    world::task::Harness::ForceClose => break,
                    world::task::Harness::MustReportThisTurn if !warned => {
                        warned = true;
                        let seg = self.current_segment(turn.channel)?;
                        let names = self.segment_names(seg)?;
                        let line = self.corpus.wording.fill("harness.must_report", &names, &[]);
                        self.append_user(seg, &line, None)?;
                    }
                    _ => {}
                }
                world::task::spend_turn(&self.db(), task)?;
            }
            let last = step + 1 == max_steps;
            let choice = if last && turn.task.is_none() {
                Choice::None
            } else {
                Choice::Auto
            };
            let reply = self.call(turn, choice).await?;
            if !reply.text.is_empty() {
                if !turn.text.is_empty() {
                    turn.text.push('\n');
                }
                turn.text.push_str(&reply.text);
            }
            if reply.calls.is_empty() {
                break;
            }
            let seg = self.current_segment(turn.channel)?;
            let wire = self.segment_wire(seg)?;
            for call in &reply.calls {
                let output = self.dispatch(turn, call).await?;
                self.append(seg, Role::Tool, &wire::tool_result(wire, &call.id, &output))?;
            }
            // A colleague pulled in speaks next; the caller resumes after him.
            if turn.closed || turn.joined.is_some() {
                break;
            }
        }
        self.finish_message(turn)
    }

    /// End of a request's run: still open means the turns ran out or the model answered
    /// without `report`. Either way what it said is the reply.
    pub(crate) fn close_open_task(&self, turn: &mut Turn) -> Result<()> {
        let Some(task) = turn.task else {
            return Ok(());
        };
        let active = world::task::get(&self.db(), task)?.status == world::TaskStatus::Active;
        if active && !turn.closed {
            self.force_close(turn, task)?;
        }
        Ok(())
    }

    pub(crate) fn current_segment(&self, ch: ChannelId) -> Result<SegmentId> {
        Ok(log::current(&self.db(), ch)?
            .ok_or_else(|| Error::Invalid(format!("channel {ch} has no log")))?
            .id)
    }

    /// Turns ran out: what was found goes out as the reply. A reply already streamed
    /// under the turn's message becomes the request's answer as it is; otherwise the
    /// plain "found nothing" line is sent.
    fn force_close(&self, turn: &mut Turn, task: TaskId) -> Result<()> {
        let now = self.now();
        if let Some(id) = turn.message
            && !turn.text.trim().is_empty()
        {
            let cites = self.citations(&turn.text, &turn.shown)?;
            store::close_task_with(&self.db(), task, id, &cites, now.ms)?;
            turn.closed = true;
            let meta = MessageMeta {
                cites,
                confidence: turn.level,
                task: Some(task),
                ..MessageMeta::default()
            };
            self.complete(turn, id, &meta)?;
            self.task_finished(task)?;
            return Ok(());
        }
        let names = self.names_now(&self.db())?;
        let text = self
            .corpus
            .wording
            .fill("harness.nothing_found", &names, &[]);
        let reported =
            world::task::force_close(&self.db(), task, &text, &[], &self.config.office, now.ms)?;
        self.after_report(turn, task, reported.message, None)
    }

    /// Store a turn's message with its text, actions and meta, and announce it.
    fn complete(&self, turn: &mut Turn, id: MessageId, meta: &MessageMeta) -> Result<()> {
        let actions = (!turn.actions.is_empty())
            .then(|| serde_json::to_value(&turn.actions))
            .transpose()?;
        let conn = self.db();
        store::complete_message(&conn, id, &turn.text, actions.as_ref(), meta)?;
        let stored = world::message::get(&conn, id)?;
        drop(conn);
        turn.message = None;
        turn.text.clear();
        turn.actions.clear();
        self.emit(Event::MessageAdded(MessageAdded {
            message: crate::view::message(&stored, self.now().offset_min, &self.config.office),
        }));
        Ok(())
    }

    /// A request was answered by `reply` (a new message). What the turn streamed before
    /// it stays its own message; an empty placeholder is dropped and its actions move
    /// onto the reply.
    pub(crate) fn after_report(
        &self,
        turn: &mut Turn,
        task: TaskId,
        reply: MessageId,
        note: Option<String>,
    ) -> Result<()> {
        turn.closed = true;
        if let Some(x) = turn.message {
            if turn.text.trim().is_empty() {
                store::discard_message(&self.db(), x)?;
                turn.message = None;
            } else {
                let cites = self.citations(&turn.text, &turn.shown)?;
                let meta = MessageMeta {
                    cites,
                    confidence: turn.level,
                    ..MessageMeta::default()
                };
                self.complete(turn, x, &meta)?;
            }
        }
        let t = world::task::get(&self.db(), task)?;
        let text = world::message::get(&self.db(), reply)?.text;
        let meta = MessageMeta {
            cites: t.cites.clone(),
            confidence: turn.level,
            task: Some(task),
            relayed: false,
            note: note.or(t.note_path.clone()),
        };
        turn.text = text;
        self.complete(turn, reply, &meta)?;
        self.task_finished(task)
    }

    /// Trust for a request the user gave, and the change announced.
    fn task_finished(&self, task: TaskId) -> Result<()> {
        let t = world::task::get(&self.db(), task)?;
        if t.granted.is_some()
            && let Some(p) = &t.assignee
        {
            world::trust::task_done(&self.db(), p, &self.config.world.trust)?;
        }
        self.changed(vec![
            api::events::WorldChange::Task {
                id: crate::view::task_id(task),
            },
            api::events::WorldChange::Digest,
        ]);
        Ok(())
    }

    /// Citations for the `[#id]`s in a reply that were shown in this session.
    pub(crate) fn citations(&self, text: &str, shown: &[String]) -> Result<Vec<world::Citation>> {
        let mut out: Vec<world::Citation> = Vec::new();
        let ix = self.ix();
        for id in assemble::ids_in(text) {
            if !shown.contains(&id) || out.iter().any(|c| c.chunk_id == id) {
                continue;
            }
            if let Some(u) = ix.folio().unit_by_id(&id)? {
                out.push(crate::view::unit_citation(&u));
            }
        }
        Ok(out)
    }

    /// Store the turn's reply text with its actions and citations, and announce it.
    fn finish_message(&self, turn: &mut Turn) -> Result<()> {
        if turn.wrapup || (turn.message.is_none() && turn.text.is_empty()) {
            return Ok(());
        }
        let id = self.message_for(turn)?;
        let meta = MessageMeta {
            cites: self.citations(&turn.text, &turn.shown)?,
            confidence: turn.level,
            ..MessageMeta::default()
        };
        // The turn's text is complete; a later forced close reads it from here.
        let text = turn.text.clone();
        self.complete(turn, id, &meta)?;
        turn.text = text;
        turn.message = Some(id);
        Ok(())
    }
}

/// What a new segment carries over from the old one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tail {
    /// Only messages before this one.
    pub before: Option<MessageId>,
}

enum Roll {
    Fresh,
    Incompatible,
    Rename,
    Material,
    Window,
}

pub(crate) fn segment(conn: &rusqlite::Connection, seg: SegmentId) -> Result<log::Segment> {
    Ok(conn.query_row(
        "SELECT id, channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at
         FROM log_segment WHERE id = ?1",
        [seg],
        |r| {
            Ok(log::Segment {
                id: r.get(0)?,
                channel: r.get(1)?,
                seq: r.get(2)?,
                prefix_a: r.get(3)?,
                prefix_b: r.get(4)?,
                prefix_c: r.get(5)?,
                shape: r.get(6)?,
                opened_at: r.get(7)?,
            })
        },
    )?)
}
