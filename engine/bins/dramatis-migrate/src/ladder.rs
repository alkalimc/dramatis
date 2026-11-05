//! Every released save format, and how each is brought to the one the app opens.
//!
//! Two families of saves exist, told apart by what they hold:
//!
//! - **Ladder family** (1.0.4 to 1.1.1): `user_version` 1 to 9, each step additive, kept
//!   here as `steps/NNNN.sql` exactly as the releases shipped them. A save is brought up
//!   by applying the steps above its version, each in its own transaction.
//! - **Request family** (1.0.0 to 1.0.3): its own `user_version` 1 to 9 with a `task`
//!   table (the delegated-request model that was later removed), which is the mark that
//!   tells it from the other. Its numbers mean something else, so no step applies to it;
//!   it is rebuilt table by table into the current layout (`bridge`).
//!
//! A save with no `save_meta` is any schema below 8. The steps are history: they are never
//! edited. A change to the current layout adds a step here (and edits `world`'s
//! `schema.sql`); a test checks that the steps and that file give the same tables.

use rusqlite::Connection;

/// One step of the ladder: `sql` takes a save from schema `to - 1` to `to`.
pub struct Step {
    pub to: i64,
    pub sql: &'static str,
    /// What the step does to a player's data, in a line.
    pub note: &'static str,
}

pub const STEPS: [Step; 9] = [
    Step {
        to: 1,
        sql: include_str!("../steps/0001.sql"),
        note: "first layout: bonds, channels, messages, memories, meter, settings, model log",
    },
    Step {
        to: 2,
        sql: include_str!("../steps/0002.sql"),
        note: "bond.trust_exact (empty: exactly the trust), channel.read_up_to / direct_with / deleted_at, fact.trust_delta; new tables seed and session",
    },
    Step {
        to: 3,
        sql: include_str!("../steps/0003.sql"),
        note: "meter.requests (0: the row was token-billed)",
    },
    Step {
        to: 4,
        sql: include_str!("../steps/0004.sql"),
        note: "channel.name / avatar / archived (empty, empty, not archived)",
    },
    Step {
        to: 5,
        sql: include_str!("../steps/0005.sql"),
        note: "new table user_avatar",
    },
    Step {
        to: 6,
        sql: include_str!("../steps/0006.sql"),
        note: "new table picture",
    },
    Step {
        to: 7,
        sql: include_str!("../steps/0007.sql"),
        note: "fact.expires (empty: never), new table daily_event, seed rebuilt for the kinds event and visit (every row copied)",
    },
    Step {
        to: 8,
        sql: include_str!("../steps/0008.sql"),
        note: "new tables save_meta and channel_summary, log_segment.digest",
    },
    Step {
        to: 9,
        sql: include_str!("../steps/0009.sql"),
        note: "daily_event.hidden (0: shown)",
    },
];

/// The request family's own steps, kept only so the tests can build a save as those
/// releases wrote it. The tool never applies them.
#[cfg(test)]
pub const REQUEST_FAMILY: [&str; 9] = [
    include_str!("../steps/lineage_a/0001.sql"),
    include_str!("../steps/lineage_a/0002.sql"),
    include_str!("../steps/lineage_a/0003.sql"),
    include_str!("../steps/lineage_a/0004.sql"),
    include_str!("../steps/lineage_a/0005.sql"),
    include_str!("../steps/lineage_a/0006.sql"),
    include_str!("../steps/lineage_a/0007.sql"),
    include_str!("../steps/lineage_a/0008.sql"),
    include_str!("../steps/lineage_a/0009.sql"),
];

/// Which family a save belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Additive ladder, 1.0.4 and later.
    Ladder,
    /// The delegated-request model, 1.0.0 to 1.0.3.
    Request,
}

/// What a save is, as far as its file can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    pub schema: i64,
    pub family: Family,
}

impl Source {
    /// The releases that wrote this format, for a person to read.
    pub fn releases(&self) -> &'static str {
        match (self.family, self.schema) {
            (Family::Request, 9) => "1.0.0 to 1.0.3",
            (Family::Request, _) => "a development build before 1.0.0",
            (Family::Ladder, 7) => "1.0.4 to 1.1.0",
            (Family::Ladder, 8 | 9) => "1.1.1",
            (Family::Ladder, _) => "a development build before 1.0.4",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let family = match self.family {
            Family::Ladder => "",
            Family::Request => ", request model",
        };
        write!(f, "schema {}{family} ({})", self.schema, self.releases())
    }
}

/// Which family `conn` holds. Read from the tables, since the number alone is the same.
pub fn family(conn: &Connection) -> rusqlite::Result<Family> {
    let task: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'task'",
        [],
        |r| r.get(0),
    )?;
    Ok(if task > 0 {
        Family::Request
    } else {
        Family::Ladder
    })
}

