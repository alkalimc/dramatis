//! Requests, case files, conclusions and the facts drawer.

use world::{bond, fact, message, seed, task};

use super::Engine;
use crate::convert::{self, world_err};
use crate::error::{ApiError, ApiResult};
use crate::events::WorldChange;
use crate::types::*;
use crate::views::*;

impl Engine {
    fn task_summary(&self, t: &task::Task, offset: i32) -> TaskSummary {
        TaskSummary {
            id: convert::task_id(t.id),
            question: t.question.clone(),
            assignee: t.assignee.as_ref().map(|p| self.person_ref(p.as_str())),
            parent: t.parent.map(convert::task_id),
            status: convert::task_status(t.status),
            pinned: t.pinned,
            channel: t.channel.map(convert::channel_id),
            created_at: convert::ts(t.created_at.unwrap_or(0), offset),
        }
    }

    pub(crate) async fn ask_impl(
        &self,
        person: PersonId,
        question: String,
        turns: Option<u32>,
        parent: Option<TaskId>,
    ) -> ApiResult<TaskId> {
        if question.trim().is_empty() {
            return Err(ApiError::Invalid {
                field: "question".into(),
            });
        }
        if let Some(p) = &parent {
            task::get(&self.w(), convert::world_task(p)?).map_err(world_err)?;
        }
        self.require_available(&person)?;
        if !self.has_chat() {
            return Err(ApiError::NoEndpoint);
        }
        self.ports.agent.ask(person, question, turns, parent).await
    }

    pub(crate) fn tasks_impl(&self) -> ApiResult<Vec<TaskSummary>> {
        let offset = self.now().offset_min;
        let all = task::list(&self.w()).map_err(world_err)?;
        Ok(all.iter().map(|t| self.task_summary(t, offset)).collect())
    }

    pub(crate) fn case_file_impl(&self, id: &TaskId) -> ApiResult<CaseFile> {
        let id = convert::world_task(id)?;
        let offset = self.now().offset_min;
        let w = self.w();
        let t = task::get(&w, id).map_err(world_err)?;
        let mut trail = Vec::new();
        for child in task::children(&w, id).map_err(world_err)? {
            let reply = match child.reply {
                Some(m) => {
                    Some(self.message_view(&message::get(&w, m).map_err(world_err)?, offset))
                }
                None => None,
            };
            trail.push(TrailEntry {
                task: self.task_summary(&child, offset),
                reply,
            });
        }
        let conclusion = fact::conclusion_of(&w, id).map_err(world_err)?;
        let reactions = match &conclusion {
            Some(f) => self.reactions(&w, f.id)?,
            None => Vec::new(),
        };
        let conclusion = match conclusion {
            Some(f) => Some(self.fact_view(&w, &f, offset)?),
            None => None,
        };
        Ok(CaseFile {
            task: self.task_summary(&t, offset),
            trail,
            conclusion,
            reactions,
        })
    }

