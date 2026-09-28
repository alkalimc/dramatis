use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// The format version this build understands.
///
/// Bumped when a column changes meaning or moves. Version 2 replaced the single
/// `chunks.person` with the many-valued `unit_persons`, so version 1 reads through the
/// wrong columns and is refused like any other unknown version.
pub const SUPPORTED_FORMAT_VERSION: i64 = 2;

/// Capabilities this build implements, checked against `manifest.requires`.
///
/// `neighbor_expand`: units are stored exactly once, so surrounding context must be
/// fetched by widening a ranked hit rather than read out of the unit itself.
pub const IMPLEMENTED: &[&str] = &["neighbor_expand"];

/// What a corpus says about itself.
///
/// Read once at open. Every field here answers a question the reader would otherwise have
/// to guess: which segmenter to use at query time, how wide a vector is, whether this
/// build may serve this file at all.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub format_version: i64,
    pub pack: String,
    pub pack_version: i64,
    pub parser_version: i64,
    /// `name/version`, e.g. `jieba/0.42.1`. **Query-side segmentation must match**, or
    /// BM25 silently degrades: the index holds tokens produced by that exact segmenter.
    pub segmenter: String,
    /// Query-side stopwords, from the optional `stopwords` key (a JSON array of strings).
    /// Absent or empty means none. Supplied by the pack rather than built into the engine,
    /// so the list matches the corpus language and the build side that chose it.
    pub stopwords: Vec<String>,
    pub unit_count: usize,
    pub build_fingerprint: String,
    pub source_url_pattern: String,
    pub source_revid_max: i64,
    /// Obligations the reader must honour. Unmet entries are fatal at open.
    pub requires: Vec<String>,
    pub units_by_template: BTreeMap<String, i64>,
    /// Template name → the builder shape that produced it (`dialogue`, `lore`, ...). A pack
    /// may name templates freely; readers that treat a shape specially dispatch on this.
    pub template_shapes: BTreeMap<String, String>,
    pub wording: BTreeMap<String, String>,
    /// `[open, close]` around a reply line that describes action rather than speech.
    pub scene_marker: (String, String),
    /// Stands for the user's name in stored text (unit bodies, prompts). The corpus holds
    /// it verbatim; whoever assembles model-visible text substitutes it. Empty when the
    /// corpus declares none.
    pub user_placeholder: String,
    /// In-world year = local year minus this.
    pub year_offset: i64,
}

fn get<T: DeserializeOwned>(conn: &Connection, key: &'static str) -> Result<Option<T>> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM manifest WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .ok();
    match raw {
        None => Ok(None),
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|source| Error::Manifest { key, source }),
    }
}

fn require<T: DeserializeOwned>(conn: &Connection, key: &'static str) -> Result<T> {
    get(conn, key)?.ok_or(Error::MissingKey(key))
}

impl Manifest {
    pub fn load(conn: &Connection) -> Result<Self> {
        let format_version: i64 = require(conn, "format_version")?;
        if format_version != SUPPORTED_FORMAT_VERSION {
            return Err(Error::UnknownFormat {
                found: format_version,
                supported: SUPPORTED_FORMAT_VERSION,
            });
        }

        let requires: Vec<String> = get(conn, "requires")?.unwrap_or_default();
        if let Some(unmet) = requires.iter().find(|r| !IMPLEMENTED.contains(&r.as_str())) {
            return Err(Error::UnmetRequirement {
                requirement: unmet.clone(),
            });
        }

        Ok(Self {
            format_version,
            pack: get(conn, "pack")?.unwrap_or_default(),
            pack_version: get(conn, "pack_version")?.unwrap_or(0),
            parser_version: get(conn, "parser_version")?.unwrap_or(0),
            segmenter: get(conn, "segmenter")?.unwrap_or_default(),
            stopwords: get(conn, "stopwords")?.unwrap_or_default(),
            unit_count: get::<i64>(conn, "chunk_count")?.unwrap_or(0) as usize,
            build_fingerprint: get(conn, "build_fingerprint")?.unwrap_or_default(),
            source_url_pattern: get(conn, "source_url_pattern")?.unwrap_or_default(),
            source_revid_max: get(conn, "source_revid_max")?.unwrap_or(0),
            requires,
            units_by_template: get(conn, "chunks_by_template")?.unwrap_or_default(),
            template_shapes: get(conn, "template_shapes")?.unwrap_or_default(),
            wording: get(conn, "wording")?.unwrap_or_default(),
            scene_marker: get(conn, "scene_marker")?
                .unwrap_or_else(|| ("*".to_string(), "*".to_string())),
            user_placeholder: get(conn, "user_placeholder")?.unwrap_or_default(),
            year_offset: get(conn, "clock.year_offset")?.unwrap_or(0),
        })
    }

    /// Where a citation card should link. The corpus stores the pattern rather than the
    /// URLs so a site move does not invalidate every stored row.
    pub fn source_url(&self, page: &str) -> String {
        if self.source_url_pattern.is_empty() {
            return String::new();
        }
        self.source_url_pattern
            .replace("{page}", &page.replace(' ', "_"))
    }

    /// The builder shape of a template; a template the manifest does not map is its own
    /// shape.
    pub fn shape_of<'a>(&'a self, template: &'a str) -> &'a str {
        self.template_shapes
            .get(template)
            .map(String::as_str)
            .unwrap_or(template)
    }

    /// The segmenter name without its version, for dispatch.
    pub fn segmenter_name(&self) -> &str {
        self.segmenter.split('/').next().unwrap_or(&self.segmenter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(pairs: &[(&str, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE manifest (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for (key, value) in pairs {
            conn.execute("INSERT INTO manifest VALUES (?1, ?2)", [key, value])
                .unwrap();
        }
        conn
    }

    #[test]
    fn loads_the_supported_format() {
        let conn = manifest_with(&[
            ("format_version", "2"),
            ("requires", r#"["neighbor_expand"]"#),
        ]);
        let manifest = Manifest::load(&conn).unwrap();
        assert_eq!(manifest.requires, IMPLEMENTED);
        assert_eq!(manifest.scene_marker, ("*".into(), "*".into()));
        assert_eq!(
            (manifest.user_placeholder.as_str(), manifest.year_offset),
            ("", 0)
        );
    }

    #[test]
    fn reads_the_placeholder_and_the_clock_offset() {
        let conn = manifest_with(&[
            ("format_version", "2"),
            ("user_placeholder", r#""{user}""#),
            ("clock.year_offset", "100"),
        ]);
        let manifest = Manifest::load(&conn).unwrap();
        assert_eq!(manifest.user_placeholder, "{user}");
        assert_eq!(manifest.year_offset, 100);
    }

    #[test]
    fn refuses_older_and_newer_formats() {
        for version in ["1", "3"] {
            let conn = manifest_with(&[("format_version", version)]);
            assert!(matches!(
                Manifest::load(&conn),
                Err(Error::UnknownFormat { supported: 2, .. })
            ));
        }
    }

    #[test]
    fn refuses_an_unimplemented_requirement() {
        let conn = manifest_with(&[
            ("format_version", "2"),
            ("requires", r#"["neighbor_expand","span_merge"]"#),
        ]);
        assert!(matches!(
            Manifest::load(&conn),
            Err(Error::UnmetRequirement { requirement }) if requirement == "span_merge"
        ));
    }
}
