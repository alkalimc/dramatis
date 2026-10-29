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

use std::collections::{HashSet, VecDeque};

use api::events::{MessageAdded, Notification, WorldChange};
use world::presence::{Activity, Digest};
use world::quota::{self, Band, CallKind, Decision};
use world::seed::{self, Opening, SeedKind, WordsHit};
use world::wrapup::Summary;
use world::{
    Actor, Call, ChannelId, ChannelKind, MessageId, MonthDay, PersonId, Shape, TaskId, bond,
    channel, fact, message, session, settings,
};

use crate::address::Audience;
use crate::agent::{Agent, Event};
use crate::assemble::{self, Member};
use crate::error::{Error, Result, timestamp};
use crate::persona;
use crate::picture;
use crate::request::{self, Image};
use crate::session::{LogKind, Turn};
use crate::text::Names;
use crate::view;
use crate::wrapup_json;
use llm::ToolChoice;

/// Most characters of the user's earlier message that join the automatic search's query.
const EARLIER_QUERY_CHARS: usize = 300;

/// A user message stored and waiting for its replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub channel: ChannelId,
    pub message: MessageId,
    pub text: String,
    pub image: Option<Image>,
    /// The user wrote this turn: the host may change modes and memories in it.
    pub authorized: bool,
}

/// A line a person was asked to post in a group: who, where, and the words.
type Said = Option<(PersonId, ChannelId, String)>;

