//! Synthetic format-2 corpora for tests, written directly in SQL.
//!
//! The schema below mirrors the forge's writer for the columns this engine reads; the
//! forge-built toy corpus (`tests/contract.rs`) is what checks the two stay in step.
//! Lexical tokens are the text split on anything that is not alphanumeric, which is exactly
//! what the `char-bigram` segmenter produces for text without CJK, so fixtures should be
//! written in Latin script.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, params};

pub const SCHEMA: &str = "
CREATE TABLE chunks (id TEXT PRIMARY KEY, ord INTEGER NOT NULL, template TEXT NOT NULL,
    page TEXT NOT NULL, revid INTEGER, title TEXT, header TEXT, text TEXT NOT NULL,
    chars INTEGER NOT NULL, span_of TEXT, span_from INTEGER, span_to INTEGER);
CREATE UNIQUE INDEX idx_chunks_ord ON chunks(ord);
CREATE INDEX idx_chunks_span ON chunks(span_of, span_from);
CREATE VIRTUAL TABLE chunks_fts USING fts5(tokens, content='');
CREATE TABLE vectors (id INTEGER PRIMARY KEY CHECK (id = 0), dim INTEGER NOT NULL,
    count INTEGER NOT NULL, dtype TEXT NOT NULL, data BLOB NOT NULL);
CREATE TABLE persons (person_id TEXT PRIMARY KEY, primary_page TEXT NOT NULL,
    display TEXT NOT NULL, forms TEXT NOT NULL, facets TEXT NOT NULL,
    material INTEGER NOT NULL, persona_confidence REAL NOT NULL DEFAULT 0.0, birthday TEXT);
CREATE TABLE unit_persons (chunk_id TEXT NOT NULL, person_id TEXT NOT NULL,
    PRIMARY KEY (chunk_id, person_id)) WITHOUT ROWID;
CREATE INDEX idx_unit_persons_person ON unit_persons(person_id, chunk_id);
CREATE TABLE cooccur (a TEXT NOT NULL, b TEXT NOT NULL, scenes INTEGER NOT NULL,
    PRIMARY KEY (a, b), CHECK (a < b)) WITHOUT ROWID;
CREATE TABLE knowledge_scope (person_id TEXT NOT NULL, chunk_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('self', 'lived')),
    PRIMARY KEY (person_id, chunk_id)) WITHOUT ROWID;
CREATE TABLE topic_terms (person_id TEXT NOT NULL, term TEXT NOT NULL,
    PRIMARY KEY (person_id, term)) WITHOUT ROWID;
CREATE TABLE aliases (alias TEXT NOT NULL, target TEXT NOT NULL, kind TEXT NOT NULL,
    PRIMARY KEY (alias, target, kind)) WITHOUT ROWID;
CREATE TABLE prompts (subject TEXT NOT NULL, slot TEXT NOT NULL, body TEXT NOT NULL,
    generator_version TEXT NOT NULL DEFAULT '', PRIMARY KEY (subject, slot)) WITHOUT ROWID;
CREATE TABLE template_stats (template TEXT PRIMARY KEY, count INTEGER NOT NULL,
    chars_p50 INTEGER NOT NULL, chars_p95 INTEGER NOT NULL, chars_max INTEGER NOT NULL,
    stats TEXT NOT NULL DEFAULT '{}');
CREATE TABLE manifest (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// One unit to write. `span_of` defaults to the page and `span_from..=span_to` to the
/// unit's ordinal, so consecutive units of one page are neighbours.
#[derive(Debug, Clone, Default)]
pub struct UnitSpec<'a> {
    pub id: &'a str,
    pub template: &'a str,
    pub page: &'a str,
    pub header: &'a str,
    pub text: &'a str,
    pub persons: &'a [&'a str],
    pub span: Option<(&'a str, i64, i64)>,
}

/// A corpus file under the system temp directory, removed on drop.
pub struct TempFolio {
    path: PathBuf,
}

