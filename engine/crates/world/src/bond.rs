//! Relationships and modes. A user bond with no row is the default: trust 100, frozen,
//! never seen. A person pair has no bond until they share a turn.

use rusqlite::{Connection, OptionalExtension, Row};

use crate::types::{Actor, Mode, PersonId};
use crate::{Error, Result};

/// Trust of a user bond before anything happened.
pub const USER_TRUST: f64 = 100.0;
/// Trust of a person pair when their first shared turn creates it.
pub const PAIR_TRUST: f64 = 0.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Bond {
    /// `Actor::User`, or the smaller person id of a pair.
    pub a: Actor,
    pub b: PersonId,
    pub trust: u8,
    /// The unrounded value growth accumulates in.
    pub trust_exact: f64,
    pub mode: Mode,
    /// The latest message between the two.
    pub last_seen: Option<i64>,
}

fn read(row: &Row<'_>) -> rusqlite::Result<Bond> {
    let trust: u8 = row.get("trust")?;
    let exact: Option<f64> = row.get("trust_exact")?;
    Ok(Bond {
        a: row.get("a")?,
        b: row.get("b")?,
        trust,
        trust_exact: exact.unwrap_or(f64::from(trust)),
        mode: row.get("mode")?,
        last_seen: row.get("last_seen")?,
    })
}

const COLUMNS: &str = "a, b, trust, trust_exact, mode, last_seen";

fn default_user_bond(person: &PersonId) -> Bond {
    Bond {
        a: Actor::User,
        b: person.clone(),
        trust: USER_TRUST as u8,
        trust_exact: USER_TRUST,
        mode: Mode::default(),
        last_seen: None,
    }
}

/// The user's bond with a person; the default when there is no row yet.
pub fn user_bond(conn: &Connection, person: &PersonId) -> Result<Bond> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM bond WHERE a = 'user' AND b = ?1"),
            [person],
            read,
        )
        .optional()?
        .unwrap_or_else(|| default_user_bond(person)))
}

/// Write the default row if absent, so updates have something to update.
pub(crate) fn ensure_user_bond(conn: &Connection, person: &PersonId) -> Result<()> {
    reject_reserved(person)?;
    conn.execute(
        "INSERT INTO bond(a, b, trust, trust_exact) VALUES ('user', ?1, ?2, ?3)
         ON CONFLICT DO NOTHING",
        (person, USER_TRUST as i64, USER_TRUST),
    )?;
    Ok(())
}

fn reject_reserved(person: &PersonId) -> Result<()> {
    if matches!(person.as_str(), "user" | "host" | "") {
        return Err(Error::Invalid(format!("`{person}` is not a person id")));
    }
    Ok(())
}

pub fn mode(conn: &Connection, person: &PersonId) -> Result<Mode> {
    Ok(user_bond(conn, person)?.mode)
}

pub fn set_mode(conn: &Connection, person: &PersonId, mode: Mode) -> Result<()> {
    ensure_user_bond(conn, person)?;
    conn.execute(
        "UPDATE bond SET mode = ?2 WHERE a = 'user' AND b = ?1",
        (person, mode),
    )?;
    Ok(())
}

