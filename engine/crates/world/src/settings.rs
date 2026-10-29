//! User settings: key -> JSON value. A missing key means the default.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::params::{Events, QuietHours, Seed};
use crate::types::{MonthDay, PersonId, Tier};
use crate::{Error, Result};

pub const USER_NAME: &str = "user.name";
pub const USER_BIRTHDAY: &str = "user.birthday";
pub const USER_BIO: &str = "user.bio";
/// Longest personal bio, in characters.
pub const BIO_MAX: usize = 200;
pub const INTENSITY: &str = "intensity";
pub const QUIET_HOURS: &str = "quiet_hours";
pub const NOTIFICATIONS: &str = "notifications";
/// Daily events per local day the user chose (0 = none).
pub const EVENTS_PER_DAY: &str = "events.per_day";
/// When the last daily event was tried (unix ms), made or not.
pub const EVENTS_TRIED: &str = "events.tried";

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

/// Set (or with `None` / blank, clear) the user's name. Returns whether it changed: a
/// rename means every channel must roll over its segment, since sent bytes are never
/// rewritten and the name reaches the model only through new ones.
pub fn set_user_name(conn: &Connection, name: Option<&str>) -> Result<bool> {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    if user_name(conn)?.as_deref() == name {
        return Ok(false);
    }
    match name {
        Some(n) => set(conn, USER_NAME, n)?,
        None => unset(conn, USER_NAME)?,
    }
    Ok(true)
}

pub fn user_bio(conn: &Connection) -> Result<Option<String>> {
    Ok(get::<Option<String>>(conn, USER_BIO)?.flatten())
}

/// Set (or with `None` / blank, clear) the personal bio. Returns whether it changed, which
/// like a rename means the channels' logs must roll over: it sits in the cached prefix.
pub fn set_user_bio(conn: &Connection, bio: Option<&str>) -> Result<bool> {
    let bio = bio.map(str::trim).filter(|b| !b.is_empty());
    if bio.is_some_and(|b| b.chars().count() > BIO_MAX) {
        return Err(Error::Invalid("the bio is too long".into()));
    }
    if user_bio(conn)?.as_deref() == bio {
        return Ok(false);
    }
    match bio {
        Some(b) => set(conn, USER_BIO, b)?,
        None => unset(conn, USER_BIO)?,
    }
    Ok(true)
}

/// The user's own picture: its MIME type and bytes. It lives in the save, so it is
/// encrypted with it and travels with an export.
pub fn user_avatar(conn: &Connection) -> Result<Option<(String, Vec<u8>)>> {
    Ok(conn
        .query_row("SELECT mime, data FROM user_avatar WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?)
}

pub fn set_user_avatar(conn: &Connection, image: Option<(&str, &[u8])>) -> Result<()> {
    match image {
        Some((mime, data)) => conn.execute(
            "INSERT INTO user_avatar(id, mime, data) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET mime = excluded.mime, data = excluded.data",
            (mime, data),
        )?,
        None => conn.execute("DELETE FROM user_avatar", [])?,
    };
    Ok(())
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

/// Missing key: the register default. Never above what the register allows.
pub fn events_per_day(conn: &Connection, events: &Events) -> Result<u32> {
    Ok(get::<u32>(conn, EVENTS_PER_DAY)?
        .unwrap_or(events.per_day)
        .min(events.max_per_day))
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
        assert_eq!(
            quiet_hours(&w, &seed).unwrap(),
            Some(seed.quiet_hours.clone())
        );

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

        assert!(!set_user_name(&w, Some(" someone ")).unwrap(), "same name");
        assert!(set_user_name(&w, Some("other")).unwrap());
        assert!(set_user_name(&w, Some("  ")).unwrap());
        assert_eq!(user_name(&w).unwrap(), None);
        assert!(!set_user_name(&w, None).unwrap());

        unset(&w, QUIET_HOURS).unwrap();
        assert!(quiet_hours(&w, &seed).unwrap().is_some());
        set(&w, USER_BIRTHDAY, "13-40").unwrap();
        assert!(user_birthday(&w).is_err());
    }

    #[test]
    fn bio_trims_bounds_and_reports_change() {
        let w = world();
        assert_eq!(user_bio(&w).unwrap(), None);
        assert!(set_user_bio(&w, Some("  likes tea ")).unwrap());
        assert_eq!(user_bio(&w).unwrap().as_deref(), Some("likes tea"));
        assert!(!set_user_bio(&w, Some("likes tea")).unwrap());
        let long = "x".repeat(BIO_MAX + 1);
        assert!(set_user_bio(&w, Some(&long)).is_err());
        assert!(
            set_user_bio(&w, Some(&"é".repeat(BIO_MAX))).is_ok(),
            "characters, not bytes"
        );
        assert!(set_user_bio(&w, Some(" ")).unwrap());
        assert_eq!(user_bio(&w).unwrap(), None);
        assert!(!set_user_bio(&w, None).unwrap());
    }

    #[test]
    fn avatar_blob_round_trips_and_clears() {
        let w = world();
        assert_eq!(user_avatar(&w).unwrap(), None);
        set_user_avatar(&w, Some(("image/png", &[0, 1, 2, 255]))).unwrap();
        set_user_avatar(&w, Some(("image/jpeg", &[9, 8]))).unwrap();
        assert_eq!(
            user_avatar(&w).unwrap(),
            Some(("image/jpeg".to_owned(), vec![9, 8]))
        );
        set_user_avatar(&w, None).unwrap();
        assert_eq!(user_avatar(&w).unwrap(), None);
    }
}
