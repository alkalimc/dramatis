//! The world database: everything that changes with play.
//!
//! One SQLite file holds the narrative state (bonds, channels, messages, tasks, memory),
//! the agent's append-only session logs and the usage meter, so a turn's writes commit
//! together. This crate never calls a model.

pub mod params;

use std::path::Path;

use rusqlite::Connection;

/// Schema migrations, applied in order. Entry `i` upgrades `user_version` `i` to `i + 1`;
/// released entries are never edited, only appended to.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001.sql")];

/// The schema version a fully migrated database reports.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Written by a newer build. Refused rather than opened, since this build would read
    /// tables whose meaning it does not know.
    #[error("world database schema {found} is newer than this build ({supported})")]
    TooNew { found: i64, supported: i64 },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Open (creating if absent) and migrate to the latest schema.
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
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
        ] {
            assert!(conn.execute(bad, []).is_err(), "accepted: {bad}");
        }
    }
}
