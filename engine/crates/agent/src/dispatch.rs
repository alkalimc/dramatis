//! Tool execution. Every constraint is enforced here, where the call runs, never by
//! varying what the model is offered: the harness fills `as_person` and audiences,
//! disabled people and people without a persona are never candidates, `report` needs an
//! active request, `wrapup` exists only in the harness's wrap-up turn, and the host's
//! mode and memory changes need the user's own turn.

use api::tools::{self, HostCall, SharedCall, TargetKind};
use api::views::{ChannelKind, ChannelRef, ToolAction};
use index::{Filters, SearchRequest};
use world::fact::{self, About, Audience, NewFact};
use world::wrapup::{Args, Hurt, Memory, MemoryKind};
use world::{Actor, ChannelId, FactId, FactKind, Origin, PersonId, bond, channel};

use crate::agent::{Agent, Event};
use crate::assemble;
use crate::error::{Error, Result};
use crate::session::{LogKind, Turn};
use crate::text::Names;
use crate::view;
use world::quota::CallKind;

/// Most conversations `list_channels` returns: enough to find one, few enough to offer
/// as buttons.
const LISTED_CHANNELS_MAX: usize = 12;

/// Most colleagues a person's group suggestion may name, apart from the person.
const SUGGESTED_GROUP_MAX: usize = 4;

/// Longest purpose a group proposal carries, in characters; more is cut.
const PURPOSE_MAX: usize = 60;

/// A purpose as one short line: the first line, trimmed, cut to [`PURPOSE_MAX`].
fn clean_purpose(purpose: Option<&str>) -> Option<String> {
    let line = purpose?.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(
        line.chars()
            .take(PURPOSE_MAX)
            .collect::<String>()
            .trim_end()
            .to_owned(),
    )
}

/// A refused call: its text goes back to the model as the tool's output.
struct Refusal(String);

type Outcome = std::result::Result<String, Refusal>;

impl Agent {
    /// Run one tool call and return what the model reads back.
    pub(crate) async fn dispatch(
        &self,
        turn: &mut Turn,
        name: &str,
        arguments: &str,
    ) -> Result<String> {
        let seg = self.current_segment(turn.channel)?;
        let names = self.segment_names(seg)?;
        let kind = self.log_kind(&self.db(), turn.channel)?;
        let out = match kind {
            LogKind::Person => match SharedCall::parse(name, arguments) {
                Ok(c) => self.shared(turn, c, &names, seg)?,
                Err(e) => Err(self.refuse(&names, "harness.error.arguments", &e.to_string())),
            },
            LogKind::Host => match HostCall::parse(name, arguments) {
                Ok(c) => self.host(turn, c, &names, seg)?,
                Err(e) => Err(self.refuse(&names, "harness.error.arguments", &e.to_string())),
            },
        };
        Ok(match out {
            Ok(s) | Err(Refusal(s)) => s,
        })
    }

    fn refuse(&self, names: &Names, key: &str, detail: &str) -> Refusal {
        Refusal(
            self.corpus
                .wording
                .fill(key, names, &[("detail", detail), ("id", detail)]),
        )
    }

    fn say(&self, names: &Names, key: &str, fields: &[(&str, &str)]) -> String {
        self.corpus.wording.fill(key, names, fields)
    }

    /// A world error the model caused (a wrong target, too many turns) is its tool
    /// output; anything else aborts the turn.
    fn world_refusal(&self, names: &Names, e: world::Error) -> Result<Refusal> {
        match e {
            world::Error::Sqlite(_) | world::Error::Json(_) | world::Error::Io(_) => {
                Err(Error::World(e))
            }
            world::Error::Disabled(_) | world::Error::NotFound { .. } => {
                Ok(self.refuse(names, "harness.error.unavailable", ""))
            }
            other => Ok(self.refuse(names, "harness.error.refused", &other.to_string())),
        }
    }

    /// People who may be offered to a model: persona holders who are not disabled.
    pub(crate) fn offerable(&self, conn: &rusqlite::Connection) -> Result<Vec<String>> {
        let off = bond::disabled(conn)?;
        Ok(self
            .corpus
            .personas
            .persons()
            .filter(|p| !off.iter().any(|d| d.as_str() == *p))
            .map(str::to_owned)
            .collect())
    }

