//! Daily events: the host posts a small happening of the day in its own channel, with
//! offers to join a gathering or talk to someone it involves. One structured request on
//! the host role per event, drawn from a corpus unit so it fits the world; a reply that
//! cannot be read costs the call and nothing else.
//!
//! The people it involves learn of it through a world fact that ends with the day: it
//! reaches the opening block of any segment opened that day, and the first turn of a
//! segment already open when it happened. A seed lets one of them bring it up.

use llm::{Message, Request, ToolChoice, ToolSpec};
use serde_json::{Value, json};
use world::event::{self, Event as DailyEvent};
use world::fact::{self, Audience, NewFact};
use world::quota::{self, CallKind};
use world::{Actor, FactKind, PersonId, channel, message, settings};

use crate::agent::{Agent, Event};
use crate::assemble;
use crate::diag;
use crate::dispatch::clean_purpose;
use crate::error::Result;
use crate::session::{LogKind, Turn};
use crate::text::{Names, Wording};
use crate::view;
use crate::wrapup_json;
use api::events::{MessageAdded, WorldChange};
use api::views::ToolAction;

/// Longest title and text kept, in characters; more is cut.
const TITLE_MAX: usize = 60;
const TEXT_MAX: usize = 600;
/// Most people a suggested gathering brings together (the host's offer to the user).
const GATHERING_MAX: usize = 5;
/// How many of the day's people the material is drawn from.
const MATERIAL_PEOPLE: usize = 2;

/// Up to `n` of `items`, drawn at random (std's per-instance random hash keys; no ordering
/// survives between calls).
fn random_pick<T>(items: Vec<T>, n: usize) -> Vec<T> {
    use std::hash::{BuildHasher, RandomState};
    let state = RandomState::new();
    let mut keyed: Vec<(u64, T)> = items
        .into_iter()
        .enumerate()
        .map(|(i, t)| (state.hash_one(i), t))
        .collect();
    keyed.sort_by_key(|(k, _)| *k);
    keyed.into_iter().take(n).map(|(_, t)| t).collect()
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "description": "a short title"},
            "text": {"type": "string", "description": "two or three short sentences"},
            "persons": {
                "type": "array",
                "items": {"type": "string"},
                "description": "ids of the people it involves"
            },
            "group": {
                "anyOf": [
                    {"type": "null"},
                    {
                        "type": "object",
                        "properties": {
                            "members": {"type": "array", "items": {"type": "string"},
                                "description": "ids of the people who would gather"},
                            "purpose": {"type": "string", "description": "why they gather, a few words"}
                        },
                        "required": ["members", "purpose"],
                        "additionalProperties": false
                    }
                ],
                "description": "a gathering the user could join, or null"
            },
            "talk": {
                "type": "array",
                "items": {"type": "string"},
                "description": "ids of people the user might talk to about it"
            }
        },
        "required": ["title", "text", "persons", "group", "talk"],
        "additionalProperties": false
    })
}

#[derive(Debug)]
struct Draft {
    title: String,
    text: String,
    persons: Vec<String>,
    group: Option<DraftGroup>,
    talk: Vec<String>,
}

#[derive(Debug)]
struct DraftGroup {
    members: Vec<String>,
    purpose: Option<String>,
}

/// Why a reply made no event, for the diagnostics log.
#[derive(Debug, PartialEq, Eq)]
enum Unreadable {
    /// No text at all.
    Empty,
    /// Text, but no JSON object in it.
    NoJson,
    /// A JSON object without a title and text.
    NoTitleOrText,
}

impl Unreadable {
    fn reason(&self) -> &'static str {
        match self {
            Self::Empty => "empty reply",
            Self::NoJson => "reply is not JSON",
            Self::NoTitleOrText => "reply has no title or text",
        }
    }
}

