//! The request family (1.0.0 to 1.0.3) into the current layout.
//!
//! Those releases kept delegated requests in a `task` table and numbered their schema
//! on their own, so the additive steps do not apply. Every table is instead rebuilt from
//! the current layout, copying the columns both sides have by name. What the current
//! layout has no place for is converted or left out, as `NOTE` says.

use rusqlite::Connection;

pub const NOTE: &str = "request model -> current layout: every table is rebuilt from the current layout and the shared columns copied; conclusion memories become plain memories, openings and sessions of the removed request kinds are dropped, the request table goes";

/// What a rebuild changed beyond copying.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Converted {
    /// Memories of the removed `conclusion` kind, now plain memories.
    pub conclusions: i64,
    /// Meter rows of the removed `ask` call shape, now `direct`.
    pub asks: i64,
}

fn names(conn: &Connection, kind: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare(&format!(
        "SELECT name FROM sqlite_master WHERE type = '{kind}' AND name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY name"
    ))?
    .query_map([], |r| r.get(0))?
    .collect()
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
        .query_map([], |r| r.get(0))?
        .collect()
}

/// The value a column takes on its way across, where the current layout reads it otherwise.
fn value(table: &str, column: &str) -> String {
    match (table, column) {
        ("fact", "kind") => "CASE kind WHEN 'conclusion' THEN 'fact' ELSE kind END".into(),
        ("fact", "about_kind") => {
            "CASE about_kind WHEN 'task' THEN NULL ELSE about_kind END".into()
        }
        ("fact", "about_ref") => {
            "CASE WHEN about_kind = 'task' THEN NULL ELSE about_ref END".into()
        }
        ("meter", "shape") => "CASE shape WHEN 'ask' THEN 'direct' ELSE shape END".into(),
        // The model log is not carried at all, so no channel points into it.
        ("channel", "log_segment") => "NULL".into(),
        _ => format!("\"{column}\""),
    }
}

/// Which rows of the old table the current layout can hold.
fn filter(table: &str) -> &'static str {
    match table {
        "seed" => " WHERE kind IN ('commitment', 'birthday', 'memory', 'words', 'event', 'visit')",
        "session" => " WHERE cause IN ('addressed', 'join', 'commitment', 'opening')",
        _ => "",
    }
}

/// Rebuild the whole database in place, in one transaction, and give it the current
/// schema version. Foreign keys are off for the copy (the caller checks them after).
pub fn rebuild(conn: &mut Connection) -> rusqlite::Result<Converted> {
    conn.pragma_update(None, "foreign_keys", false)?;
    conn.execute_batch("PRAGMA legacy_alter_table = ON")?;
    let tx = conn.transaction()?;
    let old = names(&tx, "table")?;
    for index in names(&tx, "index")? {
        tx.execute_batch(&format!("DROP INDEX \"{index}\""))?;
    }
    let count = |sql: &str| tx.query_row(sql, [], |r| r.get::<_, i64>(0));
    let converted = Converted {
        conclusions: count("SELECT count(*) FROM fact WHERE kind = 'conclusion'")?,
        asks: count("SELECT count(*) FROM meter WHERE shape = 'ask'")?,
    };
    for table in &old {
        tx.execute_batch(&format!(
            "ALTER TABLE \"{table}\" RENAME TO \"old_{table}\""
        ))?;
    }
    tx.execute_batch(world::SCHEMA)?;
    for table in names(&tx, "table")?
        .iter()
        .filter(|t| !t.starts_with("old_"))
    {
        // The model log is cache, not content: it does not cross.
        if table == "log_segment" || table == "log_entry" || !old.contains(table) {
            continue;
        }
        let have = columns(&tx, &format!("old_{table}"))?;
        let shared: Vec<String> = columns(&tx, table)?
            .into_iter()
            .filter(|c| have.contains(c))
            .collect();
        let into = shared
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let from = shared
            .iter()
            .map(|c| value(table, c))
            .collect::<Vec<_>>()
            .join(", ");
        tx.execute_batch(&format!(
            "INSERT INTO \"{table}\" ({into}) SELECT {from} FROM \"old_{table}\"{}",
            filter(table)
        ))?;
    }
    for table in &old {
        tx.execute_batch(&format!("DROP TABLE \"old_{table}\""))?;
    }
    tx.pragma_update(None, "user_version", world::SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(converted)
}
