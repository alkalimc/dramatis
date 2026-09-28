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
pub mod clock;
pub mod fact;
pub mod message;
pub mod params;
pub mod presence;
pub mod quota;
pub mod seed;
pub mod log;
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
    NotParticipant { person: PersonId, channel: ChannelId },

    /// `request_join` asked for more turns than the caller can hand over while keeping
    /// the one it is spending now.
    #[error("cannot hand over {asked} turns: only {spare} to spare")]
    NotEnoughTurns { asked: u32, spare: u32 },

    /// `report` (or a forced close) from someone with no such active request.
    #[error("no active request {task} for `{person}`")]
    NoTask { person: PersonId, task: TaskId },

    #[error("invalid: {0}")]
    Invalid(String),
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
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
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
}
