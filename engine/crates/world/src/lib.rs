//! The world database: everything that changes with play.
//!
//! One SQLite file holds the narrative state (bonds, channels, messages, memory),
//! the agent's append-only session logs and the usage meter, so a turn's writes commit
//! together. This crate never calls a model: where one is needed it returns a [`Call`]
//! value for the caller to execute.
//!
//! Every function takes a `&Connection`, so it works the same on a [`World`] (which
//! derefs to one) and inside a caller's transaction. Time is always passed in
//! ([`clock::Now`]); nothing here reads the wall clock.

pub mod away;
pub mod bond;
pub mod channel;
pub mod cipher;
pub mod clock;
pub mod event;
pub mod fact;
pub mod leakcheck;
pub mod log;
pub mod message;
pub mod meta;
pub mod params;
pub mod picture;
pub mod presence;
pub mod quota;
pub mod rewind;
pub mod seed;
pub mod session;
pub mod settings;
pub mod trust;
pub mod types;
pub mod wrapup;

use std::ops::{Deref, DerefMut};
use std::path::Path;

use rusqlite::Connection;

pub use types::*;

/// The layout of a new save: every table, at [`SCHEMA_VERSION`].
pub const SCHEMA: &str = include_str!("../schema.sql");

/// The one schema version this build opens. A save at another version is refused, not
/// migrated: bringing an older save up to date is the job of `dramatis-migrate`, which
/// keeps the steps from every released layout to this one. Raising this number means
/// editing `schema.sql` and adding the step to that tool.
pub const SCHEMA_VERSION: i64 = 9;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Written by a newer build. Refused rather than opened, since this build would read
    /// tables whose meaning it does not know.
    #[error("world database schema {found} is newer than this build ({supported})")]
    TooNew { found: i64, supported: i64 },

    /// Written by an earlier release. Refused rather than opened: this build reads only its
    /// own layout, and `dramatis-migrate` brings an older save up to it.
    #[error(
        "world database schema {found} is older than this build ({supported}): it needs migration, run dramatis-migrate"
    )]
    Old { found: i64, supported: i64 },

    #[error("{kind} `{id}` does not exist")]
    NotFound { kind: &'static str, id: String },

    /// A disabled person cannot be triggered, asked, joined or added anywhere.
    #[error("person `{0}` is disabled")]
    Disabled(PersonId),

    /// A disabled or deleted group receives nothing.
    #[error("channel {0} does not accept messages")]
    ChannelClosed(ChannelId),

    #[error("`{person}` is not a participant of channel {channel}")]
    NotParticipant {
        person: PersonId,
        channel: ChannelId,
    },

    #[error("invalid: {0}")]
    Invalid(String),

    /// The file is encrypted and no key, or a wrong one, was given.
    #[error("the database is locked: password missing or wrong")]
    Locked,
}

pub type Result<T> = std::result::Result<T, Error>;

/// A model call `world` decided is needed. The caller (the agent) executes it; `world`
/// only records the outcome it is then told about.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// One person speaks first, with a seed ([`seed::Opening`]).
    Opening(seed::Opening),
    /// The host says one in-world line about a non-empty digest.
    HostLine(presence::Digest),
    /// A conversation ended (or its segment rolls): run the wrap-up turn on it.
    Wrapup { channel: ChannelId },
}

/// An open world database. Derefs to its [`Connection`], which is what every function in
/// this crate takes.
#[derive(Debug)]
pub struct World {
    conn: Connection,
}

impl World {
    /// Open, creating a new save if the file is absent or empty. A save of another schema
    /// version is refused ([`Error::Old`], [`Error::TooNew`]).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, None)
    }

    /// [`World::open`] on an encrypted file: `key` is the password. Without one the file
    /// is a plain SQLite file. A missing or wrong password is [`Error::Locked`].
    pub fn open_with(path: impl AsRef<Path>, key: Option<&str>) -> Result<Self> {
        let mut conn = cipher::connect(path.as_ref(), key)?;
        // One file at rest: a rollback journal exists only while a write is in flight, so
        // a save is never a file plus a log. A file written in WAL mode is converted; if
        // another process still holds it open that waits or keeps WAL, and either is safe.
        let _ = conn.pragma_update_and_check(None, "journal_mode", "DELETE", |_| Ok(()));
        setup(&mut conn)?;
        Ok(Self { conn })
    }

    /// A fresh database in memory, for tests and previews.
    pub fn in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        setup(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn into_inner(self) -> Connection {
        self.conn
    }
}