/// The steps a save at `from` still needs, as lines of the report.
pub fn pending(from: &Source) -> Vec<String> {
    match from.family {
        Family::Ladder => STEPS
            .iter()
            .filter(|s| s.to > from.schema)
            .map(|s| format!("schema {} -> {}: {}", s.to - 1, s.to, s.note))
            .collect(),
        Family::Request => vec![crate::bridge::NOTE.to_owned()],
    }
}

/// Apply the steps above `from` to `conn`, each in its own transaction with its version.
pub fn climb(conn: &mut Connection, from: i64) -> rusqlite::Result<()> {
    for step in STEPS.iter().filter(|s| s.to > from) {
        let tx = conn.transaction()?;
        tx.execute_batch(step.sql)?;
        tx.pragma_update(None, "user_version", step.to)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_are_numbered_in_order_and_end_at_the_apps_schema() {
        for (i, s) in STEPS.iter().enumerate() {
            assert_eq!(s.to, i as i64 + 1);
        }
        assert_eq!(STEPS.last().unwrap().to, world::SCHEMA_VERSION);
    }

    /// Normalised description of one table: columns in order, keys, checks and indexes. The
    /// steps add columns after the checks, so the text of the DDL differs; what it defines
    /// must not.
    fn describe(conn: &Connection) -> Vec<String> {
        let mut out = Vec::new();
        let tables: Vec<(String, String)> = conn
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (name, sql) in tables {
            let q = |text: &str| -> Vec<String> {
                let mut stmt = conn.prepare(text).unwrap();
                let n = stmt.column_count();
                stmt.query_map([], |r| {
                    Ok((0..n)
                        .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join(","))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect()
            };
            out.push(format!(
                "{name} cols {:?}",
                q(&format!(
                    "SELECT cid, name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo('{name}')"
                ))
            ));
            out.push(format!(
                "{name} kind {:?}",
                q(&format!(
                    "SELECT wr, strict FROM pragma_table_list WHERE name = '{name}'"
                ))
            ));
            out.push(format!(
                "{name} fk {:?}",
                q(&format!(
                    "SELECT \"from\", \"table\", \"to\", on_update, on_delete FROM pragma_foreign_key_list('{name}') ORDER BY \"from\""
                ))
            ));
            let mut indexes = q(&format!(
                "SELECT name, \"unique\", origin, partial FROM pragma_index_list('{name}')"
            ));
            indexes.sort();
            out.push(format!("{name} indexes {indexes:?}"));
            let mut checks = checks_of(&sql);
            checks.sort();
            out.push(format!("{name} checks {checks:?}"));
        }
        let mut indexes: Vec<String> = conn
            .prepare("SELECT name, tbl_name, sql FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL")
            .unwrap()
            .query_map([], |r| {
                Ok(format!(
                    "{} on {}: {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    squash(&r.get::<_, String>(2)?)
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        indexes.sort();
        out.extend(indexes);
        out
    }

    fn squash(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The `CHECK (...)` expressions of a CREATE TABLE, whitespace squashed.
    fn checks_of(sql: &str) -> Vec<String> {
        // Comments would hold parentheses.
        let code: String = sql
            .lines()
            .map(|l| l.split("--").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let upper = code.to_uppercase();
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = upper[from..].find("CHECK") {
            let start = from + at + "CHECK".len();
            let open = start + code[start..].find('(').unwrap();
            let mut depth = 0;
            let mut end = open;
            for (i, c) in code[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push(squash(&code[open + 1..end]));
            from = end;
        }
        out
    }

    /// The steps and `world`'s schema file define one layout. If this fails after a layout
    /// change, the change is missing from one of the two.
    #[test]
    fn the_steps_and_the_schema_file_make_the_same_tables() {
        let mut stepped = Connection::open_in_memory().unwrap();
        climb(&mut stepped, 0).unwrap();
        let built = Connection::open_in_memory().unwrap();
        built.execute_batch(world::SCHEMA).unwrap();
        let (a, b) = (describe(&stepped), describe(&built));
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x, y);
        }
        assert_eq!(a.len(), b.len());
        // And the description sees differences: a table without a column does not match.
        built
            .execute_batch("ALTER TABLE meter ADD COLUMN extra INTEGER")
            .unwrap();
        assert_ne!(describe(&built), a);
    }

    #[test]
    fn the_request_family_is_told_by_its_task_table() {
        let c = Connection::open_in_memory().unwrap();
        for sql in REQUEST_FAMILY {
            c.execute_batch(sql).unwrap();
        }
        assert_eq!(family(&c).unwrap(), Family::Request);
        let mut c = Connection::open_in_memory().unwrap();
        climb(&mut c, 0).unwrap();
        assert_eq!(family(&c).unwrap(), Family::Ladder);
    }
}