/// The JSON object in a reply: the text itself, a fenced block, or the span from the
/// first `{` to the last `}` (a line of prose before or after it).
fn object(reply: &str) -> Option<Value> {
    let text = reply.trim();
    let whole = wrapup_json::unfence(text);
    let fenced = text
        .split_once("```")
        .and_then(|(_, rest)| rest.split_once('\n'))
        .and_then(|(_, rest)| rest.split_once("```"))
        .map(|(inner, _)| inner.trim());
    let span = text
        .find('{')
        .zip(text.rfind('}'))
        .map(|(i, j)| &text[i..=j.max(i)]);
    [Some(whole), fenced, span]
        .into_iter()
        .flatten()
        .find_map(|c| {
            serde_json::from_str::<Value>(c)
                .ok()
                .filter(Value::is_object)
        })
}

/// Names a model listed: strings, or objects that carry an `id` or `name`.
fn names(v: Option<&Value>) -> Vec<String> {
    let one = |x: &Value| match x {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => ["id", "name"]
            .iter()
            .find_map(|k| o.get(*k).and_then(Value::as_str))
            .map(str::to_owned),
        _ => None,
    };
    match v {
        Some(Value::Array(items)) => items.iter().filter_map(one).collect(),
        Some(single) => one(single).into_iter().collect(),
        None => Vec::new(),
    }
}

/// The words a corpus labels the parts of an event with, for a reply written as labelled
/// prose instead of JSON. Each part has alternatives, matched as prefixes: a label may run
/// on ("People involved (ids)") before its colon.
#[derive(Debug, Clone, Default)]
pub(crate) struct Labels {
    title: Vec<String>,
    text: Vec<String>,
    persons: Vec<String>,
    group: Vec<String>,
    members: Vec<String>,
    purpose: Vec<String>,
    talk: Vec<String>,
}

impl Labels {
    pub(crate) fn new(w: &Wording, names: &Names) -> Self {
        let get = |key: &str| {
            w.get(key)
                .split('|')
                .map(|l| w.fill_text(l.trim(), names, &[]).to_ascii_lowercase())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        };
        Self {
            title: get("events.label.title"),
            text: get("events.label.text"),
            persons: get("events.label.persons"),
            group: get("events.label.group"),
            members: get("events.label.members"),
            purpose: get("events.label.purpose"),
            talk: get("events.label.talk"),
        }
    }
}

/// How far after a label its colon may be, in characters.
const LABEL_RUN: usize = 24;

/// Where a label stands in `lower` (the text, ASCII-lowercased), and where its body starts:
/// after the colon that ends the label, on the same line. The first label that has one.
fn label_at(text_lower: &str, labels: &[String]) -> Option<(usize, usize)> {
    labels
        .iter()
        .flat_map(|l| {
            text_lower
                .match_indices(l.as_str())
                .filter_map(move |(i, _)| {
                    let after = i + l.len();
                    let run: String = text_lower[after..]
                        .chars()
                        .take_while(|c| *c != '\n')
                        .take(LABEL_RUN)
                        .collect();
                    let colon = run.find([':', '\u{ff1a}'])?;
                    let width = run[colon..].chars().next().map_or(1, char::len_utf8);
                    Some((i, after + colon + width))
                })
        })
        .min()
}

/// The bodies of the labelled parts of `text`, in the order of `parts`; a body runs to
/// the next label of any of them.
fn bodies<'a>(text: &'a str, parts: &[&[String]]) -> Vec<Option<&'a str>> {
    let lower = text.to_ascii_lowercase();
    let found: Vec<Option<(usize, usize)>> = parts.iter().map(|l| label_at(&lower, l)).collect();
    found
        .iter()
        .map(|f| {
            let (_, start) = (*f)?;
            let end = found
                .iter()
                .flatten()
                .map(|(i, _)| *i)
                .filter(|i| *i >= start)
                .min()
                .unwrap_or(text.len());
            Some(text[start..end].trim())
        })
        .collect()
}

fn listed(body: Option<&str>) -> Vec<String> {
    body.map(|b| assemble::split_names(b.trim_end_matches(['.', '\u{3002}', ' '])))
        .unwrap_or_default()
}