/// Every user bond that has a row (people never touched are implicitly default).
pub fn user_bonds(conn: &Connection) -> Result<Vec<Bond>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM bond WHERE a = 'user' ORDER BY b"
    ))?;
    let rows = stmt.query_map([], read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn with_mode(conn: &Connection, mode: Mode) -> Result<Vec<PersonId>> {
    if mode == Mode::default() {
        return Err(Error::Invalid(
            "the default mode is implicit; list the others".into(),
        ));
    }
    let mut stmt = conn.prepare("SELECT b FROM bond WHERE a = 'user' AND mode = ?1 ORDER BY b")?;
    let rows = stmt.query_map([mode], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The people who may speak unprompted.
pub fn enabled(conn: &Connection) -> Result<Vec<PersonId>> {
    with_mode(conn, Mode::Enabled)
}

/// The exclude list every candidate list starts from: retrieval (`find_people`,
/// `request_join`, addressing, interjection), roster data and seed evaluation.
pub fn disabled(conn: &Connection) -> Result<Vec<PersonId>> {
    with_mode(conn, Mode::Disabled)
}

pub fn is_disabled(conn: &Connection, person: &PersonId) -> Result<bool> {
    Ok(mode(conn, person)? == Mode::Disabled)
}

/// `persons` without the disabled ones, order kept.
pub fn filter_disabled(conn: &Connection, persons: Vec<PersonId>) -> Result<Vec<PersonId>> {
    let off = disabled(conn)?;
    Ok(persons.into_iter().filter(|p| !off.contains(p)).collect())
}

/// Err([`Error::Disabled`]) for someone who may not be triggered, asked or added.
pub fn ensure_available(conn: &Connection, person: &PersonId) -> Result<()> {
    reject_reserved(person)?;
    if is_disabled(conn, person)? {
        return Err(Error::Disabled(person.clone()));
    }
    Ok(())
}

/// The stored key order of a person pair.
pub fn pair_key(p: &PersonId, q: &PersonId) -> (PersonId, PersonId) {
    if p < q {
        (p.clone(), q.clone())
    } else {
        (q.clone(), p.clone())
    }
}

/// A person pair's bond; `None` until they have shared a turn.
pub fn pair_bond(conn: &Connection, p: &PersonId, q: &PersonId) -> Result<Option<Bond>> {
    let (a, b) = pair_key(p, q);
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM bond WHERE a = ?1 AND b = ?2"),
            (&a, &b),
            read,
        )
        .optional()?)
}

/// Every pair bond `person` is in: the acquaintances the user's play made.
pub fn pair_bonds_of(conn: &Connection, person: &PersonId) -> Result<Vec<Bond>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM bond WHERE a <> 'user' AND (a = ?1 OR b = ?1) ORDER BY a, b"
    ))?;
    let rows = stmt.query_map([person], read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn ensure_pair_bond(conn: &Connection, p: &PersonId, q: &PersonId) -> Result<()> {
    reject_reserved(p)?;
    reject_reserved(q)?;
    if p == q {
        return Err(Error::Invalid("a person has no bond with themself".into()));
    }
    let (a, b) = pair_key(p, q);
    conn.execute(
        "INSERT INTO bond(a, b, trust, trust_exact) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT DO NOTHING",
        (&a, &b, PAIR_TRUST as i64, PAIR_TRUST),
    )?;
    Ok(())
}

/// Move the user's `last_seen` with a person forward to `at` (never backwards).
pub(crate) fn touch_user(conn: &Connection, person: &PersonId, at: i64) -> Result<()> {
    ensure_user_bond(conn, person)?;
    conn.execute(
        "UPDATE bond SET last_seen = max(coalesce(last_seen, ?2), ?2)
         WHERE a = 'user' AND b = ?1",
        (person, at),
    )?;
    Ok(())
}

/// Set a bond's exact trust (clamped to 0..=200) and its rounding.
pub(crate) fn store_trust(conn: &Connection, a: &Actor, b: &PersonId, exact: f64) -> Result<()> {
    let exact = exact.clamp(0.0, 200.0);
    conn.execute(
        "UPDATE bond SET trust_exact = ?3, trust = ?4 WHERE a = ?1 AND b = ?2",
        (a, b, exact, exact.round() as i64),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    #[test]
    fn everyone_defaults_to_frozen_at_full_trust() {
        let w = world();
        let b = user_bond(&w, &p("p1")).unwrap();
        assert_eq!((b.trust, b.mode, b.last_seen), (100, Mode::Frozen, None));
        assert!(user_bonds(&w).unwrap().is_empty(), "reading writes nothing");
        assert!(enabled(&w).unwrap().is_empty());
    }

    #[test]
    fn disabled_is_filtered_everywhere() {
        let w = world();
        set_mode(&w, &p("p2"), Mode::Disabled).unwrap();
        set_mode(&w, &p("p3"), Mode::Enabled).unwrap();
        assert_eq!(disabled(&w).unwrap(), vec![p("p2")]);
        assert_eq!(enabled(&w).unwrap(), vec![p("p3")]);
        assert_eq!(
            filter_disabled(&w, vec![p("p3"), p("p2"), p("p1")]).unwrap(),
            vec![p("p3"), p("p1")]
        );
        assert!(matches!(
            ensure_available(&w, &p("p2")),
            Err(Error::Disabled(_))
        ));
        ensure_available(&w, &p("p1")).unwrap();
        assert!(ensure_available(&w, &p("host")).is_err());
        set_mode(&w, &p("p2"), Mode::Frozen).unwrap();
        assert!(disabled(&w).unwrap().is_empty());
    }

    #[test]
    fn pairs_exist_only_once_created_and_are_symmetric() {
        let w = world();
        assert_eq!(pair_bond(&w, &p("b"), &p("a")).unwrap(), None);
        ensure_pair_bond(&w, &p("b"), &p("a")).unwrap();
        let bond = pair_bond(&w, &p("a"), &p("b")).unwrap().unwrap();
        assert_eq!((bond.a.clone(), bond.b.clone(), bond.trust), (Actor::Person(p("a")), p("b"), 0));
        assert_eq!(pair_bond(&w, &p("b"), &p("a")).unwrap(), Some(bond));
        assert_eq!(pair_bonds_of(&w, &p("b")).unwrap().len(), 1);
        assert!(ensure_pair_bond(&w, &p("a"), &p("a")).is_err());
    }

    #[test]
    fn last_seen_only_moves_forward() {
        let w = world();
        touch_user(&w, &p("p1"), 50).unwrap();
        touch_user(&w, &p("p1"), 20).unwrap();
        assert_eq!(user_bond(&w, &p("p1")).unwrap().last_seen, Some(50));
    }
}
