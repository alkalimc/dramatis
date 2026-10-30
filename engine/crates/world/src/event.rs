//! Daily events: short happenings the host makes up for a local day so the world feels
//! lived in. This module keeps them and decides when one is due; making one is the
//! caller's (it takes a model call).
//!
//! An event has a world-audience fact that ends with its day ([`fact::sweep_expired`]),
//! so those it involves, and everyone else lightly, can mention it that day and no later.
//! At most `settings::events_per_day` are made per local day, only while the user is
//! about, and never where unprompted speech is held back. The waking day is cut into that
//! many parts and each part gets one, so they spread; a part that passes without one is
//! not made up for.

use rusqlite::{Connection, Row};

use crate::clock::{self, DAY_MS, Now};
use crate::fact::{self, Fact};
use crate::params::{Params, QuietHours};
use crate::quota::{Band, Status};
use crate::seed::{self, Slots};
use crate::types::{FactId, MessageId, PersonId};
use crate::{Result, away, message, settings};

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
    let pace = params.pacing.get(status.tier);
    let per_day = settings::events_per_day(conn, &pace)?;
    if per_day == 0 || count_on(conn, &day_key(now))? >= per_day {
        return Ok(false);
    }
    if status.band != Band::Open || away::is_away(conn)? || away::event_slot_spent(conn, now)? {
        return Ok(false);
    }
    let quiet = settings::quiet_hours(conn, &params.seed)?;
    if quiet
        .as_ref()
        .is_some_and(|q| clock::in_quiet_hours(q, now))
    {
        return Ok(false);
    }
    if clock::too_soon(seed::last_opening(conn)?, now.ms, pace.gap_min) {
        return Ok(false);
    }
    // One event to a part of the waking day, from its slot on; a part that went by
    // without one is not made up for.
    let (from, to, come) = slots(quiet.as_ref(), now, per_day).current(now);
    let made: u32 = conn.query_row(
        "SELECT count(*) FROM daily_event WHERE at >= ?1 AND at < ?2",
        [from, to],
        |r| r.get(0),
    )?;
    if !come || made > 0 {
        return Ok(false);
    }
    Ok(message::last_user_message_at(conn)?.is_some())
}