    /// Openings a conclusion seeded, once delivered: one quoted line each.
    fn reactions(
        &self,
        conn: &rusqlite::Connection,
        conclusion: world::FactId,
    ) -> ApiResult<Vec<MessageRef>> {
        let mut stmt = conn
            .prepare(
                "SELECT s.message FROM seed s WHERE s.kind = 'conclusion' AND s.ref = ?1
                 AND s.message IS NOT NULL ORDER BY s.delivered_at",
            )
            .map_err(|e| world_err(e.into()))?;
        let ids: Vec<world::MessageId> = stmt
            .query_map([conclusion.0.to_string()], |r| r.get(0))
            .map_err(|e| world_err(e.into()))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| world_err(e.into()))?;
        let mut out = Vec::new();
        for id in ids {
            let m = message::get(conn, id).map_err(world_err)?;
            out.push(MessageRef {
                channel: convert::channel_id(m.channel),
                message: convert::message_id(m.id),
                author: convert::author(&m.author),
                excerpt: m
                    .text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        Ok(out)
    }

    /// Queue the people an event's retrieval hits, as seeds for an unprompted opening:
    /// the people with a persona whose own units match `text` best. A search that fails
    /// queues nothing; the write that triggered it has already succeeded.
    fn queue_seeds(&self, event: seed::Event, text: &str) -> ApiResult<()> {
        let Some(c) = &self.corpus else {
            return Ok(());
        };
        let off = bond::disabled(&self.w()).map_err(world_err)?;
        let scope: Vec<String> = c
            .persona_ids()
            .into_iter()
            .filter(|p| !off.iter().any(|d| d.as_str() == p))
            .collect();
        let Ok(found) =
            c.with_index(|ix| ix.find_people(text, Some(&scope), self.params.find_people.k, &[]))
        else {
            return Ok(());
        };
        let hits: Vec<seed::Hit> = found
            .persons
            .iter()
            .filter(|m| m.level != index::Level::Low)
            .map(|m| seed::Hit {
                person: world::PersonId(m.person.clone()),
                material: vec![m.reason.id.clone()],
            })
            .collect();
        seed::queue_hits(&self.w(), event, &hits, self.now().ms).map_err(world_err)?;
        Ok(())
    }

    pub(crate) async fn open_case_impl(&self, question: String) -> ApiResult<TaskId> {
        let id = task::open_case(&self.w(), &question, self.now().ms).map_err(world_err)?;
        self.queue_seeds(seed::Event::Pinned(id), question.trim())?;
        let id = convert::task_id(id);
        self.changed(vec![WorldChange::Task { id: id.clone() }]);
        let _ = self.ports.agent.on_event().await;
        Ok(id)
    }

    pub(crate) async fn pin_impl(&self, id: &TaskId, pinned: bool) -> ApiResult<()> {
        let t = convert::world_task(id)?;
        let question = {
            let w = self.w();
            task::set_pinned(&w, t, pinned).map_err(world_err)?;
            task::get(&w, t).map_err(world_err)?.question
        };
        if pinned {
            self.queue_seeds(seed::Event::Pinned(t), &question)?;
        }
        self.changed(vec![WorldChange::Task { id: id.clone() }]);
        if pinned {
            let _ = self.ports.agent.on_event().await;
        }
        Ok(())
    }

    pub(crate) async fn write_conclusion_impl(&self, id: &TaskId, text: &str) -> ApiResult<FactId> {
        let t = convert::world_task(id)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(ApiError::Invalid {
                field: "text".into(),
            });
        }
        let f = fact::write_conclusion(&self.w(), t, text, self.now().ms).map_err(world_err)?;
        self.queue_seeds(seed::Event::Conclusion(f), text)?;
        let f = convert::fact_id(f);
        self.changed(vec![
            WorldChange::Fact { id: f.clone() },
            WorldChange::Task { id: id.clone() },
        ]);
        let _ = self.ports.agent.on_event().await;
        Ok(f)
    }

    // ---- facts ----

    fn fact_view(
        &self,
        conn: &rusqlite::Connection,
        f: &fact::Fact,
        offset: i32,
    ) -> ApiResult<FactView> {
        let audience = match &f.audience {
            fact::Audience::World => Audience::World,
            fact::Audience::Participants(ch) => Audience::Participants {
                channel: convert::channel_id(*ch),
                persons: fact::audience_persons(conn, &f.audience)
                    .map_err(world_err)?
                    .unwrap_or_default()
                    .iter()
                    .map(|p| self.person_ref(p.as_str()))
                    .collect(),
            },
            fact::Audience::Own(p) => Audience::Own {
                person: self.person_ref(p.as_str()),
            },
        };
        Ok(FactView {
            id: convert::fact_id(f.id),
            author: convert::author(&f.author),
            audience,
            kind: convert::fact_kind(f.kind),
            about: f.about.as_ref().map(|a| match a {
                fact::About::Task(t) => About::Task {
                    id: convert::task_id(*t),
                },
                fact::About::Fact(x) => About::Fact {
                    id: convert::fact_id(*x),
                },
                fact::About::Person(p) => About::Person {
                    id: convert::person_id(p),
                },
                fact::About::User => About::User,
                fact::About::Citation(c) => About::Citation {
                    citation: convert::citation(c),
                },
            }),
            due: f.due.map(|ms| convert::ts(ms, offset)),
            delivered: f.delivered,
            text: f.text.clone(),
            retracted: f.retracted,
            created_at: convert::ts(f.created_at, offset),
        })
    }

    pub(crate) fn facts_impl(&self, filter: FactFilter) -> ApiResult<FactList> {
        let offset = self.now().offset_min;
        let person = filter.person.as_ref().map(convert::world_person);
        let task = filter.task.as_ref().map(convert::world_task).transpose()?;
        let w = self.w();
        let everything = fact::all(&w, true).map_err(world_err)?;
        let deleted = everything.iter().filter(|f| f.retracted).count();
        let listed = fact::list(&w, convert::fact_scope(filter.scope), person.as_ref(), task)
            .map_err(world_err)?;
        let mut items = Vec::with_capacity(listed.len());
        for f in &listed {
            items.push(self.fact_view(&w, f, offset)?);
        }
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Ok(FactList {
            total: count(everything.len() - deleted),
            deleted: count(deleted),
            items,
        })
    }

    pub(crate) fn edit_fact_impl(&self, id: &FactId, text: &str) -> ApiResult<FactId> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ApiError::Invalid {
                field: "text".into(),
            });
        }
        let new = fact::edit(&self.w(), convert::world_fact(id)?, text, self.now().ms)
            .map_err(world_err)?;
        let new = convert::fact_id(new);
        self.changed(vec![
            WorldChange::Fact { id: id.clone() },
            WorldChange::Fact { id: new.clone() },
        ]);
        Ok(new)
    }

    pub(crate) fn retract_impl(&self, id: &FactId, delete: bool) -> ApiResult<()> {
        let f = convert::world_fact(id)?;
        let w = self.w();
        if delete {
            fact::retract(&w, f)
        } else {
            fact::restore(&w, f)
        }
        .map_err(world_err)?;
        drop(w);
        self.changed(vec![WorldChange::Fact { id: id.clone() }]);
        Ok(())
    }
}