impl Deref for World {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl DerefMut for World {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

/// Run `f` all-or-nothing: if it fails, none of its writes stay. Outside a transaction
/// this takes the write lock up front (so a read-then-write function cannot fail midway
/// on a stale snapshot) and commits at the end; inside one, such as a caller's or
/// [`rewind`]'s, it is a savepoint that rolls back alone and leaves the outer
/// transaction's decision to the caller.
pub fn atomic<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let outer = conn.is_autocommit();
    conn.execute_batch(if outer {
        "BEGIN IMMEDIATE"
    } else {
        "SAVEPOINT atomic"
    })?;
    match f(conn) {
        Ok(v) => {
            let end = if outer { "COMMIT" } else { "RELEASE atomic" };
            if let Err(e) = conn.execute_batch(end) {
                undo(conn, outer);
                return Err(e.into());
            }
            Ok(v)
        }
        Err(e) => {
            undo(conn, outer);
            Err(e)
        }
    }
}

fn undo(conn: &Connection, outer: bool) {
    // Best effort: the original error is what the caller needs to see.
    let _ = conn.execute_batch(if outer {
        "ROLLBACK"
    } else {
        "ROLLBACK TO atomic; RELEASE atomic"
    });
}

/// [`World::open`], as a bare connection.
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    World::open(path).map(World::into_inner)
}

/// Enable foreign keys and make sure the database is at [`SCHEMA_VERSION`]: a file with no
/// tables yet gets the whole schema in one transaction with its version; one at any other
/// version is refused.
fn setup(conn: &mut Connection) -> Result<()> {
    // Per connection, and a no-op inside a transaction, so it is set before any.
    conn.pragma_update(None, "foreign_keys", true)?;
    let found: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match found {
        SCHEMA_VERSION => Ok(()),
        0 => {
            let tables: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            if tables > 0 {
                return Err(Error::Invalid("not a world database".into()));
            }
            let tx = conn.transaction()?;
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
            // A save made by this build says so; the pack is added when a corpus opens it.
            meta::write(conn, meta::APP_VERSION, None)
        }
        found if found > SCHEMA_VERSION => Err(Error::TooNew {
            found,
            supported: SCHEMA_VERSION,
        }),
        found => Err(Error::Old {
            found,
            supported: SCHEMA_VERSION,
        }),
    }
}

/// Tables every schema version has; a file lacking one is not a world database.
pub const CORE_TABLES: &[&str] = &[
    "bond",
    "channel",
    "channel_participant",
    "message",
    "fact",
    "log_segment",
    "log_entry",
    "meter",
    "settings",
];

/// Whether `path` is a world database this build can open: readable, intact, with the core
/// tables and exactly this build's schema version. Nothing is written.
pub fn check_file(path: impl AsRef<Path>, key: Option<&str>) -> Result<()> {
    let invalid = |why: &str| Error::Invalid(why.to_owned());
    let conn = cipher::connect_existing(path.as_ref(), key).map_err(|e| match e {
        Error::Locked if cipher::is_encrypted(path.as_ref()) => Error::Locked,
        _ => invalid("not a readable database"),
    })?;
    let quick: String = conn
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|_| invalid("not a database"))?;
    if quick != "ok" {
        return Err(invalid("the database is damaged"));
    }
    let found: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let present: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ({})",
            CORE_TABLES
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        [],
        |row| row.get(0),
    )?;
    if found < 1 || present != CORE_TABLES.len() as i64 {
        return Err(invalid("not a world database"));
    }
    match found {
        SCHEMA_VERSION => Ok(()),
        found if found > SCHEMA_VERSION => Err(Error::TooNew {
            found,
            supported: SCHEMA_VERSION,
        }),
        found => Err(Error::Old {
            found,
            supported: SCHEMA_VERSION,
        }),
    }
}

/// A consistent single-file copy of the database at `src` (`VACUUM INTO`, so a WAL that
/// has not been checkpointed is included and the copy needs no side files), encrypted
/// under the same `key` as the source. `dest` must not exist.
pub fn copy_file(src: impl AsRef<Path>, dest: impl AsRef<Path>, key: Option<&str>) -> Result<()> {
    let conn = cipher::connect_existing(src.as_ref(), key)?;
    cipher::export_to(&conn, dest.as_ref(), key)
}

pub(crate) fn not_found(kind: &'static str, id: impl ToString) -> Error {
    Error::NotFound {
        kind,
        id: id.to_string(),
    }
}

