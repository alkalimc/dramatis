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

use api::events::{
    ActivityFailure, ActivityState, MessageAdded, MessageDelta, QuotaChanged, ToolCalled,
    TurnActivity,
};
use api::meta::MessageMeta;
use api::views::ToolAction;
use futures::StreamExt;
use llm::{Event as LlmEvent, Message, Part, Response, ToolChoice};
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
use crate::meter;
use crate::persona;
use crate::request::{self, Call, Image};
use crate::text::Names;

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
    /// The last activity announced for this turn, so only changes are sent.
    pub activity: Option<ActivityState>,
    /// Names the reply's closing hand-off line listed (group replies only).
    pub hand_off: Vec<String>,
    /// This turn's person has no tool list (emulated tools), so a proposal to the host is a
    /// closing line of the reply.
    pub proposing: bool,
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
            activity: None,
            hand_off: Vec::new(),
            proposing: false,
        }
    }

    /// Whether this turn is a reply in a group, where the closing hand-off line applies.
    fn in_group(&self) -> bool {
        matches!(self.shape, Shape::Group | Shape::Interject)
    }
}

/// Ends a turn's activity when the turn's future is dropped before it finished, so a
/// cancelled turn never leaves an indicator behind.
struct ActivityGuard<'a> {
    agent: &'a Agent,
    channel: ChannelId,
    speaker: Actor,
    ended: bool,
}

impl Drop for ActivityGuard<'_> {
    fn drop(&mut self) {
        if !self.ended {
            self.agent.announce(
                self.channel,
                &self.speaker,
                ActivityState::Failed {
                    reason: ActivityFailure::Cancelled,
                },
            );
        }
    }
}

fn failure(e: &Error) -> ActivityFailure {
    match e {
        Error::RateLimited { .. } => ActivityFailure::RateLimited,
        Error::QuotaExhausted { .. } => ActivityFailure::QuotaExhausted,
        Error::ImageRejected { .. } => ActivityFailure::ImageUnsupported,
        _ => ActivityFailure::Error,
    }
}

fn cache_key(segment: SegmentId) -> String {
    format!("dramatis-{segment}")
}

/// Weekday keys of the corpus wording, Monday first.
const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Most groups a direct chat's opening lists for its person.
const GROUPS_LISTED_MAX: usize = 5;

impl Agent {
    pub(crate) fn log_kind(&self, conn: &rusqlite::Connection, ch: ChannelId) -> Result<LogKind> {
        Ok(if channel::get(conn, ch)?.is_host_channel() {
            LogKind::Host
        } else {
            LogKind::Person
        })
    }

    /// The request shape for a log kind under the role of that kind.
    pub(crate) fn shape_for(
        &self,
        role: &ChatRole,
        kind: LogKind,
        names: &Names,
    ) -> request::Shape {
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
        request::Shape {
            wire: role.profile.wire_api,
            model: role.model.id.clone(),
            reasoning,
            tools,
            user: names.user().to_owned(),
            about: names.about().unwrap_or_default().to_owned(),
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
            let a = assemble::block_a_host(
                &ctx,
                &self.corpus.host_name,
                &self.corpus.host,
                self.config.frontend,
            );
            let c_block = assemble::block_c(
                &ctx,
                &assemble::Opening {
                    tones: Vec::new(),
                    topic: None,
                    memories: &[],
                    memory_owner: None,
                    summary: summary.map(|s| s.text.as_str()),
                    recent: &recent,
                    groups: None,
                },
            );
            return Ok([a, String::new(), c_block]);
        }
        // Where the persons' tools are emulated they get no tool list, so proposals to the
        // host are a closing line, explained here.
        let proposing = self.role_for(LogKind::Person)?.0.model.emulated_tools();
        let a = assemble::block_a(&ctx, c.kind == ChannelKind::Group, proposing);
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

        // These bytes are fixed for the segment's life: a failed read is an error, not a
        // default tone.
        let mut tones: Vec<(&str, world::trust::Tone)> = Vec::new();
        for (p, name) in people.iter().zip(&names_of) {
            let trust = bond::user_bond(conn, p)?.trust;
            tones.push((
                name.as_str(),
                world::trust::tone(trust, &self.config.world.trust),
            ));
        }
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
        // A direct chat tells its person which groups he is in, fixed for the segment. A
        // group's log is shared by all its members, so it lists none: its members are the
        // roster above.
        let mut briefs = Vec::new();
        if let (ChannelKind::Direct, Some(me)) = (c.kind, c.person()) {
            for g in channel::groups_of(conn, me)?
                .into_iter()
                .take(GROUPS_LISTED_MAX)
            {
                let mut others = Vec::new();
                for p in g.persons().iter().filter(|p| *p != me) {
                    others.push(
                        folio
                            .person(p.as_str())?
                            .map(|x| x.display)
                            .unwrap_or_else(|| p.0.clone()),
                    );
                }
                briefs.push(assemble::GroupBrief {
                    title: g.name.clone(),
                    others: others.join(self.corpus.wording.get("list.separator")),
                    purpose: g.topic.clone(),
                });
            }
        }
        let groups = match (briefs.is_empty(), names_of.first()) {
            (false, Some(person)) => Some(assemble::Groups {
                person,
                list: &briefs,
            }),
            _ => None,
        };
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
                groups,
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
        let bio = world::settings::user_bio(conn)?;
        Ok(Names::new(
            &self.corpus.placeholder,
            name.as_deref(),
            &self.corpus.wording,
        )
        .with_about(bio.as_deref()))
    }

