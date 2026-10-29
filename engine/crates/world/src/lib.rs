//! The world database: everything that changes with play.
//!
//! One SQLite file holds the narrative state (bonds, channels, messages, tasks, memory),
//! the agent's append-only session logs and the usage meter, so a turn's writes commit
//! together. This crate never calls a model: where one is needed it returns a [`Call`]
//! value for the caller to execute.
//!
//! Every function takes a `&Connection`, so it works the same on a [`World`] (which
//! derefs to one) and inside a caller's transaction. Time is always passed in
//! ([`clock::Now`]); nothing here reads the wall clock.

pub mod bond;
pub mod channel;
pub mod cipher;
pub mod clock;
pub mod fact;
pub mod leakcheck;
pub mod log;
pub mod message;
pub mod params;
pub mod picture;
pub mod presence;
pub mod quota;
pub mod rewind;
pub mod seed;
pub mod session;
pub mod settings;
pub mod task;
pub mod trust;
pub mod types;
pub mod wrapup;

use std::ops::{Deref, DerefMut};
use std::path::Path;

use rusqlite::Connection;

pub use types::*;

/// Schema migrations, applied in order. Entry `i` upgrades `user_version` `i` to `i + 1`;
/// released entries are never edited, only appended to.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001.sql"),
    include_str!("../migrations/0002.sql"),
    include_str!("../migrations/0003.sql"),
    include_str!("../migrations/0004.sql"),
    include_str!("../migrations/0005.sql"),
    include_str!("../migrations/0006.sql"),
    include_str!("../migrations/0007.sql"),
    include_str!("../migrations/0008.sql"),
];

/// The schema version a fully migrated database reports.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

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

    /// `request_join` asked for more turns than the caller can hand over while keeping
    /// the one it is spending now.
    #[error("cannot hand over {asked} turns: only {spare} to spare")]
    NotEnoughTurns { asked: u32, spare: u32 },

    /// `report` (or a forced close) from someone with no such active request.
    #[error("no active request {task} for `{person}`")]
    NoTask { person: PersonId, task: TaskId },

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
    /// Open (creating if absent) and migrate to the latest schema.
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
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// A fresh migrated database in memory, for tests and previews.
    pub fn in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        migrate(&mut conn)?;
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

/// Open (creating if absent) and migrate to the latest schema.
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    World::open(path).map(World::into_inner)
}