impl TempFolio {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFolio {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct Builder {
    conn: Connection,
    file: TempFolio,
    next_ord: i64,
    units: usize,
}

fn tokens(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

impl Builder {
    /// A fresh corpus with a format-2 manifest that declares `neighbor_expand` and the
    /// `char-bigram` segmenter.
    pub fn new() -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "folio-fixture-{}-{}.folio",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).expect("create fixture file");
        conn.execute_batch(SCHEMA).expect("fixture schema");
        let builder = Self {
            conn,
            file: TempFolio { path },
            next_ord: 0,
            units: 0,
        };
        builder
            .manifest("format_version", "2")
            .manifest("requires", r#"["neighbor_expand"]"#)
            .manifest("segmenter", r#""char-bigram/1""#)
            .manifest("pack", r#""fixture""#)
    }

    /// Set a manifest key to a raw JSON value.
    pub fn manifest(self, key: &str, json: &str) -> Self {
        self.conn
            .execute(
                "INSERT INTO manifest VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, json],
            )
            .expect("manifest row");
        self
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Write a unit, its lexical tokens and its attribution. Returns the ordinal.
    pub fn unit(&mut self, spec: UnitSpec<'_>) -> i64 {
        let ord = self.next_ord;
        self.next_ord += 1;
        let (span_of, from, to) = spec.span.unwrap_or((spec.page, ord, ord));
        let embed = if spec.header.is_empty() {
            spec.text.to_string()
        } else {
            format!("{}\n{}", spec.header, spec.text)
        };
        self.conn
            .execute(
                "INSERT INTO chunks VALUES (?1, ?2, ?3, ?4, 1, ?1, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    spec.id,
                    ord,
                    spec.template,
                    spec.page,
                    spec.header,
                    spec.text,
                    spec.text.chars().count() as i64,
                    span_of,
                    from,
                    to
                ],
            )
            .expect("chunk row");
        self.conn
            .execute(
                "INSERT INTO chunks_fts(rowid, tokens) VALUES (?1, ?2)",
                params![ord, tokens(&embed)],
            )
            .expect("fts row");
        for person in spec.persons {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO unit_persons VALUES (?1, ?2)",
                    [spec.id, person],
                )
                .expect("unit_persons row");
        }
        self.units += 1;
        ord
    }

    pub fn person(&mut self, id: &str, birthday: Option<&str>) -> &mut Self {
        let forms = format!(r#"[{{"page":"{id}","kind":"primary"}}]"#);
        self.conn
            .execute(
                "INSERT INTO persons VALUES (?1, ?1, ?1, ?2, '{}', 0, 0.5, ?3)",
                params![id, forms, birthday],
            )
            .expect("person row");
        self
    }

    pub fn scope(&mut self, person: &str, chunk: &str, kind: &str) -> &mut Self {
        self.conn
            .execute(
                "INSERT INTO knowledge_scope VALUES (?1, ?2, ?3)",
                [person, chunk, kind],
            )
            .expect("knowledge_scope row");
        self
    }

    pub fn topic(&mut self, person: &str, term: &str) -> &mut Self {
        self.conn
            .execute("INSERT INTO topic_terms VALUES (?1, ?2)", [person, term])
            .expect("topic_terms row");
        self
    }

    pub fn cooccur(&mut self, a: &str, b: &str, scenes: i64) -> &mut Self {
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        self.conn
            .execute(
                "INSERT INTO cooccur VALUES (?1, ?2, ?3)",
                params![a, b, scenes],
            )
            .expect("cooccur row");
        self
    }

    pub fn alias(&mut self, alias: &str, target: &str, kind: &str) -> &mut Self {
        self.conn
            .execute(
                "INSERT INTO aliases VALUES (?1, ?2, ?3)",
                [alias, target, kind],
            )
            .expect("alias row");
        self
    }

    /// Close the file, recording the unit count, and hand back its guard.
    pub fn finish(self) -> TempFolio {
        let units = self.units.to_string();
        let this = self.manifest("chunk_count", &units);
        this.conn.close().expect("close fixture");
        this.file
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}
