use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// The format version this build understands.
///
/// Bumped only when a column changes meaning. Adding a column does not require it, which
/// is why the check is equality-or-lower rather than exact.
pub const SUPPORTED_FORMAT_VERSION: i64 = 1;

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
    pub unit_count: usize,
    pub build_fingerprint: String,
    pub source_url_pattern: String,
    pub source_revid_max: i64,
    /// Obligations the reader must honour. Unmet entries are fatal at open.
    pub requires: Vec<String>,
    pub units_by_template: BTreeMap<String, i64>,
    pub wording: BTreeMap<String, String>,
}

fn get<T: DeserializeOwned>(conn: &Connection, key: &'static str) -> Result<Option<T>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM manifest WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
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
        if format_version > SUPPORTED_FORMAT_VERSION {
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
            unit_count: get::<i64>(conn, "chunk_count")?.unwrap_or(0) as usize,
            build_fingerprint: get(conn, "build_fingerprint")?.unwrap_or_default(),
            source_url_pattern: get(conn, "source_url_pattern")?.unwrap_or_default(),
            source_revid_max: get(conn, "source_revid_max")?.unwrap_or(0),
            requires,
            units_by_template: get(conn, "chunks_by_template")?.unwrap_or_default(),
            wording: get(conn, "wording")?.unwrap_or_default(),
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

    /// The segmenter name without its version, for dispatch.
    pub fn segmenter_name(&self) -> &str {
        self.segmenter
            .split('/')
            .next()
            .unwrap_or(&self.segmenter)
    }
}