/// Enable foreign keys and apply every migration above the database's `user_version`,
/// each in its own transaction together with its version bump.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    // Per connection, and a no-op inside a transaction, so it is set before any.
    conn.pragma_update(None, "foreign_keys", true)?;
    let found: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(Error::TooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// Tables every schema version has; a file lacking one is not a world database.
const CORE_TABLES: &[&str] = &[
    "bond",
    "channel",
    "channel_participant",
    "message",
    "task",
    "fact",
    "log_segment",
    "log_entry",
    "meter",
    "settings",
];

/// Whether `path` is a world database this build can migrate: readable, intact, with the
/// core tables and a schema version no newer than this build's. Nothing is written.
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
    if found > SCHEMA_VERSION {
        return Err(Error::TooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
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
    Ok(())
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
        "task",
        "fact",
        "log_segment",
        "log_entry",
        "meter",
        "settings",
        "seed",
        "session",
        "user_avatar",
        "picture",
    ];

    fn migrated() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    fn version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migrates_empty_to_latest_with_every_table() {
        let conn = migrated();
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
    }

    /// A database written at schema 1 with rows in every table upgrades in place, keeps
    /// its rows, and reads back through this crate with the new columns defaulted.
    #[test]
    fn migrates_schema_1_with_data_to_latest() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO channel(id, kind, origin) VALUES (1, 'direct', 'user');
             INSERT INTO channel_participant VALUES (1, 'p1');
             INSERT INTO bond(a, b, trust, mode) VALUES ('user', 'p1', 120, 'enabled');
             INSERT INTO message(channel, author, text, at) VALUES (1, 'p1', 'hi', 5);
             INSERT INTO task(question, assignee, turns_left, channel) VALUES ('q', 'p1', 3, 1);
             INSERT INTO fact(author, audience_kind, audience_ref, kind, text, created_at)
                 VALUES ('user', 'participants', '1', 'fact', 't', 0);
             INSERT INTO meter(at, shape, model, uncached, cached, output, points)
                 VALUES (1, 'direct', 'm', 1, 0, 0, 1.0);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);

        let b = bond::user_bond(&conn, &PersonId::from("p1")).unwrap();
        assert_eq!((b.trust, b.mode, b.last_seen), (120, Mode::Enabled, None));
        assert_eq!(b.trust_exact, 120.0);
        let t = task::get(&conn, TaskId(1)).unwrap();
        assert_eq!((t.turns_left, t.granted, t.created_at), (3, None, None));
        // An old task without a grant is its own tree; its budget is what it holds.
        assert_eq!(task::tree_budget(&conn, TaskId(1)).unwrap(), 3);
        assert_eq!(fact::all(&conn, true).unwrap().len(), 1);
        let ch = channel::get(&conn, ChannelId(1)).unwrap();
        assert_eq!(ch.person(), Some(&PersonId::from("p1")));
        assert_eq!(message::history(&conn, ch.id, None, 10).unwrap().len(), 1);
    }

    /// A schema-2 meter keeps its rows; they read as token-billed (no requests).
    #[test]
    fn migrates_schema_2_meter_rows_as_token_billed() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch(MIGRATIONS[1]).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute_batch(
            "INSERT INTO meter(at, shape, model, uncached, cached, output, points)
                 VALUES (1, 'direct', 'm', 5, 0, 0, 5.0);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        let (pts, req): (f64, i64) = conn
            .query_row("SELECT points, requests FROM meter", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((pts, req), (5.0, 0));
        let st = quota::status(&conn, Tier::Middle, 2, &params::Quota::default()).unwrap();
        assert_eq!(st.band, quota::Band::Open);
    }

    /// A schema-3 meter keeps its rows; both kinds count in calls from the first status.
    #[test]
    fn schema_3_meter_rows_count_as_calls() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for m in &MIGRATIONS[..3] {
            conn.execute_batch(m).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute_batch(
            "INSERT INTO meter(at, shape, model, uncached, cached, output, points, requests)
                 VALUES (1, 'direct', 'm', 6000, 0, 0, 6000.0, 0),
                        (2, 'direct', 'r', 0, 0, 0, 0.0, 1);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let st = quota::status(&conn, Tier::Middle, 3, &params::Quota::default()).unwrap();
        assert_eq!(st.window.unwrap().calls, 3.0);
    }

    /// Every step from every older version keeps the rows and defaults the new columns.
    #[test]
    fn every_older_schema_with_data_migrates_to_latest() {
        for from in 1..SCHEMA_VERSION {
            let mut conn = Connection::open_in_memory().unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            for m in &MIGRATIONS[..from as usize] {
                conn.execute_batch(m).unwrap();
            }
            conn.pragma_update(None, "user_version", from).unwrap();
            conn.execute_batch(
                "INSERT INTO channel(id, kind, origin) VALUES (1, 'direct', 'user');
                 INSERT INTO channel_participant VALUES (1, 'p1');
                 INSERT INTO bond(a, b, trust, mode) VALUES ('user', 'p1', 120, 'enabled');
                 INSERT INTO message(channel, author, text, at) VALUES (1, 'p1', 'hi', 5);
                 INSERT INTO task(question, assignee, turns_left, channel)
                     VALUES ('q', 'p1', 3, 1);
                 INSERT INTO fact(author, audience_kind, audience_ref, kind, text, created_at)
                     VALUES ('user', 'participants', '1', 'fact', 't', 0);
                 INSERT INTO meter(at, shape, model, uncached, cached, output, points)
                     VALUES (1, 'direct', 'm', 1, 0, 0, 1.0);
                 INSERT INTO settings VALUES ('user.name', '\"someone\"');",
            )
            .unwrap();
            if from >= 2 {
                conn.execute_batch(
                    "INSERT INTO seed(person, kind, ref, created_at, material)
                         VALUES ('p1', 'memory', '1', 0, '[\"c\"]');
                     INSERT INTO session(channel, person, cause, opened_at, last_at)
                         VALUES (1, 'p1', 'addressed', 1, 1);",
                )
                .unwrap();
            }
            migrate(&mut conn).unwrap();
            assert_eq!(version(&conn), SCHEMA_VERSION, "from {from}");
            let count = |t: &str| -> i64 {
                conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                    .unwrap()
            };
            for t in [
                "channel", "bond", "message", "task", "fact", "meter", "settings",
            ] {
                assert_eq!(count(t), 1, "{t} from {from}");
            }
            if from >= 2 {
                assert_eq!((count("seed"), count("session")), (1, 1), "from {from}");
            }
            let ch = channel::get(&conn, ChannelId(1)).unwrap();
            assert_eq!((ch.name, ch.avatar, ch.archived), (None, None, false));
            let t = task::get(&conn, TaskId(1)).unwrap();
            assert_eq!((t.failure, t.failure_told), (None, false));
            let requests: i64 = conn
                .query_row("SELECT requests FROM meter", [], |r| r.get(0))
                .unwrap();
            assert_eq!(requests, 0);
            assert_eq!(
                message::history(&conn, ChannelId(1), None, 10)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    /// A migration that fails halfway leaves the database at its old version, untouched.
    #[test]
    fn a_failed_step_leaves_the_old_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        for m in &MIGRATIONS[..4] {
            conn.execute_batch(m).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        // Step 5 adds `name`; a clashing column makes it fail after the statement ran.
        conn.execute_batch("ALTER TABLE channel ADD COLUMN avatar TEXT;")
            .unwrap();
        assert!(migrate(&mut conn).is_err());
        assert_eq!(version(&conn), 4);
        let has_name: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('channel') WHERE name = 'name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_name, 0, "the step rolled back whole");
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
    fn migrating_again_is_a_no_op() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO settings VALUES ('user.name', '\"someone\"')",
            [],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn refuses_a_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(migrate(&mut conn), Err(Error::TooNew { .. })));
    }

    #[test]
    fn constraints_reject_malformed_rows() {
        let conn = migrated();
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
            "INSERT INTO task(question, turns_left, cites) VALUES ('q', 1, '{}')",
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

        // An older schema is accepted: opening it migrates.
        let old = dir.join("old.sqlite");
        let c = Connection::open(&old).unwrap();
        c.execute_batch(MIGRATIONS[0]).unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
        drop(c);
        check_file(&old, None).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
