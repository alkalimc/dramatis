//! The versions a save was last opened with.
//!
//! A save records the app version and the corpus pack (name and version) it was last
//! opened under, next to the schema version SQLite keeps in `user_version`. The app
//! compares them with the running ones when a save is chosen and, once the save has
//! opened without error, writes the running ones. This module only reads and writes the
//! record; deciding what a difference means is the caller's.

use std::path::Path;

use rusqlite::Connection;

use crate::{Result, cipher};

const APP: &str = "app_version";
const PACK: &str = "pack";
const PACK_VERSION: &str = "pack_version";

/// This build's version, which is the app's: one workspace version.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a save says about itself. `None`: not recorded (a save from before the record).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamp {
    /// SQLite's `user_version`: 0 for a file with no schema yet.
    pub schema: i64,
    pub app: Option<String>,
    pub pack: Option<String>,
    pub pack_version: Option<String>,
}

fn table_exists(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'save_meta'",
        [],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM save_meta WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?)
}

/// The record in `conn`; a schema older than the record reads as unrecorded.
pub fn read(conn: &Connection) -> Result<Stamp> {
    let schema: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !table_exists(conn)? {
        return Ok(Stamp {
            schema,
            ..Stamp::default()
        });
    }
    Ok(Stamp {
        schema,
        app: get(conn, APP)?,
        pack: get(conn, PACK)?,
        pack_version: get(conn, PACK_VERSION)?,
    })
}

/// The record of the file at `path`, read without migrating or writing anything. An
/// encrypted file needs its `key`; a wrong one is [`crate::Error::Locked`]. A schema newer
/// than this build's is [`crate::Error::TooNew`], as when opening.
pub fn peek(path: impl AsRef<Path>, key: Option<&str>) -> Result<Stamp> {
    let conn = cipher::connect_existing(path.as_ref(), key)?;
    let stamp = read(&conn)?;
    if stamp.schema > crate::SCHEMA_VERSION {
        return Err(crate::Error::TooNew {
            found: stamp.schema,
            supported: crate::SCHEMA_VERSION,
        });
    }
    Ok(stamp)
}

fn put(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO save_meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

/// Record the app version and, when the corpus is known, its pack name and version. The
/// pack is left as it was when `pack` is `None` (no corpus open to say).
pub fn write(conn: &Connection, app: &str, pack: Option<(&str, &str)>) -> Result<()> {
    crate::atomic(conn, |conn| {
        put(conn, APP, app)?;
        if let Some((name, version)) = pack {
            put(conn, PACK, name)?;
            put(conn, PACK_VERSION, version)?;
        }
        Ok(())
    })
}

/// The first reads after opening: every core table answers. A save that migrated but
/// cannot be read is not stamped as opened.
pub fn probe(conn: &Connection) -> Result<()> {
    for table in crate::CORE_TABLES {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        })?;
    }
    Ok(())
}

/// A short stable fingerprint of `parts` (FNV-1a, 64 bit, hex). Not a security hash: it
/// tells two builds apart. Parts are length-prefixed so `("ab", "c")` and `("a", "bc")`
/// differ.
pub fn fingerprint<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for p in parts {
        feed(&(p.len() as u64).to_le_bytes());
        feed(p.as_bytes());
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::world;

    #[test]
    fn a_new_save_carries_the_app_version_and_no_pack() {
        let w = world();
        let s = read(&w).unwrap();
        assert_eq!(s.schema, crate::SCHEMA_VERSION);
        assert_eq!(s.app.as_deref(), Some(APP_VERSION));
        assert_eq!((s.pack, s.pack_version), (None, None));
    }

    #[test]
    fn write_replaces_and_a_missing_pack_leaves_the_old_one() {
        let w = world();
        write(&w, "9.9.9", Some(("p", "2"))).unwrap();
        write(&w, "9.9.10", None).unwrap();
        let s = read(&w).unwrap();
        assert_eq!(s.app.as_deref(), Some("9.9.10"));
        assert_eq!(s.pack.as_deref(), Some("p"));
        assert_eq!(s.pack_version.as_deref(), Some("2"));
    }

    #[test]
    fn probe_reads_every_core_table() {
        probe(&world()).unwrap();
    }

    #[test]
    fn fingerprints_are_stable_and_separate_their_parts() {
        assert_eq!(fingerprint(["a", "b"]), fingerprint(["a", "b"]));
        assert_ne!(fingerprint(["ab", "c"]), fingerprint(["a", "bc"]));
        assert_eq!(fingerprint(["x"]).len(), 16);
    }

    #[test]
    fn peek_reads_a_file_without_changing_it() {
        let dir = std::env::temp_dir().join(format!("world-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.sqlite");
        drop(crate::World::open(&path).unwrap());
        let before = std::fs::read(&path).unwrap();
        let s = peek(&path, None).unwrap();
        assert_eq!(s.app.as_deref(), Some(APP_VERSION));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // A newer schema is refused as on open.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", crate::SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        assert!(matches!(
            peek(&path, None),
            Err(crate::Error::TooNew { .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
