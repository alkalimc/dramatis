//! This release's rules for what a save holds, applied after the layout is current.
//!
//! A save holds the chat records and what was generated from them. Prompts and the
//! context built from the pack change with every build, so none of it is kept: the app
//! rebuilds it from code and pack. The table below is the whole policy; the tool prints
//! it in `--help` and a test checks that it names every table of the current layout.

use std::collections::BTreeSet;

use rusqlite::Connection;

/// The keep / convert / drop table, per kind of content, the same for every source format.
pub const POLICY: &str = "\
Kept as they are:
  message          the chat records (text, quotes, tool calls, attachments)
  channel, channel_participant
                   conversations and who is in them
  fact             memories, commitments and hurts, with their audience
  channel_summary  the summaries a conversation roll-over wrote
  daily_event, seed, session
                   daily events, the reasons people spoke first, open sessions
  bond             relationships and trust
  settings         the keys the app still reads (name, birthday, bio, intensity, quiet
                   hours, notifications, events per day, away, announced keys, voices)
  picture, user_avatar
                   pictures
  meter            the usage meter
Converted:
  every table of an older layout gets the columns later releases added, with defaults
  (schema steps 1 to 9); the request model of 1.0.0 to 1.0.3 is rebuilt into the
  current layout: conclusion memories become plain memories, call shape ask becomes direct
  save_meta        rewritten: app version of this tool, pack name and version with --folio
Dropped:
  log_segment, log_entry
                   the stored model log: prompt blocks, request shape, sent and received
                   bytes (the rows go; the app rebuilds a log from messages, summaries
                   and memories)
  channel.log_segment
                   the pointer into that log
  task             the removed request model (1.0.0 to 1.0.3), with the openings
                   (conclusion, pinned) and sessions (ask) that belonged to it
  settings keys    any key the app does not read
  save_meta keys   any key other than app_version, pack, pack_version
  any other table the current layout does not have
";

/// What the rules took out.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub log_segments: i64,
    pub log_entries: i64,
    pub settings: Vec<String>,
    pub meta: Vec<String>,
    /// Tables the current layout does not have, with their row counts.
    pub tables: Vec<(String, i64)>,
}

const META_KEYS: [&str; 3] = ["app_version", "pack", "pack_version"];

fn count(conn: &Connection, table: &str) -> rusqlite::Result<i64> {
    conn.query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
        r.get(0)
    })
}

fn keys(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare(&format!("SELECT key FROM {table} ORDER BY key"))?
        .query_map([], |r| r.get(0))?
        .collect()
}

/// Apply the policy to a database already at the current layout.
pub fn apply(conn: &Connection) -> rusqlite::Result<Dropped> {
    let mut out = Dropped {
        log_segments: count(conn, "log_segment")?,
        log_entries: count(conn, "log_entry")?,
        ..Dropped::default()
    };
    conn.execute_batch(
        "UPDATE channel SET log_segment = NULL;
         DELETE FROM log_entry;
         DELETE FROM log_segment;",
    )?;
    for key in keys(conn, "settings")?
        .into_iter()
        .filter(|k| !world::settings::is_known(k))
    {
        conn.execute("DELETE FROM settings WHERE key = ?1", [&key])?;
        out.settings.push(key);
    }
    for key in keys(conn, "save_meta")?
        .into_iter()
        .filter(|k| !META_KEYS.contains(&k.as_str()))
    {
        conn.execute("DELETE FROM save_meta WHERE key = ?1", [&key])?;
        out.meta.push(key);
    }
    // Tables the current layout lacks: read the layout from a scratch database.
    let current = Connection::open_in_memory()?;
    current.execute_batch(world::SCHEMA)?;
    let wanted: BTreeSet<String> = tables(&current)?.into_iter().collect();
    for table in tables(conn)?.into_iter().filter(|t| !wanted.contains(t)) {
        out.tables.push((table.clone(), count(conn, &table)?));
        conn.execute_batch(&format!("DROP TABLE \"{table}\""))?;
    }
    Ok(out)
}

pub fn tables(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?
    .query_map([], |r| r.get(0))?
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_names_every_table_of_the_current_layout() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(world::SCHEMA).unwrap();
        for table in tables(&c).unwrap() {
            assert!(POLICY.contains(&table), "{table} is not in the policy text");
        }
    }
}