/// Shared by the unit tests of every module.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn world() -> World {
        World::in_memory().unwrap()
    }

    pub fn p(id: &str) -> PersonId {
        PersonId::from(id)
    }

    pub const HOUR: i64 = 3_600_000;
    pub const DAY: i64 = 24 * HOUR;

    /// 2026-03-10 12:00 UTC, a Tuesday; tests use UTC so local = UTC.
    pub const T0: i64 = 1_773_144_000_000;

    pub fn at(ms: i64) -> clock::Now {
        clock::Now::utc(ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: &[&str] = &[
        "bond",
        "channel",
        "channel_participant",
        "message",
        "fact",
        "log_segment",
        "log_entry",
        "meter",
        "settings",
        "seed",
        "session",
        "user_avatar",
        "picture",
        "save_meta",
        "channel_summary",
        "daily_event",
    ];

    fn created() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        setup(&mut conn).unwrap();
        conn
    }

    fn version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn a_new_save_has_every_table_at_the_latest_schema() {
        let conn = created();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        for table in TABLES {
            let strict: i64 = conn
                .query_row(
                    "SELECT strict FROM pragma_table_list WHERE name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| panic!("table {table} is missing"));
            assert_eq!(strict, 1, "{table} is STRICT");
        }
        let all: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_list WHERE schema = 'main' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(all, TABLES.len() as i64, "TABLES lists every table");
        let s = meta::read(&conn).unwrap();
        assert_eq!(s.app.as_deref(), Some(meta::APP_VERSION));
        assert_eq!((s.pack, s.pack_version), (None, None));
    }

    #[test]
    fn the_core_tables_are_in_the_schema() {
        let conn = created();
        for table in CORE_TABLES {
            assert!(TABLES.contains(table), "{table}");
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        }
    }

    /// Any other version is refused with its own kind, and nothing is changed.
    #[test]
    fn a_save_of_another_schema_is_refused_untouched() {
        for (found, older) in [
            (1, true),
            (7, true),
            (SCHEMA_VERSION - 1, true),
            (SCHEMA_VERSION + 1, false),
        ] {
            let mut conn = Connection::open_in_memory().unwrap();
            conn.execute_batch("CREATE TABLE marker(x);").unwrap();
            conn.pragma_update(None, "user_version", found).unwrap();
            match setup(&mut conn) {
                Err(Error::Old {
                    found: f,
                    supported,
                }) if older => {
                    assert_eq!((f, supported), (found, SCHEMA_VERSION));
                }
                Err(Error::TooNew {
                    found: f,
                    supported,
                }) if !older => {
                    assert_eq!((f, supported), (found, SCHEMA_VERSION));
                }
                other => panic!("schema {found}: {other:?}"),
            }
            assert_eq!(version(&conn), found);
            let tables: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tables, 1, "no table was added");
        }
        let text = Error::Old {
            found: 3,
            supported: SCHEMA_VERSION,
        }
        .to_string();
        assert!(text.contains("dramatis-migrate"));
    }

    #[test]
    fn a_file_with_tables_and_no_version_is_not_a_world() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t(x);").unwrap();
        assert!(matches!(setup(&mut conn), Err(Error::Invalid(_))));
    }

    #[test]
    fn setting_up_again_is_a_no_op() {
        let mut conn = created();
        conn.execute(
            "INSERT INTO settings VALUES ('user.name', '\"someone\"')",
            [],
        )
        .unwrap();
        setup(&mut conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    fn setting_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM settings", [], |r| r.get(0))
            .unwrap()
    }

    fn put(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("INSERT INTO settings VALUES (?1, '1')", [key])?;
        Ok(())
    }

    fn fail_with(conn: &Connection) -> Result<()> {
        put(conn, "inner")?;
        Err(Error::Invalid("boom".into()))
    }

    #[test]
    fn atomic_commits_and_rolls_back_outside_a_transaction() {
        let w = World::in_memory().unwrap();
        atomic(&w, |c| put(c, "a")).unwrap();
        assert!(atomic(&w, fail_with).is_err());
        assert_eq!(setting_count(&w), 1);
        assert!(w.is_autocommit(), "no transaction left open");
    }

    #[test]
    fn an_inner_atomic_failure_rolls_back_only_itself() {
        let w = World::in_memory().unwrap();
        atomic(&w, |c| {
            put(c, "outer-before")?;
            assert!(atomic(c, fail_with).is_err());
            // A nested success is kept, and a failure two deep undoes only the deepest.
            atomic(c, |c| {
                put(c, "mid")?;
                assert!(atomic(c, fail_with).is_err());
                Ok(())
            })?;
            put(c, "outer-after")?;
            Ok(())
        })
        .unwrap();
        let keys: Vec<String> = w
            .prepare("SELECT key FROM settings ORDER BY key")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(keys, ["mid", "outer-after", "outer-before"]);
    }

    #[test]
    fn an_outer_failure_undoes_a_committed_inner_atomic() {
        let w = World::in_memory().unwrap();
        let r: Result<()> = atomic(&w, |c| {
            atomic(c, |c| put(c, "inner-ok"))?;
            Err(Error::Invalid("outer".into()))
        });
        assert!(r.is_err());
        assert_eq!(setting_count(&w), 0);
        assert!(w.is_autocommit());
    }

    #[test]
    fn atomic_inside_a_caller_transaction_leaves_commit_to_the_caller() {
        let w = World::in_memory().unwrap();
        let tx = w.unchecked_transaction().unwrap();
        atomic(&w, |c| put(c, "kept")).unwrap();
        assert!(atomic(&w, fail_with).is_err());
        drop(tx); // rolls back the caller's transaction
        assert_eq!(setting_count(&w), 0);
    }

    #[test]
    fn constraints_reject_malformed_rows() {
        let conn = created();
        conn.execute_batch(
            "INSERT INTO channel(id, kind, origin) VALUES (1, 'direct', 'user');
             INSERT INTO bond(a, b, trust) VALUES ('user', 'p1', 100);
             INSERT INTO fact(author, audience_kind, audience_ref, kind, text, created_at)
                 VALUES ('user', 'participants', '1', 'fact', 't', 0);",
        )
        .unwrap();
        for bad in [
            "INSERT INTO bond(a, b, trust) VALUES ('user', 'p2', 201)",
            "INSERT INTO bond(a, b, trust) VALUES ('p2', 'p1', 0)",
            "INSERT INTO bond(a, b, trust, mode) VALUES ('user', 'p3', 100, 'asleep')",
            "INSERT INTO channel(kind, origin) VALUES ('forum', 'user')",
            "INSERT INTO message(channel, author, text, at) VALUES (9, 'user', 't', 0)",
            "INSERT INTO fact(author, audience_kind, kind, text, created_at)
                 VALUES ('user', 'self', 'fact', 't', 0)",
            "INSERT INTO fact(author, audience_kind, kind, text, created_at)
                 VALUES ('user', 'world', 'rumour', 't', 0)",
            "INSERT INTO settings VALUES ('user.name', 'not json')",
            "INSERT INTO seed(person, kind, ref, created_at) VALUES ('p1', 'voice', '1', 0)",
            "INSERT INTO session(channel, person, cause, opened_at, last_at)
                 VALUES (1, 'p1', 'whim', 0, 0)",
        ] {
            assert!(conn.execute(bad, []).is_err(), "accepted: {bad}");
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("world-file-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_file_carries_uncheckpointed_rows_and_check_file_accepts_it() {
        let dir = scratch("copy");
        let src = dir.join("a.sqlite");
        let w = World::open(&src).unwrap();
        w.execute("INSERT INTO settings VALUES ('user.name', '\"Doc\"')", [])
            .unwrap();
        // `w` stays open, so the row may still sit in the WAL.
        let dest = dir.join("b.sqlite");
        copy_file(&src, &dest, None).unwrap();
        assert!(copy_file(&src, &dest, None).is_err(), "never overwrites");
        check_file(&dest, None).unwrap();
        let copy = Connection::open(&dest).unwrap();
        let n: i64 = copy
            .query_row(
                "SELECT count(*) FROM settings WHERE key = 'user.name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        drop(copy);
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn check_file_refuses_what_is_not_a_world() {
        let dir = scratch("check");
        let other = dir.join("other.sqlite");
        let c = Connection::open(&other).unwrap();
        c.execute_batch("CREATE TABLE t(x); PRAGMA user_version = 1;")
            .unwrap();
        drop(c);
        assert!(matches!(check_file(&other, None), Err(Error::Invalid(_))));

        let text = dir.join("text.sqlite");
        std::fs::write(&text, b"just some text, not a database at all").unwrap();
        assert!(check_file(&text, None).is_err());
        assert!(check_file(dir.join("absent.sqlite"), None).is_err());

        let newer = dir.join("newer.sqlite");
        drop(World::open(&newer).unwrap());
        let c = Connection::open(&newer).unwrap();
        c.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(c);
        assert!(matches!(
            check_file(&newer, None),
            Err(Error::TooNew { .. })
        ));

        // An older schema is refused too: it needs the migration tool.
        let old = dir.join("old.sqlite");
        let c = Connection::open(&old).unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.pragma_update(None, "user_version", 8).unwrap();
        drop(c);
        assert!(matches!(
            check_file(&old, None),
            Err(Error::Old { found: 8, .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
