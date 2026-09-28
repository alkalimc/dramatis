//! User settings: key -> JSON value. A missing key means the default.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::params::{QuietHours, Seed};
use crate::types::{MonthDay, PersonId, Tier};
use crate::{Error, Result};

pub const USER_NAME: &str = "user.name";
pub const USER_BIRTHDAY: &str = "user.birthday";
pub const INTENSITY: &str = "intensity";
pub const QUIET_HOURS: &str = "quiet_hours";
pub const NOTIFICATIONS: &str = "notifications";

pub fn voice_key(person: &PersonId) -> String {
    format!("voice.{person}")
}

/// The stored value, `None` when the key is absent.
pub fn get<T: DeserializeOwned>(conn: &Connection, key: &str) -> Result<Option<T>> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Error::from))
        .transpose()
}

pub fn set<T: Serialize + ?Sized>(conn: &Connection, key: &str, value: &T) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, serde_json::to_string(value)?),
    )?;
    Ok(())
}

/// Back to the default.
pub fn unset(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

pub fn user_name(conn: &Connection) -> Result<Option<String>> {
    Ok(get::<Option<String>>(conn, USER_NAME)?.flatten())
}

pub fn user_birthday(conn: &Connection) -> Result<Option<MonthDay>> {
    Ok(get::<Option<MonthDay>>(conn, USER_BIRTHDAY)?.flatten())
}

pub fn intensity(conn: &Connection) -> Result<Tier> {
    Ok(get(conn, INTENSITY)?.unwrap_or_default())
}

/// Missing key: the register default; stored `null`: the user turned quiet hours off.
pub fn quiet_hours(conn: &Connection, seed: &Seed) -> Result<Option<QuietHours>> {
    Ok(match get::<Option<QuietHours>>(conn, QUIET_HOURS)? {
        None => Some(seed.quiet_hours.clone()),
        Some(set) => set,
    })
}

pub fn notifications(conn: &Connection) -> Result<bool> {
    Ok(get(conn, NOTIFICATIONS)?.unwrap_or(true))
}

pub fn voice(conn: &Connection, person: &PersonId) -> Result<Option<String>> {
    Ok(get::<Option<String>>(conn, &voice_key(person))?.flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    #[test]
    fn defaults_and_round_trips() {
        let w = world();
        let seed = Seed::default();
        assert_eq!(user_name(&w).unwrap(), None);
        assert_eq!(intensity(&w).unwrap(), Tier::Middle);
        assert!(notifications(&w).unwrap());
        assert_eq!(quiet_hours(&w, &seed).unwrap(), Some(seed.quiet_hours.clone()));

        set(&w, USER_NAME, "someone").unwrap();
        set(&w, USER_BIRTHDAY, "02-29").unwrap();
        set(&w, INTENSITY, &Tier::Ultra).unwrap();
        set(&w, QUIET_HOURS, &None::<QuietHours>).unwrap();
        set(&w, NOTIFICATIONS, &false).unwrap();
        set(&w, &voice_key(&p("p1")), "v2").unwrap();
        assert_eq!(user_name(&w).unwrap().as_deref(), Some("someone"));
        assert_eq!(
            user_birthday(&w).unwrap(),
            Some(MonthDay { month: 2, day: 29 })
        );
        assert_eq!(intensity(&w).unwrap(), Tier::Ultra);
        assert_eq!(quiet_hours(&w, &seed).unwrap(), None);
        assert!(!notifications(&w).unwrap());
        assert_eq!(voice(&w, &p("p1")).unwrap().as_deref(), Some("v2"));

        unset(&w, QUIET_HOURS).unwrap();
        assert!(quiet_hours(&w, &seed).unwrap().is_some());
        set(&w, USER_BIRTHDAY, "13-40").unwrap();
        assert!(user_birthday(&w).is_err());
    }
}
