//! Away mode: the user has left and nobody comes to him.
//!
//! The switch is stored per save as the instant it was turned on. While it stands, every
//! unprompted thing is held (see [`is_away`]'s callers: the seed plan, the daily events,
//! the host's reminders and line). Turning it off settles the time away: the visit and
//! event slots that passed are spent, not saved up, and the reminders that fell due are
//! handed back once for one line.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::clock::Now;
use crate::params::Params;
use crate::seed::Slots;
use crate::types::Actor;
use crate::{Result, event, fact, settings};

/// Setting: the instant away mode was turned on; absent when it is off.
const AWAY: &str = "away";
/// Setting: what the last stretch away spent.
const SPENT: &str = "away.spent";

/// Slots spent on one local day, and until when the event slot of the part now in is spent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Spent {
    day: i32,
    slots: u32,
    events_until: i64,
}

fn day_key(now: Now) -> i32 {
    let d = now.local_date();
    d.year * 10_000 + i32::from(d.month) * 100 + i32::from(d.day)
}

/// When away mode was turned on, `None` when it is off.
pub fn since(conn: &Connection) -> Result<Option<i64>> {
    settings::get(conn, AWAY)
}

pub fn is_away(conn: &Connection) -> Result<bool> {
    Ok(since(conn)?.is_some())
}

/// Turn it on. A second call keeps the first instant. Returns whether it changed.
pub fn begin(conn: &Connection, now: i64) -> Result<bool> {
    if is_away(conn)? {
        return Ok(false);
    }
    settings::set(conn, AWAY, &now)?;
    Ok(true)
}

/// Visit slots spent away on today's date, to count with the openings delivered.
pub fn spent_slots(conn: &Connection, now: Now) -> Result<u32> {
    let spent: Spent = settings::get(conn, SPENT)?.unwrap_or_default();
    Ok(if spent.day == day_key(now) {
        spent.slots
    } else {
        0
    })
}

/// Whether the event slot of the part now is in was spent away.
pub fn event_slot_spent(conn: &Connection, now: Now) -> Result<bool> {
    let spent: Spent = settings::get(conn, SPENT)?.unwrap_or_default();
    Ok(now.ms < spent.events_until)
}

/// Turn it off and settle the time away. `None` when it was not on. Otherwise the texts of
/// the reminders and promises the user set that fell due meanwhile, marked delivered; the
/// day goes on from the next unused slot. Due commitments of the people themselves are
/// dropped, spent like a visit.
pub fn end(conn: &Connection, now: Now, params: &Params) -> Result<Option<Vec<String>>> {
    let Some(since) = since(conn)? else {
        return Ok(None);
    };
    let quiet = settings::quiet_hours(conn, &params.seed)?;
    let mut spent: Spent = settings::get(conn, SPENT)?.unwrap_or_default();
    if spent.day != day_key(now) {
        spent = Spent::default();
    }
    spent.day = day_key(now);
    let pace = params.pacing.get(settings::intensity(conn)?);
    spent.slots += Slots::new(quiet.as_ref(), now, pace.daily_total, 0).passed_since(now, since);
    let per_day = settings::events_per_day(conn, &pace)?;
    if per_day > 0
        && let Some(end) = event::slots(quiet.as_ref(), now, per_day).part_spent_since(now, since)
    {
        spent.events_until = spent.events_until.max(end);
    }
    settings::set(conn, SPENT, &spent)?;
    let mut said = Vec::new();
    for f in fact::due_commitments(conn, now.ms)? {
        if f.due.is_none_or(|d| d <= since) {
            continue;
        }
        fact::mark_delivered(conn, f.id)?;
        if f.author != Actor::Host && f.author != Actor::User {
            continue;
        }
        said.push(f.text);
    }
    settings::unset(conn, AWAY)?;
    Ok(Some(said))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{Audience, NewFact};
    use crate::testing::*;
    use crate::types::FactKind;

    fn commitment(w: &Connection, by: Actor, text: &str, due: i64) -> crate::FactId {
        fact::write(
            w,
            &NewFact::new(by, Audience::World, FactKind::Commitment, text).due(due),
            T0 - 1,
        )
        .unwrap()
        .id()
    }

    #[test]
    fn what_fell_due_while_away_comes_back_once_and_the_rest_stays() {
        let w = world();
        let params = Params::default();
        let before = commitment(&w, Actor::Host, "before", T0 + HOUR);
        assert!(begin(&w, T0 + 2 * HOUR).unwrap());
        assert!(
            !begin(&w, T0 + 3 * HOUR).unwrap(),
            "the first instant stays"
        );
        assert_eq!(since(&w).unwrap(), Some(T0 + 2 * HOUR));
        commitment(&w, Actor::Host, "water the plants", T0 + 3 * HOUR);
        commitment(&w, Actor::User, "call home", T0 + 4 * HOUR);
        let theirs = commitment(&w, Actor::Person(p("a")), "bring notes", T0 + 4 * HOUR);
        commitment(&w, Actor::Host, "later", T0 + 9 * HOUR);
        let said = end(&w, at(T0 + 6 * HOUR), &params).unwrap().unwrap();
        assert_eq!(said, ["water the plants", "call home"]);
        assert!(
            fact::get(&w, theirs).unwrap().delivered,
            "dropped, not kept"
        );
        // The one due before leaving is still owed; the later one is not due yet.
        let owed: Vec<_> = fact::due_commitments(&w, T0 + 6 * HOUR)
            .unwrap()
            .into_iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(owed, [before]);
        assert_eq!(end(&w, at(T0 + 6 * HOUR), &params).unwrap(), None);
    }
}
