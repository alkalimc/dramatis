//! What the app calls: a user turn, a request, a wrap-up, presence, a rename.
//!
//! A user turn is two steps so the command can return at once: [`Agent::post`] stores
//! and echoes the user's message, [`Agent::respond`] runs the replies (streamed as
//! events). [`Agent::send`] does both.
//!
//! The host's mode and memory tools run only on a turn the user wrote: a [`Posted`]
//! from the host channel carries `authorized = true`, every harness turn (presence,
//! digest, introduction) carries `false`. Whether the user's words actually ask for the
//! change is the model's reading; that only the user's own turn can carry it is the
//! harness's guarantee.

use std::collections::HashSet;

use api::events::{MessageAdded, Notification, WorldChange};
use world::log::Role;
use world::presence::{Activity, Digest};
use world::quota::{self, Band, CallKind, Decision};
use world::seed::{self, Opening, SeedKind, WordsHit};
use world::wrapup::Summary;
use world::{
    Actor, Call, ChannelId, ChannelKind, MessageId, MonthDay, PersonId, Shape, TaskId, bond,
    channel, fact, message, session, settings,
};

use crate::agent::{Agent, Event};
use crate::assemble::{self, Member};
use crate::error::{Error, Result, timestamp};
use crate::persona;
use crate::session::{LogKind, Turn};
use crate::text::Names;
use crate::view;
use crate::wire::{self, Choice};

/// A user message stored and waiting for its replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub channel: ChannelId,
    pub message: MessageId,
    pub text: String,
    pub image: Option<wire::Image>,
    /// The user wrote this turn: the host may change modes and memories in it.
    pub authorized: bool,
}

/// What a presence step did.
#[derive(Debug, Clone, PartialEq)]
pub struct PresenceOutcome {
    pub digest: Digest,
    /// Whether the host said something.
    pub host_spoke: bool,
    pub opened: Option<(PersonId, ChannelId)>,
}

impl Agent {
    // ---- user turns ----

    /// Store the user's message and echo it. Nothing is sent to a model yet.
    pub fn post(
        &self,
        channel: ChannelId,
        text: &str,
        image: Option<wire::Image>,
    ) -> Result<Posted> {
        self.post_with(channel, text, image, &crate::store::MessageMeta::default())
    }

    /// Pass a quoted excerpt to someone: it is posted in his direct channel as the
    /// user's message, marked as a retelling. Run [`Agent::respond`] on the result.
    pub fn relay(&self, excerpt: &str, to: &PersonId) -> Result<Posted> {
        let ch = self.open_direct(to)?;
        let meta = crate::store::MessageMeta {
            relayed: true,
            ..crate::store::MessageMeta::default()
        };
        self.post_with(ch, excerpt, None, &meta)
    }