/// The event in a reply written as labelled prose, the last thing tried: a title and a
/// text, then the people, a gathering and the people to talk to when they are there.
fn parse_prose(reply: &str, labels: &Labels) -> Option<Draft> {
    let [title, text, persons, group, talk] = bodies(
        reply,
        &[
            &labels.title,
            &labels.text,
            &labels.persons,
            &labels.group,
            &labels.talk,
        ],
    )[..] else {
        return None;
    };
    let (title, text) = (
        title.filter(|t| !t.is_empty())?,
        text.filter(|t| !t.is_empty())?,
    );
    let group = group.filter(|g| !g.is_empty()).map(|g| {
        match bodies(g, &[&labels.members, &labels.purpose])[..] {
            [None, None] => DraftGroup {
                members: listed(Some(g)),
                purpose: None,
            },
            [members, purpose] => DraftGroup {
                members: listed(members),
                purpose: purpose
                    .filter(|p| !p.is_empty())
                    .map(|p| p.trim_end_matches(['\u{ff1b}', ';']).trim().to_owned()),
            },
            _ => unreachable!("two parts, two bodies"),
        }
    });
    Some(Draft {
        title: title.to_owned(),
        text: text.to_owned(),
        persons: listed(persons),
        group,
        talk: listed(talk),
    })
}

/// The event in a reply: a JSON object (the arguments of the `make_event` call are one),
/// else labelled prose. Extra fields are ignored and optional ones may be missing; only a
/// title and a text are needed.
fn parse(reply: &str, labels: &Labels) -> std::result::Result<Draft, Unreadable> {
    if reply.trim().is_empty() {
        return Err(Unreadable::Empty);
    }
    let Some(doc) = object(reply) else {
        return parse_prose(reply, labels).ok_or(Unreadable::NoJson);
    };
    let field = |k: &str| doc.get(k).and_then(Value::as_str).map(str::trim);
    let (Some(title), Some(text)) = (field("title"), field("text")) else {
        return Err(Unreadable::NoTitleOrText);
    };
    if title.is_empty() || text.is_empty() {
        return Err(Unreadable::NoTitleOrText);
    }
    let group = doc
        .get("group")
        .filter(|g| g.is_object())
        .map(|g| DraftGroup {
            members: names(g.get("members")),
            purpose: g.get("purpose").and_then(Value::as_str).map(str::to_owned),
        });
    Ok(Draft {
        title: title.to_owned(),
        text: text.to_owned(),
        persons: names(doc.get("persons")),
        group,
        talk: names(doc.get("talk")),
    })
}

/// What the model wrote: its text, or, when an endpoint put the answer in a thinking
/// block and left the text empty, the thinking.
fn answer(reply: &llm::Response) -> String {
    let text = reply.text();
    if !text.trim().is_empty() {
        return text;
    }
    reply
        .parts
        .iter()
        .filter_map(|p| match p {
            llm::Part::Native(v) if v["type"] == "thinking" => v["thinking"].as_str(),
            _ => None,
        })
        .collect()
}

fn cut(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect::<String>()
}

impl Agent {
    /// The ended facts go. Cheap enough to run wherever facts are read for a log.
    pub(crate) fn sweep(&self) {
        let _ = fact::sweep_expired(&self.db(), self.now().ms);
    }

    /// Make today's event if one is due. The caller holds the planner lock. Anything that
    /// goes wrong, a refused call or a reply that cannot be read, ends in `false` and no
    /// message; the next try waits `events.retry_min`.
    pub(crate) async fn daily_event(&self) -> Result<bool> {
        self.sweep();
        let now = self.now();
        let events = &self.config.world.events;
        if self.chat().is_err() {
            return Ok(false);
        }
        {
            let conn = self.db();
            let tier = settings::intensity(&conn)?;
            let status = quota::status(&conn, tier, now.ms, &self.config.world.quota)?;
            if !event::due(&conn, now, &self.config.world, &status)? {
                return Ok(false);
            }
            let tried: Option<i64> = settings::get(&conn, settings::EVENTS_TRIED)?;
            let wait = i64::from(events.retry_min) * world::clock::MINUTE_MS;
            if tried.is_some_and(|t| now.ms - t < wait) {
                return Ok(false);
            }
            settings::set(&conn, settings::EVENTS_TRIED, &now.ms)?;
        }
        if !self.decide(CallKind::Opening)?.allowed() {
            return Ok(false);
        }
        let Some((material, request)) = self.event_request()? else {
            return Ok(false);
        };
        let Some(draft) = self.event_draft(request).await? else {
            return Ok(false);
        };
        self.post_event(draft, &material)
    }