    /// Open the next segment of a channel. A rollover passes the old segment's summary
    /// and a [`Tail`]: the last messages go into the opening block.
    pub(crate) fn open_segment(
        &self,
        ch: ChannelId,
        summary: Option<&Summary>,
        tail: Option<Tail>,
    ) -> Result<SegmentId> {
        self.sweep();
        let conn = self.db();
        let kind = self.log_kind(&conn, ch)?;
        let (role, _) = self.role_for(kind)?;
        let names = self.names_now(&conn)?;
        let shape = self.shape_for(&role, kind, &names);
        let [a, b, c] = self.prefix(&conn, ch, &names, summary, tail)?;
        let id = log::open_segment(
            &conn,
            ch,
            [a.as_bytes(), b.as_bytes(), c.as_bytes()],
            &shape.to_json(),
            summary,
            self.now().ms,
        )?;
        Ok(id)
    }

    /// Why the current segment must roll over, if it must.
    fn must_roll(&self, ch: ChannelId) -> Result<Option<Roll>> {
        let conn = self.db();
        let (role, _) = self.role_for(self.log_kind(&conn, ch)?)?;
        let Some(seg) = log::current(&conn, ch)? else {
            return Ok(Some(Roll::Fresh));
        };
        let names = self.names_now(&conn)?;
        let Some(stored) = request::Shape::parse(&seg.shape) else {
            return Ok(Some(Roll::Unreadable));
        };
        if !stored.knows(&names) {
            return Ok(Some(Roll::Rename));
        }
        let entries = log::entries(&conn, seg.id)?;
        let Some(items) = entries
            .iter()
            .map(|e| request::parse(&e.bytes))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(Some(Roll::Unreadable));
        };
        // Text only: an attached image is not material, and its encoding would dwarf it.
        let material: usize = items
            .iter()
            .flat_map(request::harness_texts)
            .map(str::len)
            .sum();
        let (c, _) = quota::ratios(role.prices(), &self.config.world.cost);
        let roll = &self.config.agent.roll;
        // A per-call fee makes a long segment free to keep: only the window limits it.
        if !role.per_request() && roll.tokens(material) as f64 > roll.threshold(c) {
            return Ok(Some(Roll::Material));
        }
        if let Some(window) = role.model.context_window {
            let known = self
                .filled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&seg.id)
                .copied();
            let used = known.unwrap_or_else(|| {
                let bytes = seg.prefix_a.len()
                    + seg.prefix_b.len()
                    + seg.prefix_c.len()
                    + entries.iter().map(|e| e.bytes.len()).sum::<usize>();
                roll.tokens(bytes)
            });
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
            Some(Roll::Unreadable) => {
                // Written by another build: nothing more can be appended, and no wrap-up
                // can run on it either.
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

    /// A segment's prefix blocks and items, in body order.
    pub(crate) fn log(&self, seg: SegmentId) -> Result<Log> {
        let conn = self.db();
        let s = segment(&conn, seg)?;
        let text = |b: Vec<u8>| String::from_utf8(b).unwrap_or_default();
        let items = log::entries(&conn, seg)?
            .iter()
            .filter_map(|e| request::parse(&e.bytes))
            .collect();
        Ok(Log {
            system: [text(s.prefix_a), text(s.prefix_b), text(s.prefix_c)],
            items,
        })
    }

    /// Ids a segment has already shown: every `[#id]` the harness wrote into it.
    pub(crate) fn seen(&self, seg: SegmentId) -> Result<HashSet<String>> {
        let log = self.log(seg)?;
        let mut out: HashSet<String> = api::cites::ids(&log.system[2]).into_iter().collect();
        for t in log.items.iter().flat_map(request::harness_texts) {
            out.extend(api::cites::ids(t));
        }
        Ok(out)
    }

    pub(crate) fn append(&self, seg: SegmentId, item: &Message) -> Result<()> {
        log::append(
            &self.db(),
            seg,
            log_role(item.role),
            &request::to_bytes(item),
            self.now().ms,
        )?;
        Ok(())
    }

    /// Append a harness or user entry.
    pub(crate) fn append_user(
        &self,
        seg: SegmentId,
        text: &str,
        image: Option<&Image>,
    ) -> Result<()> {
        self.append(seg, &request::user(text, image))
    }

    /// The segment's names: what its bytes call the user.
    pub(crate) fn segment_names(&self, seg: SegmentId) -> Result<Names> {
        let s = segment(&self.db(), seg)?;
        Ok(match request::Shape::parse(&s.shape) {
            Some(shape) => Names::fixed(&self.corpus.placeholder, &shape.user),
            None => self.names_now(&self.db())?,
        })
    }

    /// The harness context line of one turn: the in-world date, the weekday and time of
    /// day where the user is, and, when it is long, how long ago the user last wrote
    /// (`since`, the instant of that message). It goes in the user's entry, never in the
    /// cached prefix, and is written every turn: a clock quoted once would sit far back in
    /// the log by the time someone asks the time.
    pub(crate) fn context_line(
        &self,
        seg: SegmentId,
        turns: Option<u32>,
        since: Option<i64>,
    ) -> Result<String> {
        let now = self.now();
        let d = world::clock::in_world_date(now, self.corpus.year_offset);
        let date = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
        let names = self.segment_names(seg)?;
        let w = &self.corpus.wording;
        let local_minutes = now.ms.div_euclid(world::clock::MINUTE_MS) + i64::from(now.offset_min);
        // 1970-01-01 was a Thursday.
        let weekday = WEEKDAYS[(local_minutes.div_euclid(24 * 60) + 3).rem_euclid(7) as usize];
        let part = match now.local_minute() / 60 {
            22..=23 | 0..=4 => "night",
            5..=10 => "morning",
            11..=13 => "noon",
            14..=17 => "afternoon",
            _ => "evening",
        };
        let (weekday, part) = (format!("weekday.{weekday}"), format!("daypart.{part}"));
        let time = world::clock::local_time(now);
        let fields = [
            ("date", date.as_str()),
            ("weekday", w.get(&weekday)),
            ("time", time.as_str()),
            ("part", w.get(&part)),
        ];
        let mut line = match turns {
            Some(n) => {
                let n = n.to_string();
                let mut f = fields.to_vec();
                f.push(("turns", &n));
                w.fill("harness.context.turns", &names, &f)
            }
            None => w.fill("harness.context", &names, &fields),
        };
        let hours = since.map_or(0, |at| (now.ms - at).max(0) / world::clock::HOUR_MS);
        if hours >= i64::from(self.config.agent.context.gap_hours) {
            let span = if hours >= 48 {
                w.fill("span.days", &names, &[("n", &(hours / 24).to_string())])
            } else {
                w.fill("span.hours", &names, &[("n", &hours.to_string())])
            };
            line.push(' ');
            line.push_str(&w.fill("harness.context.gap", &names, &[("span", &span)]));
        }
        Ok(line)
    }

    /// One model call on the channel's current segment, streamed. The reply is appended
    /// verbatim and its usage metered before anything else happens.
    pub(crate) async fn call(&self, turn: &mut Turn, choice: ToolChoice) -> Result<Response> {
        let kind = self.log_kind(&self.db(), turn.channel)?;
        let (role, ep) = self.role_for(kind)?;
        let seg = log::current(&self.db(), turn.channel)?
            .ok_or_else(|| Error::Invalid(format!("channel {} has no log", turn.channel)))?
            .id;
        let names = self.segment_names(seg)?;
        let mut shape = self.shape_for(&role, kind, &names);
        // Emulated tools do not answer a wrap-up reliably, but structured output does: the
        // call carries no tools and asks for the `wrapup` arguments as one JSON document.
        let structured = role.model.emulated_tools() && turn.wrapup;
        let schema = structured
            .then(|| shape.tools.iter().find(|t| t.name == "wrapup"))
            .flatten()
            .map(|t| t.parameters.clone());
        let choice = if structured { ToolChoice::Auto } else { choice };
        if structured {
            shape.tools.clear();
        }
        // The stored shape keeps the full list; only this call's request leaves it out.
        let proposing = role.model.emulated_tools()
            && kind == LogKind::Person
            && matches!(
                turn.shape,
                Shape::Direct | Shape::Group | Shape::Interject | Shape::Opening
            );
        turn.proposing = proposing;
        if proposing {
            shape.tools.clear();
        }
        let log = self.log(seg)?;
        // A model that cannot see gets the pictures in words. One that refused a picture
        // is asked again with the newest one only: its history must not fail every turn.
        let model_key = format!("{}/{}", role.profile.name, role.model.id);
        let learned = self
            .sightless
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&model_key);
        let described;
        let items = if (learned || !role.model.sees()) && request::has_picture(&log.items) {
            let note = self
                .corpus
                .wording
                .fill("harness.picture_unseen", &names, &[]);
            described = request::without_pictures(&log.items, role.model.sees(), &note);
            &described
        } else {
            &log.items
        };
        let sent_picture = request::has_picture(items);
        let mut req = request::build(
            &Call {
                shape: &shape,
                cache_key: cache_key(seg),
                system: [&log.system[0], &log.system[1], &log.system[2]],
                messages: items,
                choice,
            },
            &ep,
        )?;
        req.output_schema = schema;
        let mut retried = false;
        let (reply, refused) = loop {
            let mut events = match self.llm.stream(&ep, &req).await {
                Ok(events) => events,
                Err(e) => return Err(self.picture_refusal(e, sent_picture, &model_key)),
            };
            let mut done = None;
            let mut failure = None;
            let mut streamed = false;
            // Text that may still turn out to be a canned refusal is held back, so a
            // refusal never reaches the screen.
            let mut held = String::new();
            let mut holding = true;
            // A group reply's trailing hand-off line, held back until it is known not to be.
            let marks = assemble::closing_marks(&self.corpus.wording, turn.in_group(), proposing);
            let mark_list = marks.list();
            let mut tail = String::new();
            while let Some(event) = events.next().await {
                match event {
                    Ok(LlmEvent::TextDelta(delta)) if !turn.wrapup => {
                        if holding {
                            held.push_str(&delta);
                            if assemble::refusal(&held, self.corpus.cjk) == assemble::Refusal::No {
                                holding = false;
                                let all = std::mem::take(&mut held);
                                self.stream_text(turn, &mut streamed, &mut tail, &mark_list, all)?;
                            }
                        } else {
                            self.stream_text(turn, &mut streamed, &mut tail, &mark_list, delta)?;
                        }
                    }
                    Ok(LlmEvent::Done(r)) => done = Some(r),
                    Ok(_) => {}
                    Err(e) => {
                        failure = Some(self.picture_refusal(e, sent_picture, &model_key));
                        break;
                    }
                }
            }
            drop(events);
            let reply = match (failure, done) {
                (None, Some(r)) => r,
                (failure, _) => {
                    // Nothing reached the log; a message row with no text yet goes too.
                    if let Some(id) = turn.message
                        && turn.text.is_empty()
                    {
                        world::message::discard(&self.db(), id)?;
                        turn.message = None;
                    }
                    return Err(failure.unwrap_or_else(|| Error::Endpoint {
                        detail: "the reply ended before it was complete".into(),
                    }));
                }
            };
            if sent_picture {
                self.sightless
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&model_key);
            }
            let refused = reply.tool_calls().next().is_none()
                && assemble::refusal(&reply.text(), self.corpus.cjk) == assemble::Refusal::Is;
            if !refused && !held.is_empty() {
                self.stream_text(turn, &mut streamed, &mut tail, &mark_list, held)?;
            }
            // What is still held at the end is the closing line itself, or a start of
            // speech that only looked like one.
            if !refused && !tail.is_empty() && assemble::split_closing(&tail, marks).text == tail {
                self.stream_delta(turn, &mut streamed, std::mem::take(&mut tail))?;
            }
            if refused && !retried {
                // A fresh request, paid like any other; the refusal is not kept.
                retried = true;
                self.record_usage(turn, seg, &shape.model, &role, reply.usage)?;
                continue;
            }
            break (reply, refused);
        };
        let mut reply = if structured {
            adopt_json_call(reply)
        } else if role.model.emulated_tools() && !shape.tools.is_empty() {
            adopt_text_call(reply, false, &shape.tools)
        } else {
            reply
        };
        if refused {
            // Twice in a row: the turn goes on without a reply, and the log keeps no
            // words that are not the person's.
            reply.parts.clear();
        } else {
            let item = Message::assistant(reply.parts.clone());
            log::append(
                &self.db(),
                seg,
                Role::Assistant,
                &request::to_bytes(&item),
                self.now().ms,
            )?;
        }
        self.record_usage(turn, seg, &shape.model, &role, reply.usage)?;
        Ok(reply)
    }

    /// The error of a failed call. A client error on a request that carried a picture is
    /// taken as the endpoint refusing the picture (the answer rarely says so in a way that
    /// can be matched), and the model is remembered as not seeing.
    fn picture_refusal(&self, e: llm::Error, sent_picture: bool, model_key: &str) -> Error {
        match e {
            llm::Error::Http {
                status: status @ (400 | 413 | 415 | 422),
                message,
            } if sent_picture => {
                self.sightless
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(model_key.to_owned());
                Error::ImageRejected {
                    detail: format!("HTTP {status}: {message}"),
                }
            }
            other => other.into(),
        }
    }

    /// One call's usage: the segment's fill, the meter row and the quota event.
    fn record_usage(
        &self,
        turn: &Turn,
        seg: SegmentId,
        model: &str,
        role: &ChatRole,
        u: llm::Usage,
    ) -> Result<()> {
        self.filled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                seg,
                u.uncached_in + u.cached_in + u.cache_write_in + u.output,
            );
        meter::record(
            &self.db(),
            self.now().ms,
            turn.shape,
            Some(turn.channel),
            model,
            u,
            role.prices(),
            role.cache_write_ratio(),
            role.per_request(),
            &self.config.world.cost,
            &self.config.agent.cost,
        )?;
        self.emit_quota()
    }

    /// Show a piece of a reply as it arrives, keeping back trailing lines that are or may
    /// become closing lines. With no marker it passes straight through.
    fn stream_text(
        &self,
        turn: &mut Turn,
        streamed: &mut bool,
        tail: &mut String,
        marks: &[&str],
        delta: String,
    ) -> Result<()> {
        if marks.is_empty() {
            return self.stream_delta(turn, streamed, delta);
        }
        tail.push_str(&delta);
        let n = assemble::shown_len(tail, marks);
        if n > 0 {
            let rest = tail.split_off(n);
            let shown = std::mem::replace(tail, rest);
            self.stream_delta(turn, streamed, shown)?;
        }
        Ok(())
    }

    /// Show a piece of the reply as it arrives. A later step's text joins the earlier
    /// one on a new line, as stored.
    fn stream_delta(&self, turn: &mut Turn, streamed: &mut bool, mut delta: String) -> Result<()> {
        if !std::mem::replace(streamed, true) && !turn.text.is_empty() {
            delta.insert(0, '\n');
        }
        let id = self.message_for(turn)?;
        self.set_activity(turn, ActivityState::Writing);
        self.emit(Event::MessageDelta(MessageDelta {
            channel: crate::view::channel_id(turn.channel),
            message: crate::view::message_id(id),
            author: crate::view::author(&turn.speaker),
            delta,
        }));
        Ok(())
    }

    /// Whether the protocol of the channel's role lets `tool_choice` differ between calls
    /// on one log. Where it does not, a different choice drops the cached history, so
    /// every call sends the same one and the harness asks in text instead.
    pub(crate) fn may_vary_choice(&self, ch: ChannelId) -> Result<bool> {
        let kind = self.log_kind(&self.db(), ch)?;
        Ok(self.role_for(kind)?.0.profile.wire_api.may_force_tool())
    }

    pub(crate) fn emit_quota(&self) -> Result<()> {
        let _order = self.quota_events.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.now();
        let conn = self.db();
        let tier = world::settings::intensity(&conn)?;
        let status = quota::status(&conn, tier, now.ms, &self.config.world.quota)?;
        let view = crate::view::quota_status(&conn, &status, now, &self.config.world.quota)?;
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

    fn announce(&self, channel: ChannelId, speaker: &Actor, state: ActivityState) {
        self.emit(Event::TurnActivity(TurnActivity {
            channel: crate::view::channel_id(channel),
            author: crate::view::author(speaker),
            state,
        }));
    }

    /// Announce the turn's activity if it changed. The wrap-up is invisible.
    pub(crate) fn set_activity(&self, turn: &mut Turn, state: ActivityState) {
        if turn.wrapup || turn.activity.as_ref() == Some(&state) {
            return;
        }
        self.announce(turn.channel, &turn.speaker, state.clone());
        turn.activity = Some(state);
    }

    /// Run the model on a turn until it answers in text, a request closes, or the step
    /// budget is spent. The last allowed step offers no tool, so it ends in text.
    /// The turn's activity runs from here to the end, on every path out.
    pub(crate) async fn run(&self, turn: &mut Turn, max_steps: u32) -> Result<()> {
        let mut guard = ActivityGuard {
            agent: self,
            channel: turn.channel,
            speaker: turn.speaker.clone(),
            ended: turn.wrapup,
        };
        let out = self.run_steps(turn, max_steps).await;
        if out.is_err() {
            // What the screen showed before the failure is the message, not an empty row.
            let _ = self.finish_message(turn);
        }
        guard.ended = true;
        let end = match &out {
            Ok(()) => ActivityState::Done,
            Err(e) => ActivityState::Failed { reason: failure(e) },
        };
        self.set_activity(turn, end);
        out
    }

    async fn run_steps(&self, turn: &mut Turn, max_steps: u32) -> Result<()> {
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
                let conn = self.db();
                world::task::spend_turn(&conn, task)?;
                // Tells another process that shares the save this request is alive.
                world::task::touch(&conn, task, self.now().ms)?;
                drop(conn);
            }
            let last = step + 1 == max_steps;
            let choice = if last && turn.task.is_none() && self.may_vary_choice(turn.channel)? {
                ToolChoice::None
            } else {
                ToolChoice::Auto
            };
            self.set_activity(turn, ActivityState::Thinking);
            let reply = self.call(turn, choice).await?;
            let text =
                assemble::visible_text(&reply.text(), (&self.corpus.scene.0, &self.corpus.scene.1));
            if !text.is_empty() {
                if !turn.text.is_empty() {
                    turn.text.push('\n');
                }
                turn.text.push_str(&text);
            }
            let calls: Vec<_> = reply.tool_calls().collect();
            if calls.is_empty() {
                break;
            }
            let seg = self.current_segment(turn.channel)?;
            let mut results = Vec::with_capacity(calls.len());
            for (id, name, args) in &calls {
                self.set_activity(
                    turn,
                    ActivityState::Tool {
                        name: (*name).into(),
                    },
                );
                match self.dispatch(turn, name, args).await {
                    Ok(output) => results.push(request::tool_result_part(id, output)),
                    Err(e) => {
                        // The reply with these calls is already in the log; every call
                        // needs an output or the endpoint refuses the channel from now on.
                        for (id, _, _) in &calls[results.len()..] {
                            results.push(request::failed_tool_result(id));
                        }
                        let _ = self.append(seg, &request::tool_results(results));
                        return Err(e);
                    }
                }
            }
            self.append(seg, &request::tool_results(results))?;
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
            let cites = self.citations(turn.channel, &turn.text, &turn.shown)?;
            world::task::close_with(&self.db(), task, id, &cites, now.ms)?;
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
        let meta = (!meta.is_empty())
            .then(|| serde_json::to_value(meta))
            .transpose()?;
        world::message::complete(&conn, id, &turn.text, actions.as_ref(), meta.as_ref())?;
        let stored = world::message::get(&conn, id)?;
        let shown =
            crate::view::message(&conn, &stored, self.now().offset_min, &self.config.office);
        drop(conn);
        turn.message = None;
        turn.text.clear();
        turn.actions.clear();
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
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
                world::message::discard(&self.db(), x)?;
                turn.message = None;
            } else {
                let cites = self.citations(turn.channel, &turn.text, &turn.shown)?;
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
            ..MessageMeta::default()
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

    /// Citations for the `[#id]`s in a reply that were shown in this session: in this
    /// turn (`shown`) or anywhere in the channel's current log.
    pub(crate) fn citations(
        &self,
        ch: ChannelId,
        text: &str,
        shown: &[String],
    ) -> Result<Vec<world::Citation>> {
        // Bound first: a guard in a `match` scrutinee would live through the arms.
        let current = log::current(&self.db(), ch)?;
        let seen = match current {
            Some(seg) => self.seen(seg.id)?,
            None => HashSet::new(),
        };
        let mut out: Vec<world::Citation> = Vec::new();
        let ix = self.ix();
        for id in api::cites::ids(text) {
            let Some(id) = assemble::resolve_id(&id, shown.iter().chain(seen.iter())) else {
                continue;
            };
            if out.iter().any(|c| c.chunk_id == id) {
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
        // A hand-over with nothing said first: the colleague's answer is the reply, so
        // no empty bubble stays behind.
        if turn.joined.is_some() && turn.text.trim().is_empty() {
            if let Some(id) = turn.message.take() {
                world::message::discard(&self.db(), id)?;
            }
            turn.text.clear();
            turn.actions.clear();
            return Ok(());
        }
        let marks = assemble::closing_marks(&self.corpus.wording, turn.in_group(), turn.proposing);
        let closing = assemble::split_closing(&turn.text, marks);
        turn.text = closing.text.clone();
        turn.hand_off = closing.hand_off.clone();
        if turn.proposing && (closing.find.is_some() || closing.group.is_some()) {
            self.propose(turn, closing)?;
        }
        if turn.in_group() {
            // A model that starts by repeating the message before it word for word: the
            // copy is not this person's line.
            let before = message::history(&self.db(), turn.channel, turn.message, 1)?;
            if let Some(prev) = before.first().filter(|m| m.author != turn.speaker) {
                let echo = prev.text.trim();
                if echo.chars().count() >= 8
                    && let Some(rest) = turn.text.trim_start().strip_prefix(echo)
                {
                    turn.text = rest.trim_start().to_owned();
                }
            }
            // Only the hand-off line was written: nothing to keep as a message.
            if turn.text.trim().is_empty() && turn.actions.is_empty() {
                if let Some(id) = turn.message.take() {
                    world::message::discard(&self.db(), id)?;
                }
                return Ok(());
            }
        }
        let id = self.message_for(turn)?;
        let meta = MessageMeta {
            cites: self.citations(turn.channel, &turn.text, &turn.shown)?,
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

/// A segment as a request is built from it.
pub(crate) struct Log {
    pub system: [String; 3],
    pub items: Vec<Message>,
}

/// The reply to a structured wrap-up request (a JSON object, possibly in a code fence,
/// keeping only the schema's own top-level keys): the JSON document becomes the `wrapup`
/// call, so the loop treats it like a tool call. A reply that is not a JSON object is
/// still read as a fenced call block; with neither it stays text, and nothing is recorded.
fn adopt_json_call(mut reply: Response) -> Response {
    let Some(doc) = crate::wrapup_json::document(&reply.text()) else {
        return adopt_text_call(reply, true, &[]);
    };
    reply.parts.retain(|p| !matches!(p, Part::Text(_)));
    reply.parts.push(Part::ToolCall {
        id: "json_call_0".into(),
        name: "wrapup".into(),
        args_json: doc,
    });
    reply
}

/// On an endpoint that only emulates tools, a call may arrive as a fenced block in the
/// text instead of a structured call. When no structured call came, the block becomes
/// the call, so the loop treats both alike; the stored reply keeps the words before it.
/// Only a tool the request offered counts (in a wrap-up, only `wrapup`).
fn adopt_text_call(mut reply: Response, wrapup: bool, tools: &[api::tools::ToolSpec]) -> Response {
    if reply.tool_calls().next().is_some() {
        return reply;
    }
    let Some((name, args_json, lead)) = assemble::text_call(&reply.text()) else {
        return reply;
    };
    let offered = if wrapup {
        name == "wrapup"
    } else {
        tools.iter().any(|t| t.name == name)
    };
    if !offered {
        return reply;
    }
    reply.parts.retain(|p| !matches!(p, Part::Text(_)));
    if !lead.is_empty() {
        reply.parts.push(Part::Text(lead));
    }
    reply.parts.push(Part::ToolCall {
        id: "text_call_0".into(),
        name,
        args_json,
    });
    reply
}

fn log_role(role: llm::Role) -> Role {
    match role {
        llm::Role::User => Role::User,
        llm::Role::Assistant => Role::Assistant,
        llm::Role::Tool => Role::Tool,
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
    Unreadable,
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