    fn post_with(
        &self,
        channel: ChannelId,
        text: &str,
        image: Option<wire::Image>,
        meta: &crate::store::MessageMeta,
    ) -> Result<Posted> {
        if text.trim().is_empty() && image.is_none() {
            return Err(Error::Invalid("empty message".into()));
        }
        let now = self.now();
        let conn = self.db();
        let ch = channel::get(&conn, channel)?;
        if let Some(p) = ch.person() {
            self.available(&conn, p)?;
        }
        let id = message::append(&conn, channel, &Actor::User, text, None, None, now.ms)?;
        if !meta.is_empty() {
            crate::store::complete_message(&conn, id, text, None, meta)?;
        }
        session::touch_channel(&conn, channel, now.ms)?;
        let stored = message::get(&conn, id)?;
        let host = ch.is_host_channel();
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded {
            message: view::message(&stored, now.offset_min, &self.config.office),
        }));
        Ok(Posted {
            channel,
            message: id,
            text: text.to_owned(),
            image,
            authorized: host,
        })
    }

    /// Run the replies to a posted message. Errors after the message was stored (the
    /// endpoint failed, the quota ran out) leave it stored.
    pub async fn respond(&self, posted: Posted) -> Result<()> {
        let _turn = self.turns.lock().await;
        let ch = channel::get(&self.db(), posted.channel)?;
        if let Decision::Exhausted { release_at } = self.decide(CallKind::UserInitiated)? {
            self.exhausted_line(release_at)?;
            return Err(Error::QuotaExhausted { release_at });
        }
        if ch.is_host_channel() {
            self.host_turn(&posted).await
        } else if ch.kind == ChannelKind::Group {
            self.group_turn(&posted).await
        } else {
            let person = ch.person().cloned().ok_or_else(|| {
                Error::Invalid(format!("channel {} has no owner", posted.channel))
            })?;
            self.direct_turn(&posted, &person).await
        }
    }

    /// `post` then `respond`.
    pub async fn send(
        &self,
        channel: ChannelId,
        text: &str,
        image: Option<wire::Image>,
    ) -> Result<MessageId> {
        let posted = self.post(channel, text, image)?;
        let id = posted.message;
        self.respond(posted).await?;
        Ok(id)
    }

    pub(crate) fn decide(&self, kind: CallKind) -> Result<Decision> {
        let now = self.now();
        let conn = self.db();
        let tier = settings::intensity(&conn)?;
        Ok(quota::can_call(
            &conn,
            tier,
            kind,
            now.ms,
            &self.config.world.quota,
        )?)
    }

    /// The host's fixed line when the quota is used up: no model call.
    fn exhausted_line(&self, release_at: Option<i64>) -> Result<()> {
        let now = self.now();
        let names = self.names()?;
        let when = release_at
            .map(|ms| world::clock::local_time(now.at(ms)))
            .unwrap_or_default();
        let text = self
            .corpus
            .wording
            .fill("host.quota_exhausted", &names, &[("when", &when)]);
        let conn = self.db();
        let host = channel::host(&conn)?;
        let id = message::append(&conn, host, &Actor::Host, &text, None, None, now.ms)?;
        let stored = message::get(&conn, id)?;
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded {
            message: view::message(&stored, now.offset_min, &self.config.office),
        }));
        Ok(())
    }

    /// The user's entry: their words verbatim, then the harness's lines and material.
    fn user_entry(
        &self,
        seg: world::SegmentId,
        posted: &Posted,
        lines: &[String],
        turns: Option<u32>,
    ) -> Result<()> {
        let context = self.context_line(seg, turns)?.unwrap_or_default();
        let mut parts: Vec<&str> = vec![&posted.text];
        parts.extend(lines.iter().map(String::as_str));
        parts.push(&context);
        let text = assemble::entry(&parts);
        self.append_user(seg, &text, posted.image.as_ref())
    }

    async fn host_turn(&self, posted: &Posted) -> Result<()> {
        let seg = self
            .ensure_segment(posted.channel, Some(posted.message))
            .await?;
        self.user_entry(seg, posted, &[], None)?;
        let mut turn = Turn::new(posted.channel, Actor::Host, Shape::Host);
        turn.authorized = posted.authorized;
        self.run(&mut turn, self.config.agent.turn_loop.max_steps)
            .await?;
        for asked in std::mem::take(&mut turn.asks) {
            self.run_ask(asked.task, asked.channel).await?;
        }
        Ok(())
    }

    async fn direct_turn(&self, posted: &Posted, person: &PersonId) -> Result<()> {
        let now = self.now();
        {
            let conn = self.db();
            self.available(&conn, person)?;
            session::open(
                &conn,
                posted.channel,
                person,
                session::Budget::User,
                session::Cause::Addressed,
                now.ms,
            )?;
        }
        let seg = self
            .ensure_segment(posted.channel, Some(posted.message))
            .await?;
        let speaker = Actor::Person(person.clone());
        let mut turn = Turn::new(posted.channel, speaker.clone(), Shape::Direct);
        let lines = self.attachment(seg, &mut turn, &posted.text, Vec::new())?;
        self.user_entry(seg, posted, &lines, None)?;
        self.drive(&mut turn, self.config.agent.turn_loop.max_steps)
            .await?;
        self.replied(posted.channel, person)?;
        Ok(())
    }

    /// The retrieval attachment of a turn for its speaker.
    fn attachment(
        &self,
        seg: world::SegmentId,
        turn: &mut Turn,
        query: &str,
        persons: Vec<String>,
    ) -> Result<Vec<String>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut seen = self.seen(seg)?;
        let resp = self.retrieve(turn.channel, &turn.speaker, query, persons, &seen)?;
        let names = self.segment_names(seg)?;
        let offer_join = turn.speaker.person().is_some();
        let m = assemble::material(&self.ctx(&names), &resp, &mut seen, offer_join);
        turn.level = m.level;
        turn.shown.extend(m.shown);
        Ok(vec![m.text])
    }

    /// He replied: the session spends a reply and trust grows by a shared turn.
    fn replied(&self, ch: ChannelId, person: &PersonId) -> Result<()> {
        let now = self.now();
        let conn = self.db();
        session::replied(&conn, ch, person, now.ms)?;
        world::trust::shared_turn(&conn, ch, person, &self.config.world.trust)?;
        drop(conn);
        self.changed(vec![WorldChange::Trust {
            person: view::person_id(person),
        }]);
        Ok(())
    }

    /// Run a turn, handing over to a colleague it pulls in. In a request the caller
    /// resumes once the colleague has answered, within the request's turns; in an
    /// ordinary conversation the colleague's one answer ends the turn.
    pub(crate) async fn drive(&self, turn: &mut Turn, max_steps: u32) -> Result<()> {
        loop {
            self.run(turn, max_steps).await?;
            let Some(joined) = turn.joined.take() else {
                break;
            };
            self.join_turn(turn.channel, &joined, turn.shape).await?;
            let resume = match turn.task {
                Some(t) => world::task::get(&self.db(), t)?.status == world::TaskStatus::Active,
                None => false,
            };
            if !resume {
                break;
            }
            turn.text.clear();
            turn.message = None;
            turn.actions.clear();
        }
        self.close_open_task(turn)
    }

    /// A colleague pulled in answers. His persona enters the log as an entry the first
    /// time he speaks here, so the prefix is never rewritten.
    async fn join_turn(
        &self,
        ch: ChannelId,
        joined: &world::task::Joined,
        shape: Shape,
    ) -> Result<()> {
        let seg = self.current_segment(ch)?;
        let names = self.segment_names(seg)?;
        let name = self.display(joined.person.as_str())?;
        let mut lines = Vec::new();
        if !self.log_has_persona(seg, &names, &name)? {
            let persona = persona::person(self.ix().folio(), joined.person.as_str())?
                .ok_or_else(|| Error::NoPersona(joined.person.clone()))?;
            lines.push(assemble::persona_section(
                &self.ctx(&names),
                &Member::Person {
                    name: &name,
                    persona: &persona,
                },
            ));
        }
        lines.push(
            self.corpus
                .wording
                .fill("harness.joined", &names, &[("name", &name)]),
        );
        let speaker = Actor::Person(joined.person.clone());
        let mut turn = Turn::new(ch, speaker, shape);
        turn.task = joined.task;
        self.append_user(
            seg,
            &assemble::entry(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            None,
        )?;
        let steps = if joined.task.is_some() {
            joined.turns.max(1) + 1
        } else {
            self.config.agent.turn_loop.max_steps
        };
        // No further hand-over from a colleague: one join per turn.
        self.run(&mut turn, steps).await?;
        turn.joined = None;
        self.close_open_task(&mut turn)?;
        self.replied(ch, &joined.person)?;
        Ok(())
    }

    fn log_has_persona(&self, seg: world::SegmentId, names: &Names, name: &str) -> Result<bool> {
        let marker = assemble::persona_marker(&self.ctx(names), name);
        let items = self.items(seg)?;
        Ok(items
            .iter()
            .flat_map(|i| wire::texts(i))
            .any(|t| t.lines().any(|l| l == marker)))
    }

    async fn group_turn(&self, posted: &Posted) -> Result<()> {
        let now = self.now();
        let (members, named) = {
            let conn = self.db();
            let ch = channel::get(&conn, posted.channel)?;
            let members: Vec<PersonId> = self
                .corpus
                .personas
                .filter(bond::filter_disabled(&conn, ch.persons())?);
            drop(conn);
            let named = self.named_in(&posted.text, &members)?;
            (members, named)
        };
        if members.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = members.iter().map(|p| p.0.clone()).collect();
        let people = self.ix().rank_participants(&posted.text, &ids)?;
        let hits: Vec<PersonId> = people
            .persons
            .iter()
            .map(|m| PersonId::from(m.person.as_str()))
            .collect();
        let Some(addressed) = channel::address(&self.db(), posted.channel, named.as_ref(), &hits)?
        else {
            return Ok(());
        };
        session::open(
            &self.db(),
            posted.channel,
            &addressed,
            session::Budget::User,
            session::Cause::Addressed,
            now.ms,
        )?;
        let seg = self
            .ensure_segment(posted.channel, Some(posted.message))
            .await?;
        let names = self.segment_names(seg)?;
        let speaker = Actor::Person(addressed.clone());
        let mut turn = Turn::new(posted.channel, speaker, Shape::Group);
        let name = self.display(addressed.as_str())?;
        let mut lines =
            vec![
                self.corpus
                    .wording
                    .fill("harness.speaker", &names, &[("name", &name)]),
            ];
        lines.extend(self.attachment(seg, &mut turn, &posted.text, Vec::new())?);
        self.user_entry(seg, posted, &lines, None)?;
        self.drive(&mut turn, self.config.agent.turn_loop.max_steps)
            .await?;
        self.replied(posted.channel, &addressed)?;

        // At most one more line, from the enabled member with the best own material.
        let scored: Vec<(PersonId, f64)> = people
            .persons
            .iter()
            .map(|m| (PersonId::from(m.person.as_str()), m.score))
            .collect();
        let min = self
            .config
            .agent
            .interject
            .min_confidence
            .unwrap_or(self.ix().params().confidence.high_top1);
        let status = {
            let conn = self.db();
            let tier = settings::intensity(&conn)?;
            quota::status(&conn, tier, self.now().ms, &self.config.world.quota)?
        };
        let pick = channel::interjector(
            &self.db(),
            posted.channel,
            &addressed,
            &scored,
            min,
            &status,
        )?;
        if let Some(p) = pick.filter(|p| self.has_persona(p.as_str())) {
            self.interject(posted.channel, &p).await?;
        }
        Ok(())
    }

    /// The participant the user named: the longest display name or id in the text.
    fn named_in(&self, text: &str, members: &[PersonId]) -> Result<Option<PersonId>> {
        let mut best: Option<(usize, PersonId)> = None;
        for p in members {
            let display = self.display(p.as_str())?;
            for n in [display.as_str(), p.as_str()] {
                if !n.is_empty()
                    && text.contains(n)
                    && best.as_ref().is_none_or(|(l, _)| n.len() > *l)
                {
                    best = Some((n.len(), p.clone()));
                }
            }
        }
        Ok(best.map(|(_, p)| p))
    }

    async fn interject(&self, ch: ChannelId, person: &PersonId) -> Result<()> {
        let seg = self.current_segment(ch)?;
        let names = self.segment_names(seg)?;
        let name = self.display(person.as_str())?;
        let mut turn = Turn::new(ch, Actor::Person(person.clone()), Shape::Interject);
        // His own matching units: the reason he speaks.
        let last_user = message::history(&self.db(), ch, None, 8)?
            .into_iter()
            .rev()
            .find(|m| m.author == Actor::User)
            .map(|m| m.text)
            .unwrap_or_default();
        let mut lines =
            vec![
                self.corpus
                    .wording
                    .fill("harness.interject", &names, &[("name", &name)]),
            ];
        lines.extend(self.attachment(seg, &mut turn, &last_user, vec![person.0.clone()])?);
        self.append_user(
            seg,
            &assemble::entry(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            None,
        )?;
        self.run(&mut turn, 1).await?;
        self.replied(ch, person)?;
        Ok(())
    }

    // ---- channels and modes ----

    /// The direct channel with a person, created on first use. Nobody without a persona
    /// and nobody disabled gets one.
    pub fn open_direct(&self, person: &PersonId) -> Result<ChannelId> {
        let conn = self.db();
        self.available(&conn, person)?;
        Ok(channel::direct(&conn, person)?)
    }

    /// A group the user starts; every member must be someone who can talk.
    pub fn create_group(&self, members: &[PersonId], topic: Option<&str>) -> Result<ChannelId> {
        let conn = self.db();
        for m in members {
            self.available(&conn, m)?;
        }
        let id = channel::create_group(&conn, members, topic, world::Origin::User)?;
        drop(conn);
        self.changed(vec![WorldChange::Channel {
            id: view::channel_id(id),
        }]);
        Ok(id)
    }

    /// Change a person's or a group's mode, as the user. Enabling someone without a
    /// persona is refused.
    pub fn set_mode(&self, target: &api::types::Target, mode: api::types::Mode) -> Result<()> {
        let m = view::world_mode(mode);
        match target {
            api::types::Target::Person { id } => {
                self.set_person_mode(&PersonId(id.0.clone()), m)?;
            }
            api::types::Target::Channel { id } => {
                let ch =
                    id.0.parse::<i64>()
                        .map(ChannelId)
                        .map_err(|_| Error::Invalid(format!("channel `{}`", id.0)))?;
                channel::set_mode(&self.db(), ch, m)?;
            }
        }
        self.emit(Event::ModeChanged(api::events::ModeChanged {
            target: target.clone(),
            mode,
        }));
        Ok(())
    }

    /// Queue event seeds for an event that searched the roster (a conclusion written, a
    /// question pinned): the people whose own units match `text` best. Run
    /// [`Agent::on_event`] afterwards to let one of them speak.
    pub fn queue_event(&self, event: seed::Event, text: &str) -> Result<u32> {
        let offer = self.offerable(&self.db())?;
        let people = self
            .ix()
            .find_people(text, Some(&offer), self.config.find_people.k, &[])?;
        let hits: Vec<seed::Hit> = people
            .persons
            .iter()
            .filter(|m| m.level != index::Level::Low)
            .map(|m| seed::Hit {
                person: PersonId::from(m.person.as_str()),
                material: vec![m.reason.id.clone()],
            })
            .collect();
        Ok(seed::queue_hits(&self.db(), event, &hits, self.now().ms)?)
    }

    // ---- requests ----

    /// The user asks a person to look into a question. Returns the request at once; the
    /// work runs in [`Agent::run_request`].
    pub fn ask(
        &self,
        person: &PersonId,
        question: &str,
        turns: Option<u32>,
        parent: Option<TaskId>,
    ) -> Result<world::task::Asked> {
        let conn = self.db();
        self.available(&conn, person)?;
        let tier = settings::intensity(&conn)?;
        let asked = world::task::ask(
            &conn,
            person,
            question,
            turns,
            parent,
            tier,
            &self.config.world.ask,
            self.now().ms,
        )?;
        drop(conn);
        self.changed(vec![WorldChange::Task {
            id: view::task_id(asked.task),
        }]);
        Ok(asked)
    }

    /// Work on a request until it is answered or its turns run out.
    pub async fn run_request(&self, asked: world::task::Asked) -> Result<()> {
        let _turn = self.turns.lock().await;
        if let Decision::Exhausted { release_at } = self.decide(CallKind::UserInitiated)? {
            return Err(Error::QuotaExhausted { release_at });
        }
        self.run_ask(asked.task, asked.channel).await
    }

    async fn run_ask(&self, task: TaskId, ch: ChannelId) -> Result<()> {
        let t = world::task::get(&self.db(), task)?;
        let Some(person) = t.assignee.clone() else {
            return Ok(());
        };
        let seg = self.ensure_segment(ch, None).await?;
        let names = self.segment_names(seg)?;
        let name = self.display(person.as_str())?;
        let mut turn = Turn::new(ch, Actor::Person(person.clone()), Shape::Ask);
        turn.task = Some(task);
        let mut lines = vec![self.corpus.wording.fill(
            "harness.ask",
            &names,
            &[("name", &name), ("question", &t.question)],
        )];
        lines.extend(self.attachment(seg, &mut turn, &t.question, Vec::new())?);
        if let Some(c) = self.context_line(seg, Some(t.turns_left))? {
            lines.push(c);
        }
        self.append_user(
            seg,
            &assemble::entry(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            None,
        )?;
        self.drive(&mut turn, t.turns_left.saturating_add(1))
            .await?;
        // The request was its own conversation: wrap it up when nothing else is open here.
        let open = session::open_sessions(&self.db())?
            .iter()
            .any(|s| s.channel == ch);
        if !open {
            self.run_wrapup(ch, false).await?;
        }
        Ok(())
    }

    // ---- wrap-up ----

    /// The conversation in a channel ended: close its sessions and wrap it up.
    pub async fn wrapup(&self, ch: ChannelId) -> Result<()> {
        let _turn = self.turns.lock().await;
        session::close_channel(&self.db(), ch, self.now().ms)?;
        self.run_wrapup(ch, false).await?;
        Ok(())
    }

    /// The forced `wrapup` turn on the channel's current segment. Skipped when the model
    /// cannot be forced to a tool, when the log holds nothing to wrap up, when it was
    /// written for another wire API, and in the host's own log (it keeps no memories).
    /// With `summary` (a rollover) the summary for the next segment is asked for.
    pub(crate) async fn run_wrapup(&self, ch: ChannelId, summary: bool) -> Result<Option<Summary>> {
        let (role, _) = self.chat()?;
        if !role.forced_tool() || self.log_kind(&self.db(), ch)? == LogKind::Host {
            return Ok(None);
        }
        let Some(seg) = world::log::current(&self.db(), ch)? else {
            return Ok(None);
        };
        let empty = world::log::entries(&self.db(), seg.id)?.is_empty();
        if empty || self.segment_wire(seg.id)? != role.profile.wire_api {
            return Ok(None);
        }
        if !self.decide(CallKind::UserInitiated)?.allowed() {
            return Ok(None);
        }
        let Some(author) = self.wrapup_author(ch)? else {
            return Ok(None);
        };
        let names = self.segment_names(seg.id)?;
        let key = if summary {
            "harness.wrapup.summary"
        } else {
            "harness.wrapup"
        };
        let line = self.corpus.wording.fill(key, &names, &[]);
        self.append_user(seg.id, &line, None)?;
        let mut turn = Turn::new(ch, Actor::Person(author.clone()), Shape::Wrapup);
        turn.wrapup = true;
        let reply = self.call(&mut turn, Choice::Tool("wrapup")).await?;
        let wire = self.segment_wire(seg.id)?;
        for call in &reply.calls {
            let output = self.dispatch(&mut turn, call).await?;
            self.append(
                seg.id,
                Role::Tool,
                &wire::tool_result(wire, &call.id, &output),
            )?;
        }
        let Some(args) = turn.wrapped else {
            return Ok(None);
        };
        let outcome = world::wrapup::apply(
            &self.db(),
            ch,
            &Actor::Person(author.clone()),
            &args,
            self.now(),
            &self.config.world.wrapup,
            &self.config.world.trust,
        )?;
        let mut changes: Vec<WorldChange> = outcome
            .facts
            .iter()
            .chain(&outcome.hurt)
            .map(|f| WorldChange::Fact {
                id: view::fact_id(*f),
            })
            .collect();
        if outcome.hurt.is_some() {
            changes.push(WorldChange::Trust {
                person: view::person_id(&author),
            });
        }
        self.changed(changes);
        Ok(if summary { outcome.summary } else { None })
    }

    /// Whose session runs the wrap-up: the owner of a direct channel, the last person
    /// who spoke in a group.
    fn wrapup_author(&self, ch: ChannelId) -> Result<Option<PersonId>> {
        let conn = self.db();
        let c = channel::get(&conn, ch)?;
        if let Some(p) = c.person() {
            return Ok(Some(p.clone()).filter(|p| self.has_persona(p.as_str())));
        }
        let members = c.persons();
        for m in message::history(&conn, ch, None, 64)?.into_iter().rev() {
            if let Actor::Person(p) = m.author
                && members.contains(&p)
                && self.has_persona(p.as_str())
            {
                return Ok(Some(p));
            }
        }
        Ok(None)
    }

    // ---- rename ----

    /// The user's name changed: every channel with a log rolls over, so no segment keeps
    /// talking about the user under the old name. A log with nothing appended yet is
    /// simply replaced.
    pub async fn on_rename(&self) -> Result<()> {
        let _turn = self.turns.lock().await;
        let channels = crate::store::channels_with_segments(&self.db())?;
        for ch in channels {
            let Some(seg) = world::log::current(&self.db(), ch)? else {
                continue;
            };
            let names = self.names_now(&self.db())?;
            let stale = wire::Shape::parse(&seg.shape).is_none_or(|s| s.user != names.user());
            if !stale {
                continue;
            }
            if world::log::entries(&self.db(), seg.id)?.is_empty() {
                self.open_segment(ch, None, None)?;
            } else {
                self.roll(ch, None).await?;
            }
        }
        Ok(())
    }

    // ---- presence ----

    /// The user opened the app (`opened`) or came back from idle. Computes the digest,
    /// runs the calls `world` plans (wrap-ups of conversations that ended while idle,
    /// the host's one line, at most one opening) and returns the digest at once if the
    /// moment is not a presence occasion.
    pub async fn presence(&self, opened: bool) -> Result<PresenceOutcome> {
        let _turn = self.turns.lock().await;
        let now = self.now();
        if !world::presence::is_return(&self.db(), now.ms, opened, &self.config.world)? {
            return Ok(PresenceOutcome {
                digest: self.world_digest(now)?,
                host_spoke: false,
                opened: None,
            });
        }
        let words = self.words_hits(now)?;
        let birthdays = self.birthdays()?;
        let topic = TopicHits { agent: self };
        let planned = {
            let conn = self.db();
            let tier = settings::intensity(&conn)?;
            world::presence::on_presence(
                &conn,
                now,
                tier,
                &self.config.world,
                &birthdays,
                &words,
                &topic,
            )?
        };
        let has_endpoint = self.chat().is_ok();
        let mut out = PresenceOutcome {
            digest: planned.digest.clone(),
            host_spoke: false,
            opened: None,
        };
        if !has_endpoint {
            return Ok(out);
        }
        let introduce = opened && self.host_never_spoke()?;
        if introduce && self.decide(CallKind::HostLine)?.allowed() {
            out.host_spoke = self.host_harness(None).await?;
        }
        for call in planned.calls {
            match call {
                Call::Wrapup { channel } => {
                    self.run_wrapup(channel, false).await?;
                }
                Call::HostLine(d) if !introduce => {
                    out.host_spoke = self.host_harness(Some(&d)).await?;
                }
                Call::HostLine(_) => {}
                Call::Opening(o) => {
                    if self.opening(&o).await? {
                        out.opened = Some((o.person.clone(), o.channel));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Event seeds outside presence (a commitment fell due, a conclusion was written):
    /// at most one opening, within every limit.
    pub async fn on_event(&self) -> Result<Option<(PersonId, ChannelId)>> {
        let _turn = self.turns.lock().await;
        if self.chat().is_err() {
            return Ok(None);
        }
        let now = self.now();
        let topic = TopicHits { agent: self };
        let planned = {
            let conn = self.db();
            let tier = settings::intensity(&conn)?;
            let status = quota::status(&conn, tier, now.ms, &self.config.world.quota)?;
            let limits = seed::Limits {
                seed: &self.config.world.seed,
                session: &self.config.world.session,
                quota: &status,
            };
            seed::plan(&conn, &seed::Occasion::Event, now, &limits, &topic)?
        };
        if let seed::Planned::Open(o) = planned
            && self.opening(&o).await?
        {
            return Ok(Some((o.person, o.channel)));
        }
        Ok(None)
    }

    fn host_never_spoke(&self) -> Result<bool> {
        let conn = self.db();
        let host = channel::host(&conn)?;
        Ok(message::last(&conn, host)?.is_none())
    }

    /// The digest, disabled people and people without a persona left out.
    pub fn world_digest(&self, now: world::clock::Now) -> Result<Digest> {
        let birthdays = self.birthdays()?;
        let mut d = world::presence::digest(&self.db(), now, &birthdays)?;
        d.items.retain(|w| self.has_persona(w.person.as_str()));
        Ok(d)
    }

    fn birthdays(&self) -> Result<Vec<(PersonId, MonthDay)>> {
        Ok(self
            .ix()
            .folio()
            .persons()?
            .into_iter()
            .filter(|p| self.has_persona(&p.person_id))
            .filter_map(|p| {
                let day = MonthDay::parse(p.birthday.as_deref()?)?;
                Some((PersonId(p.person_id), day))
            })
            .collect())
    }

    /// The user's recent words that hit an enabled person's own units today.
    fn words_hits(&self, now: world::clock::Now) -> Result<Vec<WordsHit>> {
        let enabled = self.corpus.personas.filter(bond::enabled(&self.db())?);
        let since = now.local_day_start();
        let mut out = Vec::new();
        for p in enabled {
            let msgs = message::user_words_seen_by(&self.db(), &p, since, 3)?;
            for m in msgs {
                let people =
                    self.ix()
                        .find_people(&m.text, Some(std::slice::from_ref(&p.0)), 1, &[])?;
                if let Some(hit) = people
                    .persons
                    .first()
                    .filter(|h| h.level == index::Level::High)
                {
                    out.push(WordsHit {
                        person: p.clone(),
                        message: m.id,
                        material: vec![hit.reason.id.clone()],
                    });
                    break;
                }
            }
        }
        Ok(out)
    }

    /// A harness turn in the host's own log: the digest line, or the introduction on the
    /// very first open. One call, no tools, nothing the user asked for.
    async fn host_harness(&self, digest: Option<&Digest>) -> Result<bool> {
        let ch = channel::host(&self.db())?;
        let seg = self.ensure_segment(ch, None).await?;
        let names = self.segment_names(seg)?;
        let mut lines = vec![match digest {
            Some(d) => self.digest_text(d, &names)?,
            None => self.corpus.wording.fill("harness.introduce", &names, &[]),
        }];
        if let Some(c) = self.context_line(seg, None)? {
            lines.push(c);
        }
        self.append_user(
            seg,
            &assemble::entry(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            None,
        )?;
        let mut turn = Turn::new(ch, Actor::Host, Shape::Host);
        self.run(&mut turn, 1).await?;
        Ok(turn.message.is_some())
    }

    /// The digest as the host reads it.
    pub(crate) fn digest_text(&self, d: &Digest, names: &Names) -> Result<String> {
        let w = &self.corpus.wording;
        if d.is_empty() {
            return Ok(w.fill("harness.nothing_found", names, &[]));
        }
        let mut lines = vec![w.fill("harness.digest", names, &[])];
        for item in &d.items {
            let name = self.display(item.person.as_str())?;
            let n = [("name", name.as_str())];
            lines.push(match &item.activity {
                Activity::Messaged { .. } => w.fill("harness.digest.messaged", names, &n),
                Activity::Replied { question, .. } => w.fill(
                    "harness.digest.replied",
                    names,
                    &[("name", &name), ("question", question)],
                ),
                Activity::CommitmentDue { text, .. } => w.fill(
                    "harness.digest.commitment_due",
                    names,
                    &[("name", &name), ("text", text)],
                ),
                Activity::CameBy { .. } => w.fill("harness.digest.came_by", names, &n),
                Activity::Birthday => w.fill("harness.digest.birthday", names, &n),
            });
        }
        Ok(lines.join("\n"))
    }

    /// Run a planned opening. Returns whether he spoke (the seed is then used).
    async fn opening(&self, o: &Opening) -> Result<bool> {
        if !self.has_persona(o.person.as_str()) || !self.decide(CallKind::Opening)?.allowed() {
            return Ok(false);
        }
        let seg = self.ensure_segment(o.channel, None).await?;
        let names = self.segment_names(seg)?;
        let name = self.display(o.person.as_str())?;
        let w = &self.corpus.wording;
        let mut lines = vec![w.fill("harness.opening", &names, &[("name", &name)])];
        let fact_text = match o.fact {
            Some(f) if self.readers_may_see(o.channel, &o.person, f)? => {
                Some(fact::get(&self.db(), f)?.text)
            }
            _ => None,
        };
        let task_text = match &o.kind {
            SeedKind::Pinned(t) => Some(world::task::get(&self.db(), *t)?.question),
            SeedKind::Words(m) => Some(message::get(&self.db(), *m)?.text),
            _ => None,
        };
        let detail = |key: &str, text: Option<&String>| -> Option<String> {
            text.map(|t| w.fill(key, &names, &[("name", &name), ("text", t)]))
        };
        let line = match &o.kind {
            SeedKind::Commitment(_) => detail("harness.opening.commitment", fact_text.as_ref()),
            SeedKind::Conclusion(_) => detail("harness.opening.conclusion", fact_text.as_ref()),
            SeedKind::Memory(_) => detail("harness.opening.memory", fact_text.as_ref()),
            SeedKind::Pinned(_) => detail("harness.opening.pinned", task_text.as_ref()),
            SeedKind::Words(_) => detail("harness.opening.words", task_text.as_ref()),
            SeedKind::Birthday(_) => Some(w.fill("harness.opening.birthday", &names, &[])),
        };
        lines.extend(line);
        let units: Vec<folio::Unit> = {
            let ix = self.ix();
            o.material
                .iter()
                .filter_map(|id| ix.folio().unit_by_id(id).ok().flatten())
                .collect()
        };
        let mut turn = Turn::new(o.channel, Actor::Person(o.person.clone()), Shape::Opening);
        if !units.is_empty() {
            let mut seen: HashSet<String> = self.seen(seg)?;
            let m = assemble::units(&self.ctx(&names), &units, &mut seen);
            turn.shown.extend(m.shown);
            lines.push(m.text);
        }
        if let Some(c) = self.context_line(seg, None)? {
            lines.push(c);
        }
        self.append_user(
            seg,
            &assemble::entry(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            None,
        )?;
        self.run(&mut turn, self.config.agent.turn_loop.max_steps)
            .await?;
        let Some(msg) = turn.message else {
            return Ok(false);
        };
        let now = self.now();
        seed::delivered(&self.db(), o, msg, now.ms)?;
        if settings::notifications(&self.db())? {
            let text = message::get(&self.db(), msg)?.text;
            self.emit(Event::Notification(Notification {
                person: view::person_id(&o.person),
                channel: view::channel_id(o.channel),
                message: view::message_id(msg),
                text,
            }));
        }
        self.changed(vec![WorldChange::Digest]);
        Ok(true)
    }

    /// Whether everyone who reads the channel's log may recall a fact.
    fn readers_may_see(&self, ch: ChannelId, speaker: &PersonId, f: world::FactId) -> Result<bool> {
        let conn = self.db();
        Ok(fact::visible_to(&conn, Some(speaker), ch)?
            .iter()
            .any(|x| x.id == f))
    }

    /// A timestamp at the user's offset.
    pub fn timestamp(&self, ms: i64) -> api::types::Timestamp {
        timestamp(ms, self.now().offset_min)
    }

    /// The quota band now.
    pub fn band(&self) -> Result<Band> {
        let now = self.now();
        let conn = self.db();
        let tier = settings::intensity(&conn)?;
        Ok(quota::status(&conn, tier, now.ms, &self.config.world.quota)?.band)
    }
}

/// "Does a group's topic match a seed": the topic's retrieval shares a unit with the
/// seed's material.
struct TopicHits<'a> {
    agent: &'a Agent,
}

impl seed::TopicMatch for TopicHits<'_> {
    fn matches(&self, seed: &seed::Candidate, group: &channel::Channel) -> bool {
        let Some(topic) = group.topic.as_deref().filter(|t| !t.trim().is_empty()) else {
            return false;
        };
        if seed.material.is_empty() {
            return false;
        }
        let Ok(resp) = self.agent.ix().search(&index::SearchRequest::new(topic)) else {
            return false;
        };
        resp.hits
            .iter()
            .any(|h| seed.material.iter().any(|m| m == h.item.id()))
    }
}