    /// The ids of the units the request draws on, and the request.
    fn event_request(&self) -> Result<Option<(Vec<String>, Request)>> {
        let now = self.now();
        let names = self.names()?;
        let w = &self.corpus.wording;
        let offerable = {
            let mut all = self.offerable(&self.db())?;
            all.sort();
            all
        };
        if offerable.is_empty() {
            return Ok(None);
        }
        let recent = event::recent(&self.db(), self.config.world.events.list)?;
        let used: Vec<String> = recent.iter().flat_map(|e| e.material.clone()).collect();
        let group_topics: Vec<String> = channel::list(&self.db())?
            .into_iter()
            .filter(|c| c.kind == world::ChannelKind::Group)
            .filter_map(|c| c.topic)
            .filter(|t| !t.trim().is_empty())
            .take(2)
            .collect();
        // Who it may involve: a fresh random handful each time, leaving out whoever the
        // recent events were about while there are others to choose from. What happens is
        // the model's to choose; only the direction is given.
        let pool_max = (self.config.world.events.pool as usize).max(1);
        let lately: std::collections::HashSet<&str> = recent
            .iter()
            .flat_map(|e| e.persons.iter().map(PersonId::as_str))
            .collect();
        let mut candidates: Vec<String> = offerable
            .iter()
            .filter(|p| !lately.contains(p.as_str()))
            .cloned()
            .collect();
        if candidates.len() < pool_max {
            candidates = offerable.clone();
        }
        let pool = random_pick(candidates, pool_max);
        // The material is a few of their own units of the kinds the corpus lists, picked at
        // random, and never a search over the whole story: the event is a small moment,
        // and the model should not retell what it reads.
        let kinds: Vec<String> = w
            .get("events.material")
            .split('|')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        let mut units: Vec<folio::Unit> = Vec::new();
        for person in pool.iter().take(MATERIAL_PEOPLE) {
            let mut req = index::SearchRequest::new(&self.display(person)?);
            req.filters.templates = kinds.clone();
            req.filters.persons = vec![person.clone()];
            req.exclude = used
                .iter()
                .cloned()
                .chain(units.iter().map(|u| u.id.clone()))
                .collect();
            let Ok(resp) = self.ix().search(&req) else {
                continue;
            };
            let hits: Vec<folio::Unit> = resp
                .hits
                .iter()
                .filter(|h| !req.exclude.iter().any(|x| x == h.item.id()))
                .filter_map(|h| h.item.unit().cloned())
                .collect();
            units.extend(random_pick(hits, 1));
        }
        let material = if units.is_empty() {
            String::new()
        } else {
            let mut seen = std::collections::HashSet::new();
            assemble::units(&self.ctx(&names), &units, &mut seen).text
        };
        let mut people = Vec::new();
        for p in &pool {
            people.push(format!("{p}: {}", self.display(p)?));
        }
        let d = world::clock::in_world_date(now, self.corpus.year_offset);
        let date = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
        let (role, _) = self.role_for(LogKind::Host)?;
        let schema = schema();
        let mut extra = Vec::new();
        if !recent.is_empty() {
            let told: Vec<String> = recent
                .iter()
                .map(|e| {
                    let line = w.fill(
                        "events.fact",
                        &names,
                        &[("title", &e.title), ("text", &e.text)],
                    );
                    format!("- {line}")
                })
                .collect();
            extra.push(w.fill("harness.event.past", &names, &[("past", &told.join("\n"))]));
        }
        if !group_topics.is_empty() {
            extra.push(w.fill(
                "harness.event.recent",
                &names,
                &[("recent", &group_topics.join("\n"))],
            ));
        }
        let mut text = w.fill(
            "harness.event.ask",
            &names,
            &[
                ("date", &date),
                ("material", &material),
                ("people", &people.join("\n")),
                ("extra", &extra.join("\n")),
            ],
        );
        let mut request = Request {
            messages: Vec::new(),
            ..Request::default()
        };
        if role.model.emulated_tools() {
            // The endpoint may not enforce the response schema, so an emulating one is
            // shown the shape in words.
            let shape = wrapup_json::example(&schema);
            text = format!(
                "{text}\n\n{}",
                w.fill("harness.wrapup.shape", &names, &[("shape", &shape)])
            );
            request.output_schema = Some(schema);
        } else {
            // Tool arguments are honoured where a response schema is not.
            text = format!("{text}\n\n{}", w.fill("harness.event.tool", &names, &[]));
            request.tools = api::tools::event_tools(&self.corpus.descriptions)
                .into_iter()
                .map(|t| ToolSpec {
                    name: t.name.to_owned(),
                    description: t.description,
                    schema: t.parameters,
                })
                .collect();
            if role.profile.wire_api.may_force_tool() && role.forced_tool() {
                request.tool_choice = ToolChoice::Tool(api::tools::MAKE_EVENT.into());
            }
        }
        request.messages = vec![Message::user(text.trim())];
        Ok(Some((
            units.iter().map(|u| u.id.clone()).collect(),
            request,
        )))
    }

