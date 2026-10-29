//! Daily events: short happenings the host makes up for a local day so the world feels
//! lived in. This module keeps them and decides when one is due; making one is the
//! caller's (it takes a model call).
//!
//! An event has a world-audience fact that ends with its day ([`fact::sweep_expired`]),
//! so those it involves, and everyone else lightly, can mention it that day and no later.
//! At most `settings::events_per_day` are made per local day, only while the user is
//! about, and never where unprompted speech is held back. A day that passes without one
//! is not made up for.

use rusqlite::{Connection, Row};

use crate::clock::{self, DAY_MS, Now};
use crate::fact::{self, Fact};
use crate::params::Params;
use crate::quota::{Band, Status};
use crate::types::{FactId, MessageId, PersonId};
use crate::{Result, message, settings};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: i64,
    /// The local date it belongs to, `YYYY-MM-DD`.
    pub day: String,
    pub at: i64,
    pub title: String,
    pub text: String,
    pub persons: Vec<PersonId>,
    /// Corpus units it was drawn from, so the next one draws on others.
    pub material: Vec<String>,
    pub fact: Option<FactId>,
    pub message: Option<MessageId>,
}

/// An event to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct New {
    pub title: String,
    pub text: String,
    pub persons: Vec<PersonId>,
    pub material: Vec<String>,
    pub fact: Option<FactId>,
}

/// The local date as it keys a day.
pub fn day_key(now: Now) -> String {
    let d = now.local_date();
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}

/// When the local day of `now` ends: what its events and their facts last until.
pub fn day_end(now: Now) -> i64 {
    now.local_day_start() + DAY_MS
}

const COLUMNS: &str = "id, day, at, title, text, persons, material, fact, message";

fn read(r: &Row<'_>) -> rusqlite::Result<Event> {
    let json = |col: &str| -> rusqlite::Result<Vec<String>> {
        let raw: String = r.get(col)?;
        serde_json::from_str(&raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    Ok(Event {
        id: r.get("id")?,
        day: r.get("day")?,
        at: r.get("at")?,
        title: r.get("title")?,
        text: r.get("text")?,
        persons: json("persons")?.into_iter().map(PersonId).collect(),
        material: json("material")?,
        fact: r.get("fact")?,
        message: r.get("message")?,
    })
}

pub fn record(conn: &Connection, new: &New, now: Now) -> Result<i64> {
    let persons: Vec<&str> = new.persons.iter().map(PersonId::as_str).collect();
    conn.execute(
        "INSERT INTO daily_event(day, at, title, text, persons, material, fact)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            day_key(now),
            now.ms,
            &new.title,
            &new.text,
            serde_json::to_string(&persons)?,
            serde_json::to_string(&new.material)?,
            new.fact,
        ),
    )?;
    Ok(conn.last_insert_rowid())
}

/// The host's post that tells the user about the event.
pub fn set_message(conn: &Connection, id: i64, message: MessageId) -> Result<()> {
    conn.execute(
        "UPDATE daily_event SET message = ?2 WHERE id = ?1",
        (id, message),
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> Result<Event> {
    Ok(conn.query_row(
        &format!("SELECT {COLUMNS} FROM daily_event WHERE id = ?1"),
        [id],
        read,
    )?)
}

/// The newest events, newest first.
pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM daily_event ORDER BY id DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit], read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Units earlier events drew on, newest first, at most `limit` events back.
pub fn used_material(conn: &Connection, limit: u32) -> Result<Vec<String>> {
    Ok(recent(conn, limit)?
        .into_iter()
        .flat_map(|e| e.material)
        .collect())
}

pub fn count_on(conn: &Connection, day: &str) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM daily_event WHERE day = ?1",
        [day],
        |r| r.get(0),
    )?)
}

/// Whether an event is due now. Not when the user turned them off or today's are made,
/// not before the user has said something (a new world gets its welcome first), and not
/// where unprompted speech is held: quiet hours, a quota band that is not open.
pub fn due(conn: &Connection, now: Now, params: &Params, status: &Status) -> Result<bool> {
    let per_day = settings::events_per_day(conn, &params.events)?;
    if per_day == 0 || count_on(conn, &day_key(now))? >= per_day {
        return Ok(false);
    }
    if status.band != Band::Open {
        return Ok(false);
    }
    if settings::quiet_hours(conn, &params.seed)?.is_some_and(|q| clock::in_quiet_hours(&q, now)) {
        return Ok(false);
    }
    Ok(message::last_user_message_at(conn)?.is_some())
}