    /// Err unless the person may be triggered: not disabled, and with a persona.
    pub(crate) fn available(&self, conn: &rusqlite::Connection, p: &PersonId) -> Result<()> {
        if !self.has_persona(p.as_str()) {
            return Err(Error::NoPersona(p.clone()));
        }
        bond::ensure_available(conn, p)?;
        Ok(())
    }

    /// Search as the speaker would: his knowledge, the memories he may see here, the
    /// session's ids set aside. The host searches as the maintainer face.
    pub(crate) fn retrieve(
        &self,
        channel: ChannelId,
        speaker: &Actor,
        query: &str,
        persons: Vec<String>,
        seen: &std::collections::HashSet<String>,
    ) -> Result<index::SearchResponse> {
        let (as_person, memories) = {
            let conn = self.db();
            let as_person = channel::as_person(&conn, channel, speaker)?;
            let memories = fact::visible_to(&conn, as_person.as_ref(), channel)?;
            (as_person, memories)
        };
        let ids: Vec<String> = memories.iter().map(assemble::memory_id).collect();
        let pairs: Vec<(&str, &str)> = ids
            .iter()
            .zip(&memories)
            .map(|(id, f)| (id.as_str(), f.text.as_str()))
            .collect();
        let mut exclude: Vec<String> = seen.iter().cloned().collect();
        exclude.sort();
        let req = SearchRequest {
            query: query.to_owned(),
            filters: Filters {
                persons,
                ..Filters::default()
            },
            as_person: as_person.map(|p| p.0),
            exclude,
            ..SearchRequest::default()
        };
        Ok(self.ix().search_with_memories(&req, &pairs)?)
    }