    /// The model's draft, metered, or `None` when the call failed or the reply is not an
    /// event.
    async fn event_draft(&self, request: Request) -> Result<Option<Draft>> {
        let names = self.names()?;
        let (role, ep) = self.role_for(LogKind::Host)?;
        let reply = match self.llm.complete(&ep, &request).await {
            Ok(r) => r,
            Err(e) => {
                self.drop_event("call failed", &e.to_string());
                return Ok(None);
            }
        };
        let host = channel::host(&self.db())?;
        let model = self.shape_for(&role, LogKind::Host, &names).model;
        crate::meter::record(
            &self.db(),
            self.now().ms,
            world::Shape::Host,
            Some(host),
            &model,
            reply.usage,
            role.prices(),
            role.cache_write_ratio(),
            role.per_request(),
            &self.config.world.cost,
            &self.config.agent.cost,
        )?;
        self.emit_quota()?;
        let labels = Labels::new(&self.corpus.wording, &names);
        // The call's arguments when the model made it; else whatever it wrote.
        let text = reply
            .tool_calls()
            .find(|(_, name, _)| *name == api::tools::MAKE_EVENT)
            .map(|(_, _, args)| args.to_owned())
            .unwrap_or_else(|| answer(&reply));
        match parse(&text, &labels) {
            Ok(draft) => Ok(Some(draft)),
            Err(why) => {
                self.drop_event(why.reason(), &text);
                Ok(None)
            }
        }
    }

    /// One line in the diagnostics log about an event that was not posted.
    fn drop_event(&self, reason: &str, text: &str) {
        diag::record(
            &self.config.logs,
            self.now().ms,
            &[("what", "event dropped"), ("reason", reason)],
            text,
        );
    }