/// Longest line a person posts in a group for the user, in characters.
pub(crate) const SAY_MAX: usize = 400;

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
    pub fn post(&self, channel: ChannelId, text: &str, image: Option<Image>) -> Result<Posted> {
        self.post_with(channel, text, image, &api::meta::MessageMeta::default())
    }

    /// Pass a quoted excerpt to someone: it is posted in his direct channel as the
    /// user's message, marked as a retelling. Run [`Agent::respond`] on the result.
    pub fn relay(&self, excerpt: &str, to: &PersonId) -> Result<Posted> {
        let ch = self.open_direct(to)?;
        let meta = api::meta::MessageMeta {
            relayed: true,
            ..api::meta::MessageMeta::default()
        };
        self.post_with(ch, excerpt, None, &meta)
    }

    fn post_with(
        &self,
        channel: ChannelId,
        text: &str,
        image: Option<Image>,
        meta: &api::meta::MessageMeta,
    ) -> Result<Posted> {
        if text.trim().is_empty() && image.is_none() {
            return Err(Error::Invalid("empty message".into()));
        }
        let now = self.now();
        // Before the world is locked: decoding a picture takes a moment.
        let kind = self.log_kind(&self.db(), channel)?;
        let image = image.map(|i| self.shrink(i, kind));
        let conn = self.db();
        let ch = channel::get(&conn, channel)?;
        if let Some(p) = ch.person() {
            self.available(&conn, p)?;
        }
        // The picture as shrunk goes into the save with the message: it is what the log
        // keeps too, and no cache or file outside the save can lose it.
        let id = world::atomic(&conn, |conn| {
            let mut meta = meta.clone();
            if let Some(i) = &image
                && let Some(bytes) = api::media::base64_decode(&i.data_base64)
            {
                meta.picture = Some(world::picture::put(conn, &i.mime, &bytes)?);
            }
            let attachment = (!meta.is_empty())
                .then(|| serde_json::to_value(&meta))
                .transpose()
                .map_err(|e| world::Error::Invalid(e.to_string()))?;
            message::append(
                conn,
                channel,
                &Actor::User,
                text,
                None,
                attachment.as_ref(),
                now.ms,
            )
        })?;
        session::touch_channel(&conn, channel, now.ms)?;
        let stored = message::get(&conn, id)?;
        let host = ch.is_host_channel();
        let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
        Ok(Posted {
            channel,
            message: id,
            text: text.to_owned(),
            image,
            authorized: host,
        })
    }

    /// The picture as the model's endpoint should get it (and as the log keeps it).
    fn shrink(&self, image: Image, kind: LogKind) -> Image {
        let max = self
            .role_for(kind)
            .ok()
            .and_then(|(role, _)| role.model.max_image_bytes);
        picture::prepare(image, &self.config.agent.picture, max)
    }

    /// Run the replies to a posted message. Errors after the message was stored (the
    /// endpoint failed, the quota ran out) leave it stored.
    pub async fn respond(&self, posted: Posted) -> Result<()> {
        let result = self.respond_inner(posted).await;
        if matches!(result, Err(Error::RateLimited { .. })) {
            // The app shows only events, so the host says it in its own channel.
            self.host_line("host.rate_limited", &[])?;
        }
        result
    }

    async fn respond_inner(&self, posted: Posted) -> Result<()> {
        let mut asks = Vec::new();
        let mut said = None;
        let first = {
            let _turn = self.lock_channel(posted.channel).await;
            // Taken back between the post and now: nothing to answer.
            if message::get(&self.db(), posted.message).is_err() {
                return Ok(());
            }
            let ch = channel::get(&self.db(), posted.channel)?;
            if let Decision::Exhausted { release_at } = self.decide(CallKind::UserInitiated)? {
                self.exhausted_line(release_at)?;
                return Err(Error::QuotaExhausted { release_at });
            }
            if ch.is_host_channel() {
                // Requests made before a failure are kept: they exist in the world and
                // must end in a state, not stay open with nothing running.
                let (made, result) = self.host_turn(&posted).await;
                asks = made;
                result
            } else if ch.kind == ChannelKind::Group {
                self.group_turn(&posted).await
            } else {
                let person = ch.person().cloned().ok_or_else(|| {
                    Error::Invalid(format!("channel {} has no owner", posted.channel))
                })?;
                self.direct_turn(&posted, &person).await.map(|s| said = s)
            }
        };
        // Requests the host made run in the asked person's channel, after the host's own
        // lock is released: no flow holds two channel locks. After a failed host turn
        // they are not run (the endpoint is likely still failing); they end as failed and
        // the host sees that on its next turn.
        let ran = self.run_asks(asks, first.is_err()).await;
        // The line a person was asked to post in a group goes out after the direct chat's
        // lock is released, for the same reason.
        let posted_say = match (&first, said) {
            (Ok(()), Some((person, group, words))) => {
                self.say_in_group(&person, group, &words).await
            }
            _ => Ok(()),
        };
        first.and(ran).and(posted_say)
    }

    /// Run the requests of one host turn in order. Every one ends in a state: after the
    /// first failure (or with `skip`) the rest are marked failed without a call.
    async fn run_asks(&self, asks: Vec<world::task::Asked>, skip: bool) -> Result<()> {
        let mut stop = skip;
        let mut first: Result<()> = Ok(());
        // Dropped mid-way (the work was cancelled), the requests not yet ended are marked.
        let mut guards: Vec<_> = asks.iter().map(|a| self.guard(a.task)).collect();
        guards.reverse();
        for asked in asks {
            let mut guard = guards.pop().expect("one guard per request");
            if stop {
                self.fail_request(asked.task, world::task::Failure::Failed);
                guard.disarm();
                continue;
            }
            let ran = {
                let _turn = self.lock_channel(asked.channel).await;
                self.run_ask(asked.task, asked.channel).await
            };
            guard.disarm();
            if let Err(e) = ran {
                self.fail_request(asked.task, world::task::Failure::Failed);
                stop = true;
                first = Err(e);
            }
        }
        first
    }

    /// A request ended without an answer: it stops being open, and the UI and the host
    /// see that. Does nothing to one that was answered.
    pub(crate) fn fail_request(&self, task: TaskId, why: world::task::Failure) {
        let changed = world::task::fail(&self.db(), task, why, self.now().ms);
        if matches!(changed, Ok(true)) {
            self.changed(vec![WorldChange::Task {
                id: view::task_id(task),
            }]);
        }
    }

    /// Take back the user's `message` and everything after it in `ch` ([`world::rewind`]),
    /// returning its text. Refused while a turn holds the channel.
    pub fn rewind(&self, ch: ChannelId, message: MessageId) -> Result<String> {
        let _turn = self.try_lock_channel(ch).ok_or(Error::Busy)?;
        // A context line is the harness's, not something the user wrote.
        let stored = message::get(&self.db(), message)?;
        if api::meta::MessageMeta::of(stored.attachment.as_ref()).service {
            return Err(Error::Invalid("not the user's own message".into()));
        }
        let out = world::rewind::rewind(&self.db(), ch, message)?;
        self.emit(Event::Rewound(api::events::Rewound {
            channel: crate::view::channel_id(ch),
            from: crate::view::message_id(message),
        }));
        let mut changes: Vec<WorldChange> = out
            .facts
            .iter()
            .map(|f| WorldChange::Fact {
                id: view::fact_id(*f),
            })
            .collect();
        changes.extend(out.tasks.iter().map(|t| WorldChange::Task {
            id: view::task_id(*t),
        }));
        changes.push(WorldChange::Digest);
        self.changed(changes);
        Ok(out.text)
    }

    /// `post` then `respond`.
    pub async fn send(
        &self,
        channel: ChannelId,
        text: &str,
        image: Option<Image>,
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
        let when = release_at
            .map(|ms| world::clock::local_time(self.now().at(ms)))
            .unwrap_or_default();
        self.host_line("host.quota_exhausted", &[("when", &when)])
    }

    /// A fixed line of the host's, from the corpus wording, shown in its channel.
    fn host_line(&self, key: &str, fields: &[(&str, &str)]) -> Result<()> {
        let now = self.now();
        let names = self.names()?;
        let text = self.corpus.wording.fill(key, &names, fields);
        let conn = self.db();
        let host = channel::host(&conn)?;
        let id = message::append(&conn, host, &Actor::Host, &text, None, None, now.ms)?;
        let stored = message::get(&conn, id)?;
        let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
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
        let before = message::history(&self.db(), posted.channel, Some(posted.message), 50)?;
        let since = before
            .iter()
            .rev()
            .find(|m| m.author == Actor::User)
            .map(|m| m.at);
        let context = self.context_line(seg, turns, since)?;
        let mut parts: Vec<&str> = vec![&posted.text];
        parts.extend(lines.iter().map(String::as_str));
        parts.push(&context);
        let text = assemble::entry(&parts);
        self.append_user(seg, &text, posted.image.as_ref())
    }

    /// Returns the requests the host made, even when the turn failed after making them;
    /// the caller runs them.
    async fn host_turn(&self, posted: &Posted) -> (Vec<world::task::Asked>, Result<()>) {
        let mut turn = Turn::new(posted.channel, Actor::Host, Shape::Host);
        turn.authorized = posted.authorized;
        let result = self.host_turn_run(posted, &mut turn).await;
        (std::mem::take(&mut turn.asks), result)
    }

    async fn host_turn_run(&self, posted: &Posted, turn: &mut Turn) -> Result<()> {
        let seg = self
            .ensure_segment(posted.channel, Some(posted.message))
            .await?;
        let (lines, told) = self.request_lines(seg)?;
        self.user_entry(seg, posted, &lines, None)?;
        for t in told {
            world::task::mark_told(&self.db(), t)?;
        }
        self.run(turn, self.config.agent.turn_loop.max_steps).await
    }

    /// One line per open request, and per request that ended without an answer (once),
    /// so the host answers about what is really happening. Also returns the failures
    /// shown, to be marked told once the entry is stored.
    fn request_lines(&self, seg: world::SegmentId) -> Result<(Vec<String>, Vec<TaskId>)> {
        let names = self.segment_names(seg)?;
        let (open, failed) = {
            let conn = self.db();
            (
                world::task::open_requests(&conn)?,
                world::task::untold_failures(&conn)?,
            )
        };
        let mut lines = Vec::new();
        let mut told = Vec::new();
        for t in open.iter().chain(&failed) {
            let Some(person) = &t.assignee else { continue };
            let name = self.display(person.as_str())?;
            let state = match t.failure {
                None => self.corpus.wording.fill(
                    "harness.task.active",
                    &names,
                    &[("turns", &t.turns_left.to_string())],
                ),
                Some(world::task::Failure::Failed) => {
                    self.corpus.wording.fill("harness.task.failed", &names, &[])
                }
                Some(world::task::Failure::Interrupted) => {
                    self.corpus
                        .wording
                        .fill("harness.task.interrupted", &names, &[])
                }
            };
            if t.failure.is_some() {
                told.push(t.id);
            }
            lines.push(self.corpus.wording.fill(
                "harness.task",
                &names,
                &[
                    ("id", &t.id.0.to_string()),
                    ("name", &name),
                    ("question", &t.question),
                    ("state", &state),
                ],
            ));
        }
        Ok((lines, told))
    }

    async fn direct_turn(&self, posted: &Posted, person: &PersonId) -> Result<Said> {
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
        let mut lines = Vec::new();
        // A colleague spoke here since the owner last did: say who answers now, or the
        // log's last voice carries on.
        if self.last_voice(posted)?.is_some_and(|p| &p != person) {
            let names = self.segment_names(seg)?;
            let name = self.display(person.as_str())?;
            lines.push(
                self.corpus
                    .wording
                    .fill("harness.speaker", &names, &[("name", &name)]),
            );
        }
        let query = self.retrieval_query(posted.channel, &posted.text, Some(posted.message))?;
        lines.extend(self.attachment(seg, &mut turn, &query, Vec::new())?);
        lines.extend(self.event_lines(seg, &mut turn, std::slice::from_ref(person))?);
        self.user_entry(seg, posted, &lines, None)?;
        self.drive(&mut turn, self.config.agent.turn_loop.max_steps)
            .await?;
        self.replied(posted.channel, person)?;
        Ok(turn
            .say
            .take()
            .map(|(group, words)| (person.clone(), group, words)))
    }

    /// The person who wrote the last person message before `posted` in its channel.
    fn last_voice(&self, posted: &Posted) -> Result<Option<PersonId>> {
        let before = message::history(&self.db(), posted.channel, Some(posted.message), 8)?;
        Ok(before.into_iter().rev().find_map(|m| match m.author {
            Actor::Person(p) => Some(p),
            _ => None,
        }))
    }

    /// What the automatic search looks for: the user's latest message with the user's one
    /// before it in this channel (`before` is the latest one's id), so a follow-up such as
    /// "and him?" still finds what the thread is about. The earlier message is cut short so
    /// it cannot outweigh the latest.
    pub(crate) fn retrieval_query(
        &self,
        ch: ChannelId,
        latest: &str,
        before: Option<MessageId>,
    ) -> Result<String> {
        let earlier = message::history(&self.db(), ch, before, 16)?
            .into_iter()
            .rev()
            .find(|m| {
                m.author == Actor::User
                    && !api::meta::MessageMeta::of(m.attachment.as_ref()).service
            })
            .map(|m| m.text.chars().take(EARLIER_QUERY_CHARS).collect::<String>());
        Ok(match earlier.as_deref().map(str::trim) {
            Some(e) if !e.is_empty() => format!("{e}\n{latest}"),
            _ => latest.to_owned(),
        })
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
        let m = assemble::material(
            &self.ctx(&names),
            &resp,
            &mut seen,
            self.config.agent.retrieve.low_hits as usize,
        );
        turn.level = m.level;
        turn.shown.extend(m.shown);
        turn.material = m.text.clone();
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
    /// time he speaks here, so the prefix is never rewritten. If the log already holds a
    /// memory he may not recall (the owner's, from rooms he was not in), it rolls over
    /// first: wrap-up on the old segment, then a new one with only shared material. No
    /// summary is asked for, so nothing private is retold into the new one.
    async fn join_turn(
        &self,
        ch: ChannelId,
        joined: &world::task::Joined,
        shape: Shape,
    ) -> Result<()> {
        let mut seg = self.current_segment(ch)?;
        if self.hides_from(seg, &joined.person)? {
            self.run_wrapup(ch, false).await?;
            seg = self.open_segment(ch, None, Some(crate::session::Tail { before: None }))?;
        }
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

    /// Whether a segment shows a memory `person` may not recall.
    fn hides_from(&self, seg: world::SegmentId, person: &PersonId) -> Result<bool> {
        let seen = self.seen(seg)?;
        let conn = self.db();
        for id in seen {
            // Memory ids are `m<fact id>`; unit ids are hex hashes and never start so.
            let Some(n) = id.strip_prefix('m').and_then(|r| r.parse::<i64>().ok()) else {
                continue;
            };
            if !fact::can_see(&conn, person, world::FactId(n))? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn log_has_persona(&self, seg: world::SegmentId, names: &Names, name: &str) -> Result<bool> {
        let marker = assemble::persona_marker(&self.ctx(names), name);
        let log = self.log(seg)?;
        let has = |t: &str| t.lines().any(|l| l == marker);
        Ok(log.system.iter().any(|t| has(t))
            || log.items.iter().flat_map(request::harness_texts).any(has))
    }

    /// The segment of a group a member is about to speak in. A group's log is shared, so
    /// someone added after it opened already reads everything said so far; what he lacks
    /// is his own persona, which enters as an entry. If the log shows a memory he may not
    /// recall, it rolls over first, as when a colleague is pulled into a talk.
    async fn admit(
        &self,
        ch: ChannelId,
        seg: world::SegmentId,
        person: &PersonId,
        before: Option<MessageId>,
    ) -> Result<world::SegmentId> {
        let name = self.display(person.as_str())?;
        let names = self.segment_names(seg)?;
        if self.log_has_persona(seg, &names, &name)? {
            return Ok(seg);
        }
        let mut seg = seg;
        if self.hides_from(seg, person)? {
            self.run_wrapup(ch, false).await?;
            seg = self.open_segment(ch, None, Some(crate::session::Tail { before }))?;
            // The new prefix was built with him among the members.
            let names = self.segment_names(seg)?;
            if self.log_has_persona(seg, &names, &name)? {
                return Ok(seg);
            }
        }
        self.introduce(seg, person)?;
        Ok(seg)
    }

    /// Append a member's persona to a segment as an entry.
    fn introduce(&self, seg: world::SegmentId, person: &PersonId) -> Result<()> {
        let names = self.segment_names(seg)?;
        let name = self.display(person.as_str())?;
        let persona = persona::person(self.ix().folio(), person.as_str())?
            .ok_or_else(|| Error::NoPersona(person.clone()))?;
        let section = assemble::persona_section(
            &self.ctx(&names),
            &Member::Person {
                name: &name,
                persona: &persona,
            },
        );
        self.append_user(seg, &assemble::entry(&[section.as_str()]), None)
    }

    /// People were added to a group or taken out of it (the roster already changed): each
    /// change is a service line in the group, and the group's log says so too, so the
    /// members know who is there. A newcomer's persona enters the log here when nothing
    /// forbids it, else when he first speaks. Fixed wording, no model call.
    pub async fn members_changed(
        &self,
        ch: ChannelId,
        joined: &[PersonId],
        left: &[PersonId],
    ) -> Result<()> {
        let _turn = self.lock_channel(ch).await;
        let seg = world::log::current(&self.db(), ch)?.map(|s| s.id);
        let names = match seg {
            Some(s) => self.segment_names(s)?,
            None => self.names_now(&self.db())?,
        };
        let mut said = Vec::new();
        for (who, key) in joined
            .iter()
            .map(|p| (p, "harness.joined"))
            .chain(left.iter().map(|p| (p, "harness.left")))
        {
            let name = self.display(who.as_str())?;
            said.push((
                who,
                key == "harness.joined",
                self.corpus.wording.fill(key, &names, &[("name", &name)]),
            ));
        }
        if let Some(seg) = seg {
            let mut lines = Vec::new();
            for (who, joining, text) in &said {
                if *joining {
                    let name = self.display(who.as_str())?;
                    let known = self.log_has_persona(seg, &names, &name)?;
                    if !known && !self.hides_from(seg, who)? {
                        self.introduce(seg, who)?;
                    }
                }
                lines.push(text.as_str());
            }
            self.append_user(seg, &assemble::entry(&lines), None)?;
        }
        for (_, _, text) in &said {
            self.service_line(ch, text)?;
        }
        self.changed(vec![WorldChange::Channel {
            id: view::channel_id(ch),
        }]);
        Ok(())
    }

    /// A line only the user sees, in the user's voice, marked as a service line.
    fn service_line(&self, ch: ChannelId, text: &str) -> Result<()> {
        let meta = api::meta::MessageMeta {
            service: true,
            ..api::meta::MessageMeta::default()
        };
        let attachment =
            serde_json::to_value(&meta).map_err(|e| world::Error::Invalid(e.to_string()))?;
        let now = self.now();
        let conn = self.db();
        let id = message::append(
            &conn,
            ch,
            &Actor::User,
            text,
            None,
            Some(&attachment),
            now.ms,
        )?;
        let stored = message::get(&conn, id)?;
        let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
        Ok(())
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
        let turns = self
            .speakers(posted, &members, named.as_ref(), &hits)
            .await?;
        let addressed = match turns.first() {
            Some(first) => first.clone(),
            None => {
                let Some(a) = channel::address(&self.db(), posted.channel, named.as_ref(), &hits)?
                else {
                    return Ok(());
                };
                a
            }
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
        let seg = self
            .admit(posted.channel, seg, &addressed, Some(posted.message))
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
        let query = self.retrieval_query(posted.channel, &posted.text, Some(posted.message))?;
        lines.extend(self.attachment(seg, &mut turn, &query, Vec::new())?);
        lines.extend(self.event_lines(seg, &mut turn, &members)?);
        self.user_entry(seg, posted, &lines, None)?;
        self.drive(&mut turn, self.config.agent.turn_loop.max_steps)
            .await?;
        self.replied(posted.channel, &addressed)?;

        // The round: the first speakers the address call chose, then whoever each reply
        // hands the turn to. It ends on an empty queue or the cap, and the user's next
        // message is the only thing that starts another.
        let handed = std::mem::take(&mut turn.hand_off);
        let mut spoken = vec![addressed.clone()];
        let queue = turns.iter().skip(1).cloned().collect();
        self.round(posted.channel, &mut spoken, queue, &handed)
            .await?;
        if spoken.len() > 1 {
            return Ok(());
        }

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
        let status = self.quota_status()?;
        let pick = channel::interjector(
            &self.db(),
            posted.channel,
            &addressed,
            &scored,
            min,
            &status,
        )?;
        if let Some(p) = pick.filter(|p| self.has_persona(p.as_str())) {
            // The ordinary interjection is the end of the turn: a hand-off line on it is
            // stripped like any other but starts nothing.
            self.interject(posted.channel, &p, "harness.interject")
                .await?;
        }
        Ok(())
    }

    /// Let the queued members answer in turn, each reply's hand-off line appending its
    /// named members after those already waiting. `spoken` holds everyone who replied to
    /// this user message so far: nobody speaks twice, and its length against
    /// `group.collective_max` bounds the round. Each extra reply passes the quota band.
    async fn round(
        &self,
        ch: ChannelId,
        spoken: &mut Vec<PersonId>,
        mut queue: VecDeque<PersonId>,
        first_hand_off: &[String],
    ) -> Result<()> {
        let cap = self.config.agent.group.collective_max as usize;
        let mut handed = self.hand_off_targets(ch, first_hand_off, spoken)?;
        loop {
            for p in handed {
                if !queue.contains(&p) {
                    queue.push_back(p);
                }
            }
            let Some(next) = queue.pop_front() else {
                break;
            };
            if spoken.len() >= cap {
                break;
            }
            if spoken.contains(&next) {
                handed = Vec::new();
                continue;
            }
            if !self.unprompted_allowed()? {
                break;
            }
            let names = self.interject(ch, &next, "harness.chorus").await?;
            spoken.push(next);
            handed = self.hand_off_targets(ch, &names, spoken)?;
        }
        Ok(())
    }

    fn quota_status(&self) -> Result<quota::Status> {
        let conn = self.db();
        let tier = settings::intensity(&conn)?;
        Ok(quota::status(
            &conn,
            tier,
            self.now().ms,
            &self.config.world.quota,
        )?)
    }

    /// Whether the quota band still lets extra turns through; the user's own turn
    /// always proceeds.
    fn unprompted_allowed(&self) -> Result<bool> {
        Ok(quota::decide(&self.quota_status()?, quota::CallKind::Interjection).allowed())
    }

    /// Whether the text holds one of the corpus's collective address words
    /// (`harness.collective`, whitespace separated): it speaks to the whole group.
    fn is_collective(&self, text: &str) -> bool {
        self.corpus
            .wording
            .get("harness.collective")
            .split_whitespace()
            .any(|w| text.contains(w))
    }

    /// Who replies, in order, when the message speaks to several members; empty when the
    /// plain rule (named, best hit, last speaker) applies. One member named and no
    /// collective word is the common case and costs no extra request, as does a
    /// collective word with nobody named. Otherwise the model is asked once who is
    /// addressed (unless `group.classify` is off, where a collective word alone widens
    /// it); an unusable answer means the plain rule.
    async fn speakers(
        &self,
        posted: &Posted,
        members: &[PersonId],
        named: Option<&PersonId>,
        hits: &[PersonId],
    ) -> Result<Vec<PersonId>> {
        let group = &self.config.agent.group;
        let candidates: Vec<PersonId> = members
            .iter()
            .filter(|p| self.has_persona(p.as_str()))
            .cloned()
            .collect();
        if candidates.len() < 2 || group.collective_max < 2 {
            return Ok(Vec::new());
        }
        let mut many_named = 0;
        for p in &candidates {
            many_named += usize::from(self.is_named(&posted.text, p)?);
        }
        let collective = self.is_collective(&posted.text);
        let audience = if many_named == 1 && !collective {
            None
        } else if collective && (many_named == 0 || !group.classify) {
            Some(Audience::Everyone)
        } else if group.classify {
            self.audience(posted.channel, posted.message, &posted.text, &candidates)
                .await?
        } else {
            None
        };
        let set = match audience {
            Some(Audience::Everyone) => candidates,
            Some(Audience::Some(ids)) => ids,
            None => return Ok(Vec::new()),
        };
        let first = named.or_else(|| hits.iter().find(|h| set.contains(h)));
        Ok(channel::round(
            &self.db(),
            posted.channel,
            first,
            &set,
            group.collective_max,
        )?)
    }

    /// Whether the text names this person, by display name or id.
    fn is_named(&self, text: &str, person: &PersonId) -> Result<bool> {
        let display = self.display(person.as_str())?;
        Ok([display.as_str(), person.as_str()]
            .iter()
            .any(|n| !n.is_empty() && text.contains(n)))
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

    async fn interject(&self, ch: ChannelId, person: &PersonId, key: &str) -> Result<Vec<String>> {
        let seg = self.current_segment(ch)?;
        let seg = self.admit(ch, seg, person, None).await?;
        let names = self.segment_names(seg)?;
        let name = self.display(person.as_str())?;
        let mut turn = Turn::new(ch, Actor::Person(person.clone()), Shape::Interject);
        // His own matching units: the reason he speaks.
        let last_user = message::history(&self.db(), ch, None, 8)?
            .into_iter()
            .rev()
            .find(|m| m.author == Actor::User);
        let last_user = match last_user {
            Some(m) => self.retrieval_query(ch, &m.text, Some(m.id))?,
            None => String::new(),
        };
        let mut lines = vec![self.corpus.wording.fill(key, &names, &[("name", &name)])];
        lines.extend(self.attachment(seg, &mut turn, &last_user, vec![person.0.clone()])?);
        lines.push(self.context_line(seg, None, None)?);
        self.append_user(
            seg,
            &assemble::entry(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            None,
        )?;
        self.run(&mut turn, 1).await?;
        self.replied(ch, person)?;
        Ok(std::mem::take(&mut turn.hand_off))
    }

    /// A person's line, asked for by the user in a direct chat, posted in a group he is
    /// in. It is his message there, and the others answer as they would a hand-off:
    /// each reply passes the quota band, and `group.collective_max` bounds the round.
    /// Those the line names answer first.
    async fn say_in_group(&self, from: &PersonId, group: ChannelId, words: &str) -> Result<()> {
        let _turn = self.lock_channel(group).await;
        let (members, id) = {
            let conn = self.db();
            let ch = channel::get(&conn, group)?;
            if ch.kind != ChannelKind::Group
                || ch.deleted_at.is_some()
                || ch.mode == world::Mode::Disabled
                || !ch.persons().contains(from)
            {
                return Ok(());
            }
            let now = self.now();
            let id = message::append(
                &conn,
                group,
                &Actor::Person(from.clone()),
                words,
                None,
                None,
                now.ms,
            )?;
            session::touch_channel(&conn, group, now.ms)?;
            let stored = message::get(&conn, id)?;
            let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
            drop(conn);
            self.emit(Event::MessageAdded(MessageAdded { message: shown }));
            let members: Vec<PersonId> = self.corpus.personas.filter(bond::filter_disabled(
                &self.db(),
                ch.persons().into_iter().filter(|p| p != from).collect(),
            )?);
            (members, id)
        };
        let seg = self.ensure_segment(group, Some(id)).await?;
        let seg = self.admit(group, seg, from, Some(id)).await?;
        let names = self.segment_names(seg)?;
        let name = self.display(from.as_str())?;
        let line =
            self.corpus
                .wording
                .fill("harness.said", &names, &[("name", &name), ("text", words)]);
        self.append_user(seg, &line, None)?;
        let mut queue = VecDeque::new();
        for named in [true, false] {
            for p in &members {
                if self.is_named(words, p)? == named && !queue.contains(p) {
                    queue.push_back(p.clone());
                }
            }
        }
        let mut spoken = vec![from.clone()];
        self.round(group, &mut spoken, queue, &[]).await
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

    /// The first line of a group made for a purpose: the host says why it exists and who
    /// suggested it (`by`, or the host itself). Fixed wording, no model call.
    pub fn group_opening(&self, ch: ChannelId, by: Option<&PersonId>, purpose: &str) -> Result<()> {
        let name = self.proposer(by)?;
        let names = self.names()?;
        let text = self.corpus.wording.fill(
            "harness.group_purpose",
            &names,
            &[("name", &name), ("purpose", purpose)],
        );
        let now = self.now();
        let conn = self.db();
        channel::add_participant(&conn, ch, &Actor::Host)?;
        let id = message::append(&conn, ch, &Actor::Host, &text, None, None, now.ms)?;
        let stored = message::get(&conn, id)?;
        let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
        Ok(())
    }

    /// The user took a person found for `topic`: the person's log gets the reason, so they
    /// know why they were found, and the user sees the same line as a service line. Nothing
    /// is asked of the model; the person answers when the user writes.
    pub async fn found_opening(
        &self,
        ch: ChannelId,
        by: Option<&PersonId>,
        topic: &str,
    ) -> Result<()> {
        // No subject (a named person was offered): there is no reason to pass on.
        if topic.trim().is_empty() {
            return Ok(());
        }
        let name = self.proposer(by)?;
        let seg = self.ensure_segment(ch, None).await?;
        let names = self.segment_names(seg)?;
        let text = self.corpus.wording.fill(
            "harness.found_for",
            &names,
            &[("name", &name), ("topic", topic)],
        );
        let said = message::history(&self.db(), ch, None, 50)?
            .iter()
            .any(|m| m.author == Actor::User && m.text == text);
        if said {
            return Ok(());
        }
        let meta = api::meta::MessageMeta {
            service: true,
            ..api::meta::MessageMeta::default()
        };
        let now = self.now();
        self.append_user(seg, &text, None)?;
        let conn = self.db();
        let attachment =
            serde_json::to_value(&meta).map_err(|e| world::Error::Invalid(e.to_string()))?;
        let id = message::append(
            &conn,
            ch,
            &Actor::User,
            &text,
            None,
            Some(&attachment),
            now.ms,
        )?;
        let stored = message::get(&conn, id)?;
        let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
        Ok(())
    }

    /// Who suggested something, by name: a person, or the host when `by` is none.
    fn proposer(&self, by: Option<&PersonId>) -> Result<String> {
        match by {
            Some(p) => self.display(p.as_str()),
            None => Ok(self.corpus.host_name.clone()),
        }
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
        // Cancelled (the future dropped) before it ended, the request is marked interrupted.
        let mut guard = self.guard(asked.task);
        let ran = async {
            let _turn = self.lock_channel(asked.channel).await;
            if let Decision::Exhausted { release_at } = self.decide(CallKind::UserInitiated)? {
                return Err(Error::QuotaExhausted { release_at });
            }
            self.run_ask(asked.task, asked.channel).await
        }
        .await;
        guard.disarm();
        if ran.is_err() {
            self.fail_request(asked.task, world::task::Failure::Failed);
        }
        ran
    }

    fn guard(&self, task: TaskId) -> OpenRequest<'_> {
        OpenRequest {
            agent: self,
            task,
            armed: true,
        }
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
        lines.push(self.context_line(seg, Some(t.turns_left), None)?);
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
        let _turn = self.lock_channel(ch).await;
        session::close_channel(&self.db(), ch, self.now().ms)?;
        self.run_wrapup(ch, false).await?;
        Ok(())
    }

    /// The `wrapup` turn on the channel's current segment, forced through `tool_choice`
    /// where the protocol allows it and the model honours it; on an endpoint that only
    /// emulates tools, asked as a JSON document instead (see `Agent::call`); where the protocol must
    /// keep `tool_choice` unchanged, the harness line alone asks for the call, and a
    /// reply without it counts as no wrap-up. Skipped when the model cannot be forced,
    /// when the log holds nothing to wrap up, and in the host's own log (it keeps no
    /// memories). With `summary` (a rollover) the summary for the next segment is asked.
    pub(crate) async fn run_wrapup(&self, ch: ChannelId, summary: bool) -> Result<Option<Summary>> {
        let kind = self.log_kind(&self.db(), ch)?;
        if kind == LogKind::Host {
            return Ok(None);
        }
        let (role, _) = self.role_for(kind)?;
        // An emulated endpoint is asked for structured output, not forced to a call.
        let emulated = role.model.emulated_tools();
        let forced = !emulated && role.profile.wire_api.may_force_tool();
        if forced && !role.forced_tool() {
            return Ok(None);
        }
        let Some(seg) = world::log::current(&self.db(), ch)? else {
            return Ok(None);
        };
        if world::log::entries(&self.db(), seg.id)?.is_empty() {
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
        let mut line = self.corpus.wording.fill(key, &names, &[]);
        if emulated {
            // The endpoint may not enforce the response schema, so the request text
            // shows the shape itself, built from the schema.
            let tools = api::tools::shared_tools(&self.corpus.descriptions);
            if let Some(t) = tools.iter().find(|t| t.name == "wrapup") {
                let shape = wrapup_json::example(&t.parameters);
                let shape =
                    self.corpus
                        .wording
                        .fill("harness.wrapup.shape", &names, &[("shape", &shape)]);
                line = format!("{line}\n\n{shape}");
            }
        }
        self.append_user(seg.id, &line, None)?;
        let mut turn = Turn::new(ch, Actor::Person(author.clone()), Shape::Wrapup);
        turn.wrapup = true;
        let choice = if forced {
            ToolChoice::Tool("wrapup".into())
        } else {
            ToolChoice::Auto
        };
        let reply = self.call(&mut turn, choice).await?;
        let mut results = Vec::new();
        for (id, name, args) in reply.tool_calls() {
            let output = self.dispatch(&mut turn, name, args).await?;
            results.push(request::tool_result_part(id, output));
        }
        if !results.is_empty() {
            self.append(seg.id, &request::tool_results(results))?;
        }
        let Some(mut args) = turn.wrapped else {
            return Ok(None);
        };
        wrapup_json::keep_undated(&mut args, self.now());
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

    /// The user's name or bio changed: every channel with a log rolls over, so no segment
    /// keeps talking about the user under the old name or words. A log with nothing appended yet is
    /// simply replaced.
    pub async fn on_rename(&self) -> Result<()> {
        let mut channels: Vec<_> = world::channel::list(&self.db())?
            .into_iter()
            .filter(|c| c.log_segment.is_some())
            .map(|c| c.id)
            .collect();
        channels.sort();
        for ch in channels {
            // One channel at a time: each roll waits for that channel's turn only.
            let _turn = self.lock_channel(ch).await;
            let Some(seg) = world::log::current(&self.db(), ch)? else {
                continue;
            };
            let names = self.names_now(&self.db())?;
            let stale = request::Shape::parse(&seg.shape).is_none_or(|s| !s.knows(&names));
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
        let now = self.now();
        // The very first open: the host's pre-written welcome, no model call, so it is
        // instant, free and the same with or without an endpoint.
        if opened && self.host_never_spoke()? {
            self.host_line(&format!("host.intro.{}", self.config.frontend.key()), &[])?;
            return Ok(PresenceOutcome {
                digest: self.world_digest(now)?,
                host_spoke: true,
                opened: None,
            });
        }
        let not_a_return = |me: &Self| -> Result<Option<PresenceOutcome>> {
            if world::presence::is_return(&me.db(), now.ms, opened, &me.config.world)? {
                return Ok(None);
            }
            Ok(Some(PresenceOutcome {
                digest: me.world_digest(now)?,
                host_spoke: false,
                opened: None,
            }))
        };
        // The digest alone never waits behind a model call.
        if let Some(out) = not_a_return(self)? {
            return Ok(out);
        }
        let _plan = self.planner.lock().await;
        if let Some(out) = not_a_return(self)? {
            return Ok(out);
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
            digest: self.persona_digest(planned.digest.clone()),
            host_spoke: false,
            opened: None,
        };
        if !has_endpoint {
            return Ok(out);
        }
        for call in planned.calls {
            match call {
                Call::Wrapup { channel } => {
                    self.wrapup_ended(channel).await?;
                }
                Call::HostLine(d) => {
                    let d = self.persona_digest(d);
                    if !d.is_empty() {
                        out.host_spoke = self.host_harness(&d).await?;
                        if out.host_spoke {
                            let conn = self.db();
                            world::presence::mark_announced(
                                &conn,
                                &planned.digest,
                                &d,
                                now.local_date(),
                            )?;
                        }
                    }
                }
                Call::Opening(o) => {
                    if self.opening(&o).await? {
                        out.opened = Some((o.person.clone(), o.channel));
                    }
                }
            }
        }
        // The user is here: today's happening on the ship, if one is due.
        if self.ship_event().await? {
            out.host_spoke = true;
        }
        Ok(out)
    }

    /// Event seeds outside presence (a commitment fell due, a conclusion was written):
    /// at most one opening, within every limit.
    pub async fn on_event(&self) -> Result<Option<(PersonId, ChannelId)>> {
        let _plan = self.planner.lock().await;
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
        let d = world::presence::digest(&self.db(), now, &birthdays)?;
        Ok(self.persona_digest(d))
    }

    fn persona_digest(&self, mut d: Digest) -> Digest {
        d.items.retain(|w| self.has_persona(w.person.as_str()));
        d
    }

    /// The periodic step while the app runs: conversations idle past
    /// `session.idle_timeout` are wrapped up, then due event seeds (a commitment) may
    /// open. With the user `present` (the window shown, the terminal in use) the day's
    /// ship event may be made. Call it every minute or so; it makes no call when nothing
    /// is due.
    pub async fn tick(&self, present: bool) -> Result<Option<(PersonId, ChannelId)>> {
        let ended = {
            let now = self.now().ms;
            let conn = self.db();
            // A channel with a flow in progress is not idle, however long the flow takes.
            for ch in self.busy_channels() {
                session::touch_channel(&conn, ch, now)?;
                world::task::touch_channel(&conn, ch, now)?;
            }
            let ended = session::expire_idle(&conn, now, &self.config.world.session)?;
            // A request whose process is gone (a crash, or another process that quit).
            let cut = world::task::interrupt_open(&conn, now)?;
            drop(conn);
            self.changed(
                cut.into_iter()
                    .map(|t| WorldChange::Task {
                        id: view::task_id(t),
                    })
                    .collect(),
            );
            ended
        };
        if self.chat().is_ok() {
            for ch in &ended {
                self.wrapup_ended(*ch).await?;
            }
        }
        let opened = self.on_event().await?;
        if present && self.chat().is_ok() {
            let _plan = self.planner.lock().await;
            self.ship_event().await?;
        }
        if !ended.is_empty() {
            self.changed(vec![WorldChange::Digest]);
        }
        Ok(opened)
    }

    /// Wrap up a conversation that ended on its own (idle). Its channel may have been
    /// written to since it was found ended: then the conversation is alive again and
    /// the wrap-up is left for its own end.
    async fn wrapup_ended(&self, ch: ChannelId) -> Result<()> {
        let _turn = self.lock_channel(ch).await;
        if session::open_sessions(&self.db())?
            .iter()
            .any(|s| s.channel == ch)
        {
            return Ok(());
        }
        self.run_wrapup(ch, false).await?;
        Ok(())
    }

    pub(crate) fn birthdays(&self) -> Result<Vec<(PersonId, MonthDay)>> {
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

    /// A harness turn in the host's own log: the digest line. One call, no tools, nothing
    /// the user asked for.
    async fn host_harness(&self, digest: &Digest) -> Result<bool> {
        let ch = channel::host(&self.db())?;
        let _turn = self.lock_channel(ch).await;
        let seg = self.ensure_segment(ch, None).await?;
        let names = self.segment_names(seg)?;
        let mut lines = vec![self.digest_text(digest, &names)?];
        lines.push(self.context_line(seg, None, None)?);
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
            return Ok(w.fill("harness.digest.none", names, &[]));
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
        // A channel in the middle of a turn is not interrupted: the seed stays queued and
        // is planned again at the next occasion.
        let Some(_turn) = self.try_lock_channel(o.channel) else {
            return Ok(false);
        };
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
            SeedKind::Happening(_) => detail("harness.opening.event", fact_text.as_ref()),
        };
        lines.extend(line);
        let units: Vec<folio::Unit> = {
            let ix = self.ix();
            let mut found = Vec::new();
            for id in &o.material {
                found.extend(ix.folio().unit_by_id(id)?);
            }
            found
        };
        let mut turn = Turn::new(o.channel, Actor::Person(o.person.clone()), Shape::Opening);
        if !units.is_empty() {
            let mut seen: HashSet<String> = self.seen(seg)?;
            let m = assemble::units(&self.ctx(&names), &units, &mut seen);
            turn.shown.extend(m.shown);
            lines.push(m.text);
        }
        lines.push(self.context_line(seg, None, None)?);
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

/// A request being worked on. If dropped while armed, the work was cancelled before the
/// request ended, so it is marked interrupted instead of staying open for good.
pub(crate) struct OpenRequest<'a> {
    agent: &'a Agent,
    task: TaskId,
    armed: bool,
}

impl OpenRequest<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OpenRequest<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.agent
                .fail_request(self.task, world::task::Failure::Interrupted);
        }
    }
}