/// The slots of `per_day` events over the waking day.
pub fn slots(quiet: Option<&QuietHours>, now: Now, per_day: u32) -> Slots {
    Slots::new(quiet, now, per_day, 1)
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

    /// Run a fake clock over `[from, to)` in ten-minute steps; an event is made whenever
    /// one is due. The instants they were made at.
    fn run_clock(w: &Connection, params: &Params, from: i64, to: i64) -> Vec<i64> {
        let mut made = Vec::new();
        let mut t = from;
        while t < to {
            let now = at(t);
            if due(w, now, params, &status(w, now)).unwrap() {
                make(w, now, &["a"]);
                made.push(t);
            }
            t += 10 * 60_000;
        }
        made
    }

    fn user_spoke(w: &Connection) {
        let c = crate::channel::direct(w, &p("a")).unwrap();
        message::append(w, c, &Actor::User, "hi", None, None, 1).unwrap();
    }

    #[test]
    fn due_once_a_day_after_the_user_spoke() {
        let w = world();
        let params = Params::default();
        let midnight = T0 - 12 * clock::HOUR_MS;
        assert!(run_clock(&w, &params, midnight, midnight + DAY_MS).is_empty());
        user_spoke(&w);
        // One a day, whenever in the waking day its slot falls; the next day is a new
        // count and the missed hours are not made up.
        let made = run_clock(&w, &params, midnight, midnight + 3 * DAY_MS);
        assert_eq!(made.len(), 3);
        for (i, t) in made.iter().enumerate() {
            assert_eq!((t - midnight) / DAY_MS, i as i64);
        }
    }

    #[test]
    fn events_fall_in_distinct_slots_apart_and_outside_quiet_hours() {
        for n in [1, 2, 4] {
            let w = world();
            let params = Params::default();
            user_spoke(&w);
            settings::set(&w, settings::EVENTS_PER_DAY, &n).unwrap();
            let midnight = T0 - 12 * clock::HOUR_MS;
            let made = run_clock(&w, &params, midnight, midnight + 2 * DAY_MS);
            assert_eq!(made.len(), 2 * n as usize, "n = {n}");
            let part = 15 * 60 / i64::from(n) * 60_000;
            let gap = i64::from(params.pacing.middle.gap_min) * 60_000;
            let quiet = params.seed.quiet_hours.clone();
            for (i, t) in made.iter().enumerate() {
                assert!(
                    !clock::in_quiet_hours(&quiet, at(*t)),
                    "n = {n}: {t} is quiet"
                );
                // The day's i-th event falls in the i-th part of the waking day.
                let day = i as i64 / i64::from(n);
                let k = i as i64 % i64::from(n);
                let from = midnight + day * DAY_MS + 8 * clock::HOUR_MS + k * part;
                assert!((from..from + part).contains(t), "n = {n}: event {i} at {t}");
            }
            for pair in made.windows(2) {
                assert!(pair[1] - pair[0] >= gap, "n = {n}: {pair:?}");
            }
        }
    }

    #[test]
    fn a_missed_slot_is_not_made_up_in_a_burst() {
        let w = world();
        let params = Params::default();
        user_spoke(&w);
        settings::set(&w, settings::EVENTS_PER_DAY, &4).unwrap();
        // The app was off until late evening: one event for the part it is in, not four.
        let midnight = T0 - 12 * clock::HOUR_MS;
        let made = run_clock(
            &w,
            &params,
            midnight + 21 * clock::HOUR_MS,
            midnight + DAY_MS,
        );
        assert!(made.len() <= 1, "{made:?}");
    }

    #[test]
    fn the_setting_decides_how_many() {
        let w = world();
        let params = Params::default();
        user_spoke(&w);
        let midnight = T0 - 12 * clock::HOUR_MS;
        settings::set(&w, settings::EVENTS_PER_DAY, &2).unwrap();
        assert_eq!(run_clock(&w, &params, midnight, midnight + DAY_MS).len(), 2);
        settings::set(&w, settings::EVENTS_PER_DAY, &0).unwrap();
        assert!(run_clock(&w, &params, midnight + DAY_MS, midnight + 2 * DAY_MS).is_empty());
        // A larger number than the tier allows is cut; the choice stays stored.
        settings::set(&w, settings::EVENTS_PER_DAY, &9).unwrap();
        let pace = |t| params.pacing.get(t);
        assert_eq!(
            settings::events_per_day(&w, &pace(Tier::Middle)).unwrap(),
            4
        );
        assert_eq!(settings::events_per_day(&w, &pace(Tier::Ultra)).unwrap(), 6);
        assert_eq!(settings::events_per_day(&w, &pace(Tier::Low)).unwrap(), 2);
        // Without a choice the tier's own default stands.
        settings::unset(&w, settings::EVENTS_PER_DAY).unwrap();
        assert_eq!(settings::events_per_day(&w, &pace(Tier::Low)).unwrap(), 1);
        assert_eq!(settings::events_per_day(&w, &pace(Tier::Ultra)).unwrap(), 3);
    }

    #[test]
    fn a_visit_and_an_event_keep_the_gap() {
        let w = world();
        let params = Params::default();
        user_spoke(&w);
        let now = at(T0);
        make(&w, now, &["a"]);
        assert_eq!(seed::last_opening(&w).unwrap(), Some(T0));
        let soon = T0 + 1000;
        assert!(clock::too_soon(
            seed::last_opening(&w).unwrap(),
            soon,
            params.pacing.middle.gap_min
        ));
        // Even once its slot has come, an event waits out the gap after a visit.
        assert!(!due(&w, at(soon), &params, &status(&w, at(soon))).unwrap());
    }

    #[test]
    fn the_event_slot_that_passed_while_away_is_spent() {
        let w = world();
        let params = Params::default();
        user_spoke(&w);
        let midnight = T0 - 12 * clock::HOUR_MS;
        away::begin(&w, midnight + 8 * clock::HOUR_MS).unwrap();
        let held = run_clock(&w, &params, midnight, midnight + 20 * clock::HOUR_MS);
        assert!(held.is_empty(), "{held:?}");
        away::end(&w, at(midnight + 20 * clock::HOUR_MS), &params).unwrap();
        // Back in the evening: today's one slot went by, so nothing is made today and
        // the next day is whole.
        let rest = run_clock(
            &w,
            &params,
            midnight + 20 * clock::HOUR_MS,
            midnight + DAY_MS,
        );
        assert!(rest.is_empty(), "{rest:?}");
        let tomorrow = run_clock(&w, &params, midnight + DAY_MS, midnight + 2 * DAY_MS);
        assert_eq!(tomorrow.len(), 1);
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
