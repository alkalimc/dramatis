//! Daily events: the host posts a small happening of the day in its own channel, with
//! offers to join a gathering or talk to someone it involves. One structured request on
//! the host role per event, drawn from a corpus unit so it fits the world; a reply that
//! cannot be read costs the call and nothing else.
//!
//! The people it involves learn of it through a world fact that ends with the day: it
//! reaches the opening block of any segment opened that day, and the first turn of a
//! segment already open when it happened. A seed lets one of them bring it up.

use llm::{Message, Request};
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
use crate::view;
use crate::wrapup_json;
use api::events::{MessageAdded, WorldChange};
use api::views::ToolAction;

/// Longest title and text kept, in characters; more is cut.
const TITLE_MAX: usize = 60;
const TEXT_MAX: usize = 600;
/// Most people a suggested gathering brings together (the host's offer to the user).
const GATHERING_MAX: usize = 5;

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "description": "a short title"},
            "text": {"type": "string", "description": "two to four sentences"},
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

/// The event in a reply. Extra fields are ignored and optional ones may be missing; only
/// a title and a text are needed.
fn parse(reply: &str) -> std::result::Result<Draft, Unreadable> {
    if reply.trim().is_empty() {
        return Err(Unreadable::Empty);
    }
    let doc = object(reply).ok_or(Unreadable::NoJson)?;
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
        // The everyday topic of the day, rotating; and the topic of the newest group.
        let topics: Vec<&str> = w
            .get("events.topics")
            .split('|')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        let day = now.local_day_start() / world::clock::DAY_MS;
        let used = event::used_material(&self.db(), self.config.world.events.list)?;
        let today = event::count_on(&self.db(), &event::day_key(now))?;
        let topic = (!topics.is_empty())
            .then(|| topics[(day as usize + today as usize) % topics.len()].to_owned());
        let group_topics: Vec<String> = channel::list(&self.db())?
            .into_iter()
            .filter(|c| c.kind == world::ChannelKind::Group)
            .filter_map(|c| c.topic)
            .filter(|t| !t.trim().is_empty())
            .take(2)
            .collect();
        let mut units = Vec::new();
        for query in topic.iter().chain(group_topics.first()) {
            let mut req = index::SearchRequest::new(query.as_str());
            req.exclude = used
                .iter()
                .cloned()
                .chain(units.iter().map(|u: &folio::Unit| u.id.clone()))
                .collect();
            let Ok(resp) = self.ix().search(&req) else {
                continue;
            };
            let first = resp
                .hits
                .iter()
                .filter(|h| !req.exclude.iter().any(|x| x == h.item.id()))
                .find_map(|h| h.item.unit().cloned());
            units.extend(first);
        }
        let material = if units.is_empty() {
            String::new()
        } else {
            let mut seen = std::collections::HashSet::new();
            assemble::units(&self.ctx(&names), &units, &mut seen).text
        };
        // Who it may involve: the people of the material, then a rotating handful so the
        // same few do not carry every day. Birthdays are not a reason: with hundreds of
        // people nearly every day has one.
        let mut pool: Vec<String> = Vec::new();
        let push = |p: &str, pool: &mut Vec<String>| {
            if offerable.iter().any(|o| o == p) && !pool.iter().any(|x| x == p) {
                pool.push(p.to_owned());
            }
        };
        for u in &units {
            for p in &u.persons {
                push(p, &mut pool);
            }
        }
        let pool_max = self.config.world.events.pool as usize;
        let start = (day as usize).wrapping_mul(7) % offerable.len();
        for i in 0..offerable.len() {
            if pool.len() >= pool_max {
                break;
            }
            push(&offerable[(start + i) % offerable.len()], &mut pool);
        }
        pool.truncate(pool_max.max(1));
        let mut people = Vec::new();
        for p in &pool {
            people.push(format!("{p}: {}", self.display(p)?));
        }
        let d = world::clock::in_world_date(now, self.corpus.year_offset);
        let date = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
        let (role, _) = self.role_for(LogKind::Host)?;
        let schema = schema();
        let mut extra = Vec::new();
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
                ("topic", topic.as_deref().unwrap_or("")),
                ("material", &material),
                ("people", &people.join("\n")),
                ("extra", &extra.join("\n")),
            ],
        );
        // The endpoint may not enforce the response schema, so an emulating one is shown
        // the shape in words.
        if role.model.emulated_tools() {
            let shape = wrapup_json::example(&schema);
            text = format!(
                "{text}\n\n{}",
                w.fill("harness.wrapup.shape", &names, &[("shape", &shape)])
            );
        }
        let request = Request {
            messages: vec![Message::user(text.trim())],
            output_schema: Some(schema),
            ..Request::default()
        };
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
        let text = answer(&reply);
        match parse(&text) {
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
        let shown = view::message(&conn, &stored, now.offset_min, &self.config.office);
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
        assert!(parse(&answer(&only)).is_ok());
        let both = reply(vec![thinking, llm::Part::Text("Done.".into())]);
        assert_eq!(answer(&both), "Done.");
    }

    #[test]
    fn a_reply_is_read_tolerantly() {
        let ok = r#"{"title": "T", "text": "X", "persons": ["a"], "group": null, "talk": []}"#;
        assert!(parse(ok).is_ok());
        assert!(parse(&format!("```json\n{ok}\n```")).is_ok());
        assert!(parse(&format!("Here is the event:\n{ok}")).is_ok());
        assert!(parse(&format!("Here it is:\n```json\n{ok}\n```\nDone.")).is_ok());
        assert!(parse(r#"{"title": "T", "text": "X", "extra": 1}"#).is_ok());
        let objects = r#"{"title": "T", "text": "X", "persons": [{"id": "a"}, "b"]}"#;
        assert_eq!(parse(objects).unwrap().persons, ["a", "b"]);
        assert_eq!(parse(" ").unwrap_err(), Unreadable::Empty);
        assert_eq!(
            parse(r#"{"title": " ", "text": "X", "persons": []}"#).unwrap_err(),
            Unreadable::NoTitleOrText
        );
        assert_eq!(
            parse(r#"{"headline": "T"}"#).unwrap_err(),
            Unreadable::NoTitleOrText
        );
        assert_eq!(
            parse("Something happened on deck.").unwrap_err(),
            Unreadable::NoJson
        );
        let with_group = r#"{"title": "T", "text": "X", "persons": [], "talk": [],
            "group": {"members": ["a", "b"], "purpose": "tea"}}"#;
        assert_eq!(parse(with_group).unwrap().group.unwrap().members.len(), 2);
    }
}