    /// The ids a model wrote, as people it may name: a person's id or display name.
    fn event_people(&self, listed: &[String]) -> Result<Vec<PersonId>> {
        let mut roster = Vec::new();
        for p in self.offerable(&self.db())? {
            let display = self.display(&p)?;
            roster.push((PersonId::from(p.as_str()), display));
        }
        let ix = self.ix();
        let by_alias = |n: &str| Ok(ix.folio().persons_for_alias(n)?);
        let mut out: Vec<PersonId> = Vec::new();
        for n in listed {
            // The roster is shown as `id: name`; a model may copy the whole line.
            let head = n.split([':', '\u{ff1a}']).next().unwrap_or(n);
            let found = match crate::address::member_named(n.trim(), &roster, by_alias)? {
                Some(p) => Some(p),
                None => crate::address::member_named(head.trim(), &roster, by_alias)?,
            };
            if let Some(p) = found
                && !out.contains(&p)
            {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Store the event, tell the people it involves, and post it with its offers.
    fn post_event(&self, draft: Draft, material: &[String]) -> Result<bool> {
        let now = self.now();
        let max = self.config.world.events.max_persons as usize;
        let mut persons = self.event_people(&draft.persons)?;
        persons.truncate(max);
        if persons.is_empty() {
            self.drop_event("no person resolved", &draft.persons.join(", "));
            return Ok(false);
        }
        let title = cut(&draft.title, TITLE_MAX);
        let text = cut(&draft.text, TEXT_MAX);
        let names = self.names()?;
        let w = &self.corpus.wording;
        let told = w.fill("events.fact", &names, &[("title", &title), ("text", &text)]);
        let (id, host) = {
            let conn = self.db();
            let fact = fact::write(
                &conn,
                &NewFact::new(Actor::Host, Audience::World, FactKind::Fact, told)
                    .expires(event::day_end(now)),
                now.ms,
            )?
            .id();
            let id = event::record(
                &conn,
                &event::New {
                    title: title.clone(),
                    text: text.clone(),
                    persons: persons.clone(),
                    material: material.to_vec(),
                    fact: Some(fact),
                },
                now,
            )?;
            (id, channel::host(&conn)?)
        };
        // The offers: a gathering the host would arrange, and people worth a word.
        let mut actions = Vec::new();
        if let Some(g) = draft.group {
            let members = self.event_people(&g.members)?;
            if (2..=GATHERING_MAX).contains(&members.len()) {
                let refs = self.person_refs(&members)?;
                actions.push(ToolAction::CreateGroup {
                    members: refs,
                    channel: None,
                    purpose: clean_purpose(g.purpose.as_deref())
                        .or_else(|| clean_purpose(Some(&title))),
                });
            }
        }
        let talk: Vec<PersonId> = self
            .event_people(&draft.talk)?
            .into_iter()
            .take(max)
            .collect();
        if !talk.is_empty() {
            actions.push(ToolAction::FindPeople {
                topic: title.clone(),
                persons: self.person_refs(&talk)?,
            });
        }
        let frames: Vec<&str> = w.get("host.event.frame").split('|').collect();
        let frame = frames[id as usize % frames.len()].trim();
        let line = w.fill_text(frame, &names, &[("title", &title), ("text", &text)]);
        let calls =
            serde_json::to_value(&actions).map_err(|e| world::Error::Invalid(e.to_string()))?;
        let conn = self.db();
        let msg = message::append(
            &conn,
            host,
            &Actor::Host,
            &line,
            (!actions.is_empty()).then_some(&calls),
            None,
            now.ms,
        )?;
        event::set_message(&conn, id, msg)?;
        let stored = message::get(&conn, msg)?;
        let shown = view::message(&conn, &stored, now.offset_min);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
        self.changed(vec![WorldChange::Digest]);
        Ok(true)
    }

    fn person_refs(&self, people: &[PersonId]) -> Result<Vec<api::types::PersonRef>> {
        people
            .iter()
            .map(|p| view::person_ref(self.ix().folio(), p.as_str()))
            .collect()
    }

    /// The line a segment gets about today's events that involve the channel's people:
    /// once per segment, unless its opening block already told it.
    pub(crate) fn event_lines(
        &self,
        seg: world::SegmentId,
        turn: &mut Turn,
        people: &[PersonId],
    ) -> Result<Vec<String>> {
        self.sweep();
        let now = self.now();
        let mut seen = self.seen(seg)?;
        let names = self.segment_names(seg)?;
        let mut out = Vec::new();
        for p in people.iter().filter(|p| self.has_persona(p.as_str())) {
            for f in event::facts_for(&self.db(), p, now)? {
                let id = assemble::memory_id(&f);
                if !seen.insert(id.clone()) {
                    continue;
                }
                let line = self.corpus.wording.fill(
                    "harness.event",
                    &names,
                    &[("text", names.sub(f.text.trim()).as_ref())],
                );
                out.push(format!("[#{id}] {line}"));
                turn.shown.push(id);
            }
        }
        Ok(out)
    }

    /// Today's and the last few events, newest first.
    pub fn daily_events(&self) -> Result<Vec<DailyEvent>> {
        Ok(event::recent(&self.db(), self.config.world.events.list)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Labels {
        Labels::new(&Wording::new(&Default::default()), &Names::fixed("", "Ann"))
    }

    #[test]
    fn an_answer_left_in_a_thinking_block_is_read() {
        let doc = r#"{"title": "T", "text": "X"}"#;
        let reply = |parts| llm::Response {
            parts,
            stop: llm::Stop::EndTurn,
            usage: llm::Usage::default(),
        };
        let thinking = llm::Part::Native(json!({"type": "thinking", "thinking": doc}));
        let only = reply(vec![thinking.clone()]);
        assert!(parse(&answer(&only), &labels()).is_ok());
        let both = reply(vec![thinking, llm::Part::Text("Done.".into())]);
        assert_eq!(answer(&both), "Done.");
    }

    #[test]
    fn a_reply_is_read_tolerantly() {
        let ok = r#"{"title": "T", "text": "X", "persons": ["a"], "group": null, "talk": []}"#;
        assert!(parse(ok, &labels()).is_ok());
        assert!(parse(&format!("```json\n{ok}\n```"), &labels()).is_ok());
        assert!(parse(&format!("Here is the event:\n{ok}"), &labels()).is_ok());
        assert!(
            parse(
                &format!("Here it is:\n```json\n{ok}\n```\nDone."),
                &labels()
            )
            .is_ok()
        );
        assert!(parse(r#"{"title": "T", "text": "X", "extra": 1}"#, &labels()).is_ok());
        let objects = r#"{"title": "T", "text": "X", "persons": [{"id": "a"}, "b"]}"#;
        assert_eq!(parse(objects, &labels()).unwrap().persons, ["a", "b"]);
        assert_eq!(parse(" ", &labels()).unwrap_err(), Unreadable::Empty);
        assert_eq!(
            parse(r#"{"title": " ", "text": "X", "persons": []}"#, &labels()).unwrap_err(),
            Unreadable::NoTitleOrText
        );
        assert_eq!(
            parse(r#"{"headline": "T"}"#, &labels()).unwrap_err(),
            Unreadable::NoTitleOrText
        );
        assert_eq!(
            parse("Something happened on deck.", &labels()).unwrap_err(),
            Unreadable::NoJson
        );
        let with_group = r#"{"title": "T", "text": "X", "persons": [], "talk": [],
            "group": {"members": ["a", "b"], "purpose": "tea"}}"#;
        assert_eq!(
            parse(with_group, &labels())
                .unwrap()
                .group
                .unwrap()
                .members
                .len(),
            2
        );
    }

    /// The corpus's labels for an event written as prose, as escapes.
    fn owner_labels() -> Labels {
        let pairs = [
            ("events.label.title", "\u{6807}\u{9898}"),
            ("events.label.text", "\u{6b63}\u{6587}"),
            ("events.label.persons", "\u{7275}\u{6d89}"),
            ("events.label.group", "\u{805a}\u{4f1a}"),
            ("events.label.members", "\u{6210}\u{5458}"),
            ("events.label.purpose", "\u{7531}\u{5934}"),
            (
                "events.label.talk",
                "{user}\u{53ef}\u{4ee5}\u{53bb}\u{804a}\u{804a}\u{7684}\u{4eba}",
            ),
        ];
        let w = Wording::new(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        Labels::new(&w, &Names::fixed("", "alkali"))
    }

    #[test]
    fn an_event_written_as_labelled_prose_is_read() {
        let parts = [
            "\u{6807}\u{9898}\u{ff1a}\u{5976}\u{916a}\u{706b}\u{9505}\u{6392}\u{73ed}\u{8868}",
            "\u{6b63}\u{6587}\u{ff1a}\u{5348}\u{996d}\u{65f6}\u{8036}\u{62c9}\u{7aef}\u{7740}\u{5976}\u{916a}\u{706b}\u{9505}\u{95ee}\u{963f}\u{7c73}\u{5a05}\u{ff0c}\u{7f57}\u{5fb7}\u{5c9b}\u{80fd}\u{4e0d}\u{80fd}\u{5728}\u{201c}\u{521a}\u{8fc7}\u{5b8c}\u{7684}\u{8282}\u{201d}\u{548c}\u{201c}\u{4e0b}\u{4e00}\u{4e2a}\u{8282}\u{201d}\u{4e4b}\u{95f4}\u{518d}\u{8865}\u{4e00}\u{987f}\u{3002}\u{963f}\u{7c73}\u{5a05}\u{7b11}\u{7740}\u{8bf4}\u{53ef}\u{4ee5}\u{ff0c}\u{4f46}\u{5f97}\u{7b49}\u{5979}\u{548c}\u{51ef}\u{5c14}\u{5e0c}\u{5148}\u{8ddf}alkali\u{8c08}\u{5b8c}\u{4e8b}\u{3002}",
            "\u{7275}\u{6d89}\u{7684}\u{4eba}\u{ff1a}\u{8036}\u{62c9}\u{3001}\u{963f}\u{7c73}\u{5a05}\u{3001}\u{51ef}\u{5c14}\u{5e0c}",
            "\u{805a}\u{4f1a}\u{ff1a}\u{6210}\u{5458}\u{ff1a}\u{8036}\u{62c9}\u{3001}\u{963f}\u{7c73}\u{5a05}\u{ff1b}\u{7531}\u{5934}\u{ff1a}\u{8036}\u{62c9}\u{63d0}\u{8bae}\u{5728}\u{98df}\u{5802}\u{8bd5}\u{5976}\u{916a}\u{706b}\u{9505}\u{ff0c}\u{987a}\u{4fbf}\u{7b49}alkali\u{8c08}\u{5b8c}\u{4e8b}\u{3002}",
            "alkali\u{53ef}\u{4ee5}\u{53bb}\u{804a}\u{804a}\u{7684}\u{4eba}\u{ff1a}\u{8036}\u{62c9}\u{3001}\u{963f}\u{7c73}\u{5a05}\u{3001}\u{51ef}\u{5c14}\u{5e0c}",
        ];
        // As the host's endpoint wrote it: its parts apart by blanks or by lines.
        for gap in ["   ", "\n", "\n\n"] {
            let draft = parse(&parts.join(gap), &owner_labels()).unwrap();
            assert_eq!(
                draft.title,
                "\u{5976}\u{916a}\u{706b}\u{9505}\u{6392}\u{73ed}\u{8868}"
            );
            assert!(
                draft.text.starts_with("\u{5348}\u{996d}\u{65f6}"),
                "{}",
                draft.text
            );
            assert!(
                draft.text.ends_with("\u{8c08}\u{5b8c}\u{4e8b}\u{3002}"),
                "{}",
                draft.text
            );
            assert_eq!(draft.persons.len(), 3);
            assert_eq!(draft.talk.len(), 3);
            let group = draft.group.unwrap();
            assert_eq!(
                group.members,
                ["\u{8036}\u{62c9}", "\u{963f}\u{7c73}\u{5a05}"]
            );
            assert!(
                group
                    .purpose
                    .unwrap()
                    .starts_with("\u{8036}\u{62c9}\u{63d0}\u{8bae}")
            );
        }
        // A title and no text is nothing; so is prose with no labels at all.
        assert!(parse("Title: only", &labels()).is_err());
        assert!(parse("Something happened on deck.", &owner_labels()).is_err());
        // The default labels read English prose, and a label may run on before its colon.
        let english = "Title: Tea\nText: A pot was shared.\nPeople involved (ids): a, b\n\
                       Gathering: Members: a, b; Purpose: tea\nAnn could talk to: a";
        let draft = parse(english, &labels()).unwrap();
        assert_eq!(
            (draft.title.as_str(), draft.persons.len(), draft.talk),
            ("Tea", 2, vec!["a".to_owned()])
        );
        assert_eq!(draft.group.unwrap().purpose.as_deref(), Some("tea"));
    }
}