    fn shared(
        &self,
        turn: &mut Turn,
        call: SharedCall,
        names: &Names,
        seg: world::SegmentId,
    ) -> Result<Outcome> {
        let Some(me) = turn.speaker.person().cloned() else {
            return Ok(Err(self.refuse(names, "harness.error.unavailable", "")));
        };
        if turn.wrapup && !matches!(call, SharedCall::Wrapup(_)) {
            return Ok(Err(self.refuse(names, "harness.error.wrapup_only", "")));
        }
        match call {
            SharedCall::Search(s) => {
                self.show_action(
                    turn,
                    ToolAction::Search {
                        query: s.query.clone(),
                    },
                )?;
                let off = bond::disabled(&self.db())?;
                let persons = s
                    .persons
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|p| !off.iter().any(|d| d.as_str() == p))
                    .collect();
                let mut seen = self.seen(seg)?;
                let mut resp =
                    self.retrieve(turn.channel, &turn.speaker, &s.query, persons, &seen)?;
                drop_disabled(&mut resp, &off);
                let m = assemble::material(
                    &self.ctx(names),
                    &resp,
                    &mut seen,
                    self.config.agent.retrieve.low_hits as usize,
                );
                turn.level = m.level;
                turn.shown.extend(m.shown);
                Ok(Ok(m.text))
            }
            SharedCall::GetChunk(g) => {
                self.show_action(
                    turn,
                    ToolAction::GetChunk {
                        chunk_id: g.id.clone(),
                    },
                )?;
                let n = i64::from(g.neighbours.unwrap_or(0).min(3));
                let Some((unit, around)) = self.ix().get(&g.id, n)? else {
                    return Ok(Err(self.refuse(names, "harness.error.not_found", &g.id)));
                };
                let off = bond::disabled(&self.db())?;
                let gone = |u: &folio::Unit| {
                    !u.persons.is_empty()
                        && u.persons
                            .iter()
                            .all(|p| off.iter().any(|d| d.as_str() == p))
                };
                if gone(&unit) {
                    return Ok(Err(self.refuse(names, "harness.error.not_found", &g.id)));
                }
                let mut seen = self.seen(seg)?;
                let mut all: Vec<folio::Unit> = around.into_iter().filter(|u| !gone(u)).collect();
                let at = all
                    .iter()
                    .position(|u| u.span_from > unit.span_from)
                    .unwrap_or(all.len());
                all.insert(at, unit);
                let m = assemble::units(&self.ctx(names), &all, &mut seen);
                turn.shown.extend(m.shown);
                Ok(Ok(m.text))
            }
            SharedCall::RequestJoin(r) => self.request_join(turn, &me, r, names),
            SharedCall::AskHost(a) => self.ask_host(turn, &me, a, names),
            SharedCall::Remember(r) => {
                self.show_action(
                    turn,
                    ToolAction::Remember {
                        text: r.text.clone(),
                    },
                )?;
                let new = NewFact::new(
                    Actor::Person(me),
                    fact::channel_audience(turn.channel),
                    FactKind::Fact,
                    r.text.trim(),
                );
                if fact::normalize(&new.text).is_empty() {
                    return Ok(Err(self.refuse(names, "harness.error.refused", "empty")));
                }
                let written = fact::write(&self.db(), &new, self.now().ms)?;
                if let fact::Written::New(id) = written {
                    self.changed(vec![api::events::WorldChange::Fact {
                        id: view::fact_id(id),
                    }]);
                }
                Ok(Ok(self.say(names, "harness.remembered", &[])))
            }
            SharedCall::Report(r) => {
                // The request this turn runs on, or one he is on in this channel.
                let task = {
                    let conn = self.db();
                    let t = match turn.task {
                        Some(t) => Some(world::task::get(&conn, t)?),
                        None => world::task::active_for(&conn, &me, turn.channel)?,
                    };
                    t.filter(|t| {
                        t.assignee.as_ref() == Some(&me) && t.status == world::TaskStatus::Active
                    })
                };
                let Some(task) = task else {
                    // Nothing was asked of him: the text is what he meant to say. Refusing
                    // would cost the model another call to say it again.
                    if !turn.text.is_empty() {
                        turn.text.push('\n');
                    }
                    turn.text.push_str(&crate::assemble::visible_text(
                        &r.text,
                        (&self.corpus.scene.0, &self.corpus.scene.1),
                    ));
                    turn.closed = true;
                    return Ok(Ok(self.say(names, "harness.done", &[])));
                };
                self.show_action(turn, ToolAction::Report)?;
                let mut shown = turn.shown.clone();
                shown.extend(self.seen(seg)?);
                let mut cited = r
                    .cites
                    .iter()
                    .map(|c| format!("[#{c}]"))
                    .collect::<Vec<_>>()
                    .join(" ");
                cited.push(' ');
                cited.push_str(&r.text);
                let cites = self.citations(turn.channel, &cited, &shown)?;
                let reported = world::task::report(
                    &self.db(),
                    &me,
                    task.id,
                    &r.text,
                    &cites,
                    r.note.as_deref(),
                    &self.config.office,
                    self.now().ms,
                );
                match reported {
                    Ok(rep) => {
                        let note = rep
                            .note
                            .as_ref()
                            .map(|_| world::task::note_rel_path(task.id));
                        self.after_report(turn, task.id, rep.message, note)?;
                        Ok(Ok(self.say(names, "harness.done", &[])))
                    }
                    Err(e) => Ok(Err(self.world_refusal(names, e)?)),
                }
            }
            SharedCall::Wrapup(w) => {
                if !turn.wrapup {
                    return Ok(Err(self.refuse(names, "harness.error.wrapup_only", "")));
                }
                turn.wrapped = Some(wrapup_args(w));
                turn.closed = true;
                Ok(Ok(self.say(names, "harness.done", &[])))
            }
        }
    }

    fn request_join(
        &self,
        turn: &mut Turn,
        me: &PersonId,
        r: tools::RequestJoin,
        names: &Names,
    ) -> Result<Outcome> {
        let offer = self.offerable(&self.db())?;
        let here = channel::get(&self.db(), turn.channel)?.persons();
        let candidate = match (&r.person, &r.topic) {
            (Some(p), _) => Some(p.clone()).filter(|p| offer.contains(p)),
            (None, Some(topic)) => {
                let scope: Vec<String> = offer
                    .iter()
                    .filter(|p| !here.iter().any(|h| h.as_str() == p.as_str()))
                    .cloned()
                    .collect();
                self.ix()
                    .find_people(topic, Some(&scope), self.config.find_people.k, &[])?
                    .persons
                    .into_iter()
                    // Weak evidence is no reason to pull someone in.
                    .find(|m| m.level != index::Level::Low)
                    .map(|m| m.person)
            }
            (None, None) => None,
        };
        let person_ref = match &candidate {
            Some(p) => Some(view::person_ref(self.ix().folio(), p)?),
            None => None,
        };
        self.show_action(
            turn,
            ToolAction::RequestJoin {
                person: person_ref,
                topic: r.topic.clone(),
            },
        )?;
        let Some(candidate) = candidate.map(|p| PersonId::from(p.as_str())) else {
            return Ok(Err(self.refuse(names, "harness.error.unavailable", "")));
        };
        if turn.joined.is_some() || &candidate == me {
            return Ok(Err(self.refuse(names, "harness.error.unavailable", "")));
        }
        let joined = world::task::request_join(
            &self.db(),
            turn.channel,
            me,
            &candidate,
            r.turns,
            self.now().ms,
        );
        match joined {
            Ok(j) => {
                let name = self.display(candidate.as_str())?;
                turn.joined = Some(j);
                self.changed(vec![api::events::WorldChange::Channel {
                    id: view::channel_id(turn.channel),
                }]);
                Ok(Ok(self.say(names, "harness.joined", &[("name", &name)])))
            }
            Err(e) => Ok(Err(self.world_refusal(names, e)?)),
        }
    }

    /// A person asks the host to arrange something. Only a suggestion: it becomes an action
    /// line the front end offers as a choice, and nothing is created or opened until the
    /// user takes it. One per reply; colleagues from the roster only; the topic is the
    /// person's own wording, never the user's memories.
    fn ask_host(
        &self,
        turn: &mut Turn,
        me: &PersonId,
        a: tools::AskHost,
        names: &Names,
    ) -> Result<Outcome> {
        let unavailable = || self.refuse(names, "harness.error.unavailable", "");
        let spoken = turn.actions.iter().any(|x| {
            matches!(
                x,
                ToolAction::FindPeople { .. } | ToolAction::CreateGroup { .. }
            )
        });
        if spoken {
            return Ok(Err(self.refuse(names, "harness.error.once", "")));
        }
        // Unprompted offers stop in the quiet band, like unprompted speech.
        if !self.decide(CallKind::Opening)?.allowed() {
            return Ok(Err(unavailable()));
        }
        let offer = self.offerable(&self.db())?;
        match (a.find, a.group) {
            (Some(topic), None) => {
                let topic = topic.trim().to_owned();
                if topic.is_empty() {
                    return Ok(Err(unavailable()));
                }
                let scope: Vec<String> = offer.into_iter().filter(|p| p != me.as_str()).collect();
                let people =
                    self.ix()
                        .find_people(&topic, Some(&scope), self.config.find_people.k, &[])?;
                let found = people
                    .persons
                    .iter()
                    // Weak evidence is no reason to point the user at someone.
                    .filter(|m| m.level != index::Level::Low)
                    .map(|m| view::person_ref(self.ix().folio(), &m.person))
                    .collect::<Result<Vec<_>>>()?;
                if found.is_empty() {
                    return Ok(Err(unavailable()));
                }
                self.show_action(
                    turn,
                    ToolAction::FindPeople {
                        topic,
                        persons: found,
                    },
                )?;
            }
            (None, Some(group)) => {
                let mut members = vec![me.clone()];
                for id in group {
                    let id = PersonId::from(id.trim());
                    if !members.contains(&id) {
                        members.push(id);
                    }
                }
                let ok = members.len() >= 2
                    && members.len() <= SUGGESTED_GROUP_MAX + 1
                    && members
                        .iter()
                        .all(|m| offer.iter().any(|o| o == m.as_str()));
                if !ok {
                    return Ok(Err(unavailable()));
                }
                let refs = members
                    .iter()
                    .map(|m| view::person_ref(self.ix().folio(), m.as_str()))
                    .collect::<Result<Vec<_>>>()?;
                self.show_action(
                    turn,
                    ToolAction::CreateGroup {
                        members: refs,
                        channel: None,
                        purpose: clean_purpose(a.purpose.as_deref()),
                    },
                )?;
            }
            _ => {
                return Ok(Err(self.refuse(
                    names,
                    "harness.error.arguments",
                    "find or group",
                )));
            }
        }
        Ok(Ok(self.say(names, "harness.suggested", &[])))
    }

    /// A person's closing proposal line (where tools are emulated) takes the path of the
    /// `ask_host` call, with its limits; a refused one is dropped, since nothing answers
    /// the person. Names in a group line are the roster's display names, ids or aliases.
    pub(crate) fn propose(
        &self,
        turn: &mut Turn,
        find: Option<String>,
        group: Option<Vec<String>>,
        purpose: Option<String>,
    ) -> Result<()> {
        let Some(me) = turn.speaker.person().cloned() else {
            return Ok(());
        };
        let seg = self.current_segment(turn.channel)?;
        let names = self.segment_names(seg)?;
        let group = match group {
            Some(listed) => {
                let mut roster = Vec::new();
                for p in self.offerable(&self.db())? {
                    let display = self.display(&p)?;
                    roster.push((PersonId::from(p.as_str()), display));
                }
                let ix = self.ix();
                let by_alias = |n: &str| Ok(ix.folio().persons_for_alias(n)?);
                let mut ids = Vec::new();
                for n in &listed {
                    // A name nobody answers to stays as written, which refuses the group.
                    ids.push(match crate::address::member_named(n, &roster, by_alias)? {
                        Some(p) => p.as_str().to_owned(),
                        None => n.clone(),
                    });
                }
                Some(ids)
            }
            None => None,
        };
        // Two lines are two calls; the second meets the one-per-reply limit.
        for ask in [
            tools::AskHost {
                find,
                group: None,
                purpose: None,
            },
            tools::AskHost {
                find: None,
                group,
                purpose,
            },
        ] {
            if ask.find.is_some() || ask.group.is_some() {
                let _ = self.ask_host(turn, &me, ask, &names)?;
            }
        }
        Ok(())
    }

    fn host(
        &self,
        turn: &mut Turn,
        call: HostCall,
        names: &Names,
        seg: world::SegmentId,
    ) -> Result<Outcome> {
        let now = self.now();
        match call {
            HostCall::Digest(_) => {
                self.show_action(turn, ToolAction::Digest)?;
                let d = self.world_digest(now)?;
                Ok(Ok(self.digest_text(&d, names)?))
            }
            HostCall::FindPeople(f) => {
                let offer = self.offerable(&self.db())?;
                let people = self.ix().find_people(
                    &f.topic,
                    Some(&offer),
                    self.config.find_people.k,
                    &[],
                )?;
                let persons = people
                    .persons
                    .iter()
                    .map(|m| view::person_ref(self.ix().folio(), &m.person))
                    .collect::<Result<Vec<_>>>()?;
                self.show_action(
                    turn,
                    ToolAction::FindPeople {
                        topic: f.topic.clone(),
                        persons,
                    },
                )?;
                if people.persons.is_empty() {
                    return Ok(Ok(self.say(names, "harness.nothing_found", &[])));
                }
                let mut lines = Vec::new();
                for m in &people.persons {
                    let name = self.display(&m.person)?;
                    let reason = names
                        .sub(m.reason.text.lines().next().unwrap_or("").trim())
                        .into_owned();
                    lines.push(format!(
                        "- {name} ({}): {reason} [#{}]",
                        m.person, m.reason.id
                    ));
                }
                Ok(Ok(lines.join("\n")))
            }
            HostCall::Ask(a) => {
                let p = PersonId::from(a.person.as_str());
                if let Err(e) = self.available(&self.db(), &p) {
                    return match e {
                        Error::NoPersona(_) | Error::World(_) => {
                            Ok(Err(self.refuse(names, "harness.error.unavailable", "")))
                        }
                        e => Err(e),
                    };
                }
                let person = view::person_ref(self.ix().folio(), &a.person)?;
                self.show_action(
                    turn,
                    ToolAction::Ask {
                        person,
                        question: a.question.clone(),
                    },
                )?;
                let asked = {
                    let conn = self.db();
                    let tier = world::settings::intensity(&conn)?;
                    world::task::ask(
                        &conn,
                        &p,
                        &a.question,
                        a.turns,
                        None,
                        tier,
                        &self.config.world.ask,
                        now.ms,
                    )
                };
                match asked {
                    Ok(asked) => {
                        turn.asks.push(asked);
                        let name = self.display(p.as_str())?;
                        Ok(Ok(self.say(names, "harness.asked", &[("name", &name)])))
                    }
                    Err(e) => Ok(Err(self.world_refusal(names, e)?)),
                }
            }
            HostCall::CreateGroup(g) => {
                let members: Vec<PersonId> = g
                    .members
                    .iter()
                    .map(|m| PersonId::from(m.as_str()))
                    .collect();
                for m in &members {
                    if self.available(&self.db(), m).is_err() {
                        return Ok(Err(self.refuse(names, "harness.error.unavailable", "")));
                    }
                }
                let refs = members
                    .iter()
                    .map(|m| view::person_ref(self.ix().folio(), m.as_str()))
                    .collect::<Result<Vec<_>>>()?;
                let purpose = clean_purpose(g.purpose.as_deref());
                let created =
                    channel::create_group(&self.db(), &members, purpose.as_deref(), Origin::User);
                match created {
                    Ok(id) => {
                        if let Some(p) = &purpose {
                            self.group_opening(id, None, p)?;
                        }
                        self.show_action(
                            turn,
                            ToolAction::CreateGroup {
                                members: refs,
                                channel: Some(view::channel_id(id)),
                                purpose: purpose.clone(),
                            },
                        )?;
                        self.changed(vec![api::events::WorldChange::Channel {
                            id: view::channel_id(id),
                        }]);
                        Ok(Ok(self.say(
                            names,
                            "harness.group_created",
                            &[("id", &id.to_string())],
                        )))
                    }
                    Err(e) => Ok(Err(self.world_refusal(names, e)?)),
                }
            }
            HostCall::SetMode(s) => {
                if !turn.authorized {
                    return Ok(Err(self.refuse(names, "harness.error.not_asked", "")));
                }
                let mode = view::world_mode(s.mode);
                let target = match s.target_kind {
                    TargetKind::Person => {
                        let p = PersonId::from(s.target.as_str());
                        if let Err(e) = self.set_person_mode(&p, mode) {
                            return match e {
                                Error::World(w) => Ok(Err(self.world_refusal(names, w)?)),
                                Error::NoPersona(_) => {
                                    Ok(Err(self.refuse(names, "harness.error.unavailable", "")))
                                }
                                e => Err(e),
                            };
                        }
                        view::target_person(&p)
                    }
                    TargetKind::Group => {
                        let Ok(id) = s.target.trim().parse::<i64>().map(ChannelId) else {
                            return Ok(Err(self.refuse(
                                names,
                                "harness.error.not_found",
                                &s.target,
                            )));
                        };
                        if let Err(e) = channel::set_mode(&self.db(), id, mode) {
                            return Ok(Err(self.world_refusal(names, e)?));
                        }
                        view::target_channel(id)
                    }
                };
                self.show_action(
                    turn,
                    ToolAction::SetMode {
                        target: target.clone(),
                        mode: s.mode,
                    },
                )?;
                self.emit(Event::ModeChanged(api::events::ModeChanged {
                    target,
                    mode: s.mode,
                }));
                Ok(Ok(self.say(names, "harness.done", &[])))
            }
            HostCall::Search(s) => {
                self.show_action(
                    turn,
                    ToolAction::Search {
                        query: s.query.clone(),
                    },
                )?;
                let mut seen = self.seen(seg)?;
                let persons = s.persons.unwrap_or_default();
                let resp = self.retrieve(turn.channel, &Actor::Host, &s.query, persons, &seen)?;
                let m = assemble::material(
                    &self.ctx(names),
                    &resp,
                    &mut seen,
                    self.config.agent.retrieve.low_hits as usize,
                );
                turn.shown.extend(m.shown);
                Ok(Ok(m.text))
            }
            HostCall::Remember(r) => {
                if !turn.authorized {
                    return Ok(Err(self.refuse(names, "harness.error.not_asked", "")));
                }
                self.show_action(
                    turn,
                    ToolAction::Remember {
                        text: r.text.clone(),
                    },
                )?;
                // Written for the user, about the user, for everyone to recall.
                let new = NewFact::new(Actor::User, Audience::World, FactKind::Fact, r.text.trim())
                    .about(About::User);
                if fact::normalize(&new.text).is_empty() {
                    return Ok(Err(self.refuse(names, "harness.error.refused", "empty")));
                }
                let id = fact::write(&self.db(), &new, now.ms)?.id();
                self.changed(vec![api::events::WorldChange::Fact {
                    id: view::fact_id(id),
                }]);
                Ok(Ok(format!(
                    "{} [#{}]",
                    self.say(names, "harness.remembered", &[]),
                    assemble::memory_id(&fact::get(&self.db(), id)?)
                )))
            }
            HostCall::Forget(f) => {
                if !turn.authorized {
                    return Ok(Err(self.refuse(names, "harness.error.not_asked", "")));
                }
                let raw = api::cites::ids(&f.fact_id)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| f.fact_id.trim().to_owned());
                let Ok(id) = raw.trim_start_matches('m').parse::<i64>().map(FactId) else {
                    return Ok(Err(self.refuse(
                        names,
                        "harness.error.not_found",
                        &f.fact_id,
                    )));
                };
                if let Err(e) = fact::retract(&self.db(), id) {
                    return Ok(Err(self.world_refusal(names, e)?));
                }
                self.show_action(
                    turn,
                    ToolAction::Forget {
                        fact: view::fact_id(id),
                    },
                )?;
                self.changed(vec![api::events::WorldChange::Fact {
                    id: view::fact_id(id),
                }]);
                Ok(Ok(self.say(names, "harness.done", &[])))
            }
            HostCall::ListChannels(_) => {
                let (refs, lines) = self.channel_lines(names)?;
                self.show_action(turn, ToolAction::ListChannels { channels: refs })?;
                if lines.is_empty() {
                    return Ok(Ok(self.say(names, "harness.channels.none", &[])));
                }
                Ok(Ok(format!(
                    "{}\n{}",
                    self.say(names, "harness.channels", &[]),
                    lines.join("\n")
                )))
            }
        }
    }

    /// The user's conversations with something in them, newest first (the host's own
    /// channel left out): what the front ends offer and the lines the host reads.
    fn channel_lines(&self, names: &Names) -> Result<(Vec<ChannelRef>, Vec<String>)> {
        let now = self.now();
        let mut refs = Vec::new();
        let mut lines = Vec::new();
        // The connection lock is released before the loop takes it again.
        let all = channel::list(&self.db())?;
        for ch in all {
            if refs.len() >= LISTED_CHANNELS_MAX {
                break;
            }
            let Some(last) = world::message::last(&self.db(), ch.id)? else {
                continue;
            };
            if ch.is_host_channel() {
                continue;
            }
            let members = ch
                .persons()
                .iter()
                .map(|p| view::person_ref(self.ix().folio(), p.as_str()))
                .collect::<Result<Vec<_>>>()?;
            let (kind, key) = match ch.kind {
                world::ChannelKind::Direct => (ChannelKind::Direct, "harness.channels.direct"),
                world::ChannelKind::Group => (ChannelKind::Group, "harness.channels.group"),
            };
            let at = now.at(last.at);
            let d = world::clock::in_world_date(at, self.corpus.year_offset);
            let when = format!(
                "{:04}-{:02}-{:02} {}",
                d.year,
                d.month,
                d.day,
                world::clock::local_time(at)
            );
            let who = members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(self.say(names, key, &[("names", &who), ("when", &when)]));
            refs.push(ChannelRef {
                id: view::channel_id(ch.id),
                kind,
                members,
            });
        }
        Ok((refs, lines))
    }

    /// Change a person's mode. Enabling someone without a persona is refused: he could
    /// never be given a session.
    pub(crate) fn set_person_mode(&self, p: &PersonId, mode: world::Mode) -> Result<()> {
        if mode == world::Mode::Enabled && !self.has_persona(p.as_str()) {
            return Err(Error::NoPersona(p.clone()));
        }
        bond::set_mode(&self.db(), p, mode)?;
        Ok(())
    }
}

/// Search results never offer a disabled person's own units.
fn drop_disabled(resp: &mut index::SearchResponse, off: &[PersonId]) {
    if off.is_empty() {
        return;
    }
    let gone = |persons: &[String]| {
        !persons.is_empty()
            && persons
                .iter()
                .all(|p| off.iter().any(|d| d.as_str() == p.as_str()))
    };
    resp.hits
        .retain(|h| h.item.unit().is_none_or(|u| !gone(&u.persons)));
    for h in &mut resp.hits {
        h.neighbours.retain(|u| !gone(&u.persons));
    }
}

fn wrapup_args(w: tools::Wrapup) -> Args {
    Args {
        facts: w
            .facts
            .into_iter()
            .map(|f| Memory {
                kind: match f.kind {
                    tools::WrapupKind::Fact => MemoryKind::Fact,
                    tools::WrapupKind::Commitment => MemoryKind::Commitment,
                },
                text: f.text,
                due: f.due,
            })
            .collect(),
        hurt: w.hurt.map(|h| Hurt {
            person: h.person.map(|p| PersonId::from(p.as_str())),
            quote: h.quote,
        }),
        summary: w.summary,
    }
}
