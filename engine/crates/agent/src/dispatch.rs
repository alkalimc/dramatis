//! Tool execution. Every constraint is enforced here, where the call runs, never by
//! varying what the model is offered: the harness fills `as_person` and audiences,
//! disabled people and people without a persona are never candidates, `report` needs an
//! active request, `wrapup` exists only in the harness's wrap-up turn, and the host's
//! mode and memory changes need the user's own turn.

use api::tools::{self, HostCall, SharedCall, TargetKind};
use api::views::ToolAction;
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
use crate::wire::ToolCall;

/// A refused call: its text goes back to the model as the tool's output.
struct Refusal(String);

type Outcome = std::result::Result<String, Refusal>;

impl Agent {
    /// Run one tool call and return what the model reads back.
    pub(crate) async fn dispatch(&self, turn: &mut Turn, call: &ToolCall) -> Result<String> {
        let seg = self.current_segment(turn.channel)?;
        let names = self.segment_names(seg)?;
        let kind = self.log_kind(&self.db(), turn.channel)?;
        let out = match kind {
            LogKind::Person => match SharedCall::parse(&call.name, &call.arguments) {
                Ok(c) => self.shared(turn, c, &names, seg)?,
                Err(e) => Err(self.refuse(&names, "harness.error.arguments", &e.to_string())),
            },
            LogKind::Host => match HostCall::parse(&call.name, &call.arguments) {
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
                let m = assemble::material(&self.ctx(names), &resp, &mut seen, true);
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
                let mut seen = self.seen(seg)?;
                let mut all = around;
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
                if let fact::Written::New(id) = fact::write(&self.db(), &new, self.now().ms)? {
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
                    return Ok(Err(self.refuse(names, "harness.error.no_task", "")));
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
                let cites = self.citations(&cited, &shown)?;
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
                    .next()
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
                self.show_action(
                    turn,
                    ToolAction::FindPeople {
                        topic: f.topic.clone(),
                    },
                )?;
                let offer = self.offerable(&self.db())?;
                let people = self.ix().find_people(
                    &f.topic,
                    Some(&offer),
                    self.config.find_people.k,
                    &[],
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
                self.show_action(turn, ToolAction::CreateGroup { members: refs })?;
                match channel::create_group(&self.db(), &members, g.topic.as_deref(), Origin::User)
                {
                    Ok(id) => {
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
                let m = assemble::material(&self.ctx(names), &resp, &mut seen, false);
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
                let raw = f
                    .fact_id
                    .trim()
                    .trim_start_matches("[#")
                    .trim_end_matches(']');
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
        }
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
