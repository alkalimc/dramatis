//! The facts drawer.

use world::fact;

use super::Engine;
use crate::convert::{self, world_err};
use crate::error::{ApiError, ApiResult};
use crate::events::WorldChange;
use crate::types::*;
use crate::views::*;

impl Engine {
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
            outcome: f.outcome.map(convert::fact_outcome),
            text: f.text.clone(),
            retracted: f.retracted,
            created_at: convert::ts(f.created_at, offset),
        })
    }

    pub(crate) fn facts_impl(&self, filter: FactFilter) -> ApiResult<FactList> {
        let offset = self.now().offset_min;
        let person = filter.person.as_ref().map(convert::world_person);
        let w = self.w();
        let everything = fact::all(&w, true).map_err(world_err)?;
        let deleted = everything.iter().filter(|f| f.retracted).count();
        let listed = fact::list(&w, convert::fact_scope(filter.scope), person.as_ref())
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