/// The facts of today's events that involve `person`, oldest first. What he may bring up
/// today as something he knows of.
pub fn facts_for(conn: &Connection, person: &PersonId, now: Now) -> Result<Vec<Fact>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM daily_event WHERE day = ?1 AND fact IS NOT NULL ORDER BY id"
    ))?;
    let events: Vec<Event> = stmt
        .query_map([day_key(now)], read)?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for e in events {
        let (Some(f), true) = (e.fact, e.persons.contains(person)) else {
            continue;
        };
        let f = fact::get(conn, f)?;
        if !f.retracted && f.expires.is_none_or(|t| t > now.ms) {
            out.push(f);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{Audience, NewFact};
    use crate::quota;
    use crate::testing::*;
    use crate::types::{Actor, FactKind, Tier};

    fn status(w: &Connection, now: Now) -> Status {
        quota::status(w, Tier::Middle, now.ms, &Params::default().quota).unwrap()
    }

    fn make(w: &Connection, now: Now, who: &[&str]) -> i64 {
        let f = fact::write(
            w,
            &NewFact::new(Actor::Host, Audience::World, FactKind::Fact, "a happening")
                .expires(day_end(now)),
            now.ms,
        )
        .unwrap()
        .id();
        record(
            w,
            &New {
                title: "t".into(),
                text: "x".into(),
                persons: who.iter().map(|x| p(x)).collect(),
                material: vec!["u1".into()],
                fact: Some(f),
            },
            now,
        )
        .unwrap()
    }

    #[test]
    fn due_once_a_day_after_the_user_spoke() {
        let w = world();
        let now = at(T0);
        let params = Params::default();
        assert!(!due(&w, now, &params, &status(&w, now)).unwrap());
        let c = crate::channel::direct(&w, &p("a")).unwrap();
        message::append(&w, c, &Actor::User, "hi", None, None, T0 - 1).unwrap();
        assert!(due(&w, now, &params, &status(&w, now)).unwrap());
        make(&w, now, &["a"]);
        assert!(!due(&w, now, &params, &status(&w, now)).unwrap());
        // The next day is a new count; the missed hours are not made up.
        let next = at(T0 + DAY_MS);
        assert!(due(&w, next, &params, &status(&w, next)).unwrap());
    }

    #[test]
    fn the_setting_decides_how_many() {
        let w = world();
        let now = at(T0);
        let params = Params::default();
        let c = crate::channel::direct(&w, &p("a")).unwrap();
        message::append(&w, c, &Actor::User, "hi", None, None, T0 - 1).unwrap();
        settings::set(&w, settings::EVENTS_PER_DAY, &2).unwrap();
        make(&w, now, &["a"]);
        assert!(due(&w, now, &params, &status(&w, now)).unwrap());
        make(&w, now, &["a"]);
        assert!(!due(&w, now, &params, &status(&w, now)).unwrap());
        settings::set(&w, settings::EVENTS_PER_DAY, &0).unwrap();
        let next = at(T0 + DAY_MS);
        assert!(!due(&w, next, &params, &status(&w, next)).unwrap());
        // A larger number than the register allows is cut.
        settings::set(&w, settings::EVENTS_PER_DAY, &9).unwrap();
        assert_eq!(settings::events_per_day(&w, &params.events).unwrap(), 2);
    }

    #[test]
    fn quiet_hours_hold_it_back() {
        let w = world();
        let c = crate::channel::direct(&w, &p("a")).unwrap();
        message::append(&w, c, &Actor::User, "hi", None, None, 1).unwrap();
        let night = at(T0 + 11 * clock::HOUR_MS);
        assert!(!due(&w, night, &Params::default(), &status(&w, night)).unwrap());
    }

    #[test]
    fn the_fact_reaches_those_involved_until_the_day_ends() {
        let w = world();
        let now = at(T0);
        make(&w, now, &["a"]);
        assert_eq!(facts_for(&w, &p("a"), now).unwrap().len(), 1);
        assert!(facts_for(&w, &p("b"), now).unwrap().is_empty());
        let next = at(T0 + DAY_MS);
        assert!(facts_for(&w, &p("a"), next).unwrap().is_empty());
        assert_eq!(fact::sweep_expired(&w, day_end(now)).unwrap(), 1);
        assert!(fact::all(&w, true).unwrap().is_empty());
        // The event stays in the log; only its fact ended.
        let e = recent(&w, 5).unwrap();
        assert_eq!((e.len(), e[0].fact), (1, None));
        assert_eq!(used_material(&w, 5).unwrap(), vec!["u1".to_owned()]);
    }
}
