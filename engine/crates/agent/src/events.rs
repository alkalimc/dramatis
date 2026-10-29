//! Ship events: the host posts a small happening of the day in its own channel, with
//! offers to join a gathering or talk to someone it involves. One structured request on
//! the host role per event, drawn from a corpus unit so it fits the world; a reply that
//! cannot be read costs the call and nothing else.
//!
//! The people it involves learn of it through a world fact that ends with the day: it
//! reaches the opening block of any segment opened that day, and the first turn of a
//! segment already open when it happened. A seed lets one of them bring it up.

use llm::{Message, Request};
use serde::Deserialize;
use serde_json::{Value, json};
use world::event::{self, Event as ShipEvent};
use world::fact::{self, Audience, NewFact};
use world::quota::{self, CallKind};
use world::{Actor, FactKind, PersonId, channel, message, settings};

use crate::agent::{Agent, Event};
use crate::assemble;
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

#[derive(Debug, Deserialize)]
struct Draft {
    title: String,
    text: String,
    #[serde(default)]
    persons: Vec<String>,
    #[serde(default)]
    group: Option<DraftGroup>,
    #[serde(default)]
    talk: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DraftGroup {
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    purpose: Option<String>,
}

/// The event in a reply (a JSON object, possibly fenced). Nothing readable, or a title
/// or text left empty: `None`.
fn parse(reply: &str) -> Option<Draft> {
    let doc = wrapup_json::unfence(reply.trim());
    let d: Draft = serde_json::from_str(doc).ok()?;
    (!d.title.trim().is_empty() && !d.text.trim().is_empty()).then_some(d)
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
    pub(crate) async fn ship_event(&self) -> Result<bool> {
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
        // Who it may involve: the people of the material, today's birthdays, then a
        // rotating handful so the same few do not carry every day.
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
        let today_md = now.local_date().month_day();
        let mut birthdays = Vec::new();
        for (p, md) in self.birthdays()? {
            if md == today_md {
                birthdays.push(w.fill(
                    "harness.digest.birthday",
                    &names,
                    &[("name", &self.display(p.as_str())?)],
                ));
                push(p.as_str(), &mut pool);
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
        let mut extra = birthdays;
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
        let Ok(reply) = self.llm.complete(&ep, &request).await else {
            return Ok(None);
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
        Ok(parse(&reply.text()))
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
            if let Some(p) = crate::address::member_named(n.trim(), &roster, by_alias)?
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
        let line = names
            .sub(frame)
            .replace("{user}", names.user())
            .replace("{title}", &title)
            .replace("{text}", &text);
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
    pub fn ship_events(&self) -> Result<Vec<ShipEvent>> {
        Ok(event::recent(&self.db(), self.config.world.events.list)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_read_strictly() {
        let ok = r#"{"title": "T", "text": "X", "persons": ["a"], "group": null, "talk": []}"#;
        assert!(parse(ok).is_some());
        assert!(parse(&format!("```json\n{ok}\n```")).is_some());
        assert!(parse(r#"{"title": " ", "text": "X", "persons": []}"#).is_none());
        assert!(parse(r#"{"headline": "T"}"#).is_none());
        assert!(parse("Something happened on deck.").is_none());
        let with_group = r#"{"title": "T", "text": "X", "persons": [], "talk": [],
            "group": {"members": ["a", "b"], "purpose": "tea"}}"#;
        assert_eq!(parse(with_group).unwrap().group.unwrap().members.len(), 2);
    }
}
