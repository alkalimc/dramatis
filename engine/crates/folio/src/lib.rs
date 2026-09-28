//! Read a `.folio` knowledge base.
//!
//! One SQLite file holds everything the runtime needs: retrieval units, their lexical
//! index, their vectors, the person roster, the alias dictionary, synthesised prompts, and
//! a manifest describing the build that produced all of it.
//!
//! Two behaviours here are deliberately strict, because a corpus is a file someone
//! downloaded and the failure mode worth engineering against is not a crash but a silent
//! half-working state:
//!
//! * an unknown `format_version` is refused rather than guessed at;
//! * a declared requirement this build does not implement is refused rather than ignored.
//!
//! The second matters more than it looks. Units are stored exactly once, so
//! `neighbor_expand` means "widen a ranked hit or serve truncated context". A reader that
//! quietly skipped that would return answers which look complete and are not.

pub mod error;
#[cfg(feature = "fixture")]
pub mod fixture;
pub mod manifest;
pub mod unit;
pub mod vectors;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

pub use error::{Error, Result};
pub use manifest::Manifest;
pub use unit::{Person, PersonForm, Scope, TemplateStats, Unit};
pub use vectors::Vectors;

/// The column list every unit query selects, in `row_to_unit` order, over `chunks c`.
///
/// Attribution is many-valued and lives in `unit_persons`; it comes back as one JSON
/// array per unit so each statement stays a single row-per-unit query.
macro_rules! unit_columns {
    () => {
        "c.id, c.ord, c.template, c.page, c.revid, c.title, c.header, c.text, c.chars, \
         c.span_of, c.span_from, c.span_to, \
         (SELECT json_group_array(person_id) FROM \
            (SELECT person_id FROM unit_persons WHERE chunk_id = c.id ORDER BY person_id))"
    };
}

macro_rules! person_columns {
    () => {
        "person_id, primary_page, display, forms, material, persona_confidence, birthday"
    };
}

/// An opened corpus.
pub struct Folio {
    conn: Connection,
    manifest: Manifest,
    path: PathBuf,
    vectors: Option<Vectors>,
}

impl Folio {
    /// Open read-only. The runtime never writes to a corpus: it is a distributable whose
    /// fingerprint must keep describing its contents.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(Error::Missing(path));
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Generous page cache: the lexical path touches the FTS index repeatedly and this
        // is the cheapest latency win available. Negative means KiB.
        conn.pragma_update(None, "cache_size", -32_000)?;
        let manifest = Manifest::load(&conn)?;
        Ok(Self {
            conn,
            manifest,
            path,
            vectors: None,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Load the vector store.
    ///
    /// Separate from `open` because the lexical path alone is useful — the library form of
    /// the product searches without an encoder — and because reading a vector store that
    /// can run to hundreds of megabytes should be an explicit act rather than a side effect of opening a file.
    pub fn load_vectors(&mut self) -> Result<&Vectors> {
        if self.vectors.is_none() {
            let (dim, count, raw): (i64, i64, Vec<u8>) = self.conn.query_row(
                "SELECT dim, count, data FROM vectors WHERE id = 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            self.vectors = Some(Vectors::new(raw, dim as usize, count as usize)?);
        }
        Ok(self.vectors.as_ref().expect("just loaded"))
    }

    pub fn vectors(&self) -> Option<&Vectors> {
        self.vectors.as_ref()
    }

    pub fn has_vectors(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
        Ok(count > 0)
    }

    pub fn unit_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Fetch units by ordinal, returned in the order requested.
    ///
    /// The statement shape is **fixed** and cached. An earlier version built SQL with one
    /// placeholder per ordinal, so every distinct candidate count produced a new statement
    /// to parse and plan; measured on a real corpus, preparation consumed most of the
    /// latency budget of a query whose execution was a small fraction of it. Ordinals
    /// arrive as a JSON array through one bound parameter and `json_each` turns it into
    /// rows, so the plan is the same for any count. SQL does not preserve the caller's
    /// ranking, so results are reordered on the way out.
    pub fn units_by_ord(&self, ords: &[i64]) -> Result<Vec<Unit>> {
        if ords.is_empty() {
            return Ok(Vec::new());
        }
        let ords_json = serde_json::to_string(ords).expect("i64 array always serialises");
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            unit_columns!(),
            " FROM chunks c JOIN json_each(?1) o ON c.ord = o.value"
        ))?;
        let rows = stmt.query_map([ords_json], Self::row_to_unit)?;
        let mut found: HashMap<i64, Unit> = HashMap::with_capacity(ords.len());
        for unit in rows {
            let unit = unit?;
            found.insert(unit.ord, unit);
        }
        Ok(ords.iter().filter_map(|ord| found.remove(ord)).collect())
    }

    /// Unit ordinals for a list of unit ids. Unknown ids are skipped: a stored id whose page
    /// has since changed simply no longer names a unit.
    pub fn ords_for_ids(&self, ids: &[String]) -> Result<Vec<i64>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_json = serde_json::to_string(ids).expect("string array serialises");
        let mut stmt = self.conn.prepare_cached(
            "SELECT ord FROM chunks WHERE id IN (SELECT value FROM json_each(?1))",
        )?;
        let rows = stmt.query_map([ids_json], |row| row.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn unit_by_id(&self, id: &str) -> Result<Option<Unit>> {
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            unit_columns!(),
            " FROM chunks c WHERE c.id = ?1"
        ))?;
        let mut rows = stmt.query_map([id], Self::row_to_unit)?;
        match rows.next() {
            Some(unit) => Ok(Some(unit?)),
            None => Ok(None),
        }
    }

    /// The units adjacent to this one in its source sequence.
    ///
    /// This is how `neighbor_expand` is satisfied. Storing each record once and widening
    /// on demand costs one indexed lookup per returned hit; storing half a window of
    /// lookbehind in every unit made most positions redundant and nearly doubled the vector
    /// store, which is why the corpus stopped doing that.
    pub fn neighbours(&self, unit: &Unit, before: i64, after: i64) -> Result<Vec<Unit>> {
        let (Some(span_of), Some(from), Some(to)) = (&unit.span_of, unit.span_from, unit.span_to)
        else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            unit_columns!(),
            " FROM chunks c \
                 WHERE c.span_of = ?1 AND c.span_to >= ?2 AND c.span_from <= ?3 AND c.id <> ?4 \
                 ORDER BY c.span_from"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![span_of, from - before, to + after, unit.id],
            Self::row_to_unit,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn template_stats(&self) -> Result<Vec<TemplateStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT template, count, chars_p50, chars_p95, chars_max \
             FROM template_stats ORDER BY count DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TemplateStats {
                template: row.get(0)?,
                count: row.get(1)?,
                embed_p50: row.get(2)?,
                embed_p95: row.get(3)?,
                embed_max: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Resolve a name through the alias dictionary.
    ///
    /// Redirects, disambiguation candidates, real names and alternate-form names are all
    /// human-curated synonymy the source's editors already wrote. Alternate-form entries are
    /// why this matters for identity: a query naming one incarnation must reach the one
    /// person, or the alias dictionary silently disagrees with the roster.
    pub fn resolve_alias(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT target FROM aliases WHERE alias = ?1 ORDER BY kind, target",
        )?;
        let rows = stmt.query_map([name], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// The people an alias names, as roster ids.
    ///
    /// Separate from `resolve_alias` because the two answer different questions. An alias
    /// may point at a page that is not a person — an event, a place, an item — and a caller
    /// restricting retrieval to a person needs the join, not the raw target. Most aliases
    /// typically resolve to somebody; the rest are pages about things, and returning those
    /// as "persons" would be a filter that matches nothing.
    pub fn persons_for_alias(&self, name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT p.person_id FROM aliases a JOIN persons p ON p.person_id = a.target \
             WHERE a.alias = ?1 ORDER BY p.person_id",
        )?;
        let rows = stmt.query_map([name], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn person(&self, person_id: &str) -> Result<Option<Person>> {
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            person_columns!(),
            " FROM persons WHERE person_id = ?1"
        ))?;
        let mut rows = stmt.query_map([person_id], Self::row_to_person)?;
        match rows.next() {
            Some(person) => Ok(Some(person?)),
            None => Ok(None),
        }
    }

    /// The whole roster, ordered by id.
    pub fn persons(&self) -> Result<Vec<Person>> {
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            person_columns!(),
            " FROM persons ORDER BY person_id"
        ))?;
        let rows = stmt.query_map([], Self::row_to_person)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// People whose birthday is `mm_dd` (`MM-DD`), ordered by id.
    pub fn persons_born_on(&self, mm_dd: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT person_id FROM persons WHERE birthday = ?1 ORDER BY person_id",
        )?;
        let rows = stmt.query_map([mm_dd], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Everyone who shares scenes with `person`, most shared scenes first.
    ///
    /// Pairs are stored once with `a < b`, so both columns are searched.
    pub fn cooccur(&self, person: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT b, scenes FROM cooccur WHERE a = ?1 \
             UNION ALL SELECT a, scenes FROM cooccur WHERE b = ?1 \
             ORDER BY 2 DESC, 1",
        )?;
        let rows = stmt.query_map([person], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Shared scenes for one pair, in either order; 0 when they never met.
    pub fn cooccur_pair(&self, a: &str, b: &str) -> Result<i64> {
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT scenes FROM cooccur WHERE a = ?1 AND b = ?2")?;
        let mut rows = stmt.query_map([a, b], |row| row.get::<_, i64>(0))?;
        Ok(rows.next().transpose()?.unwrap_or(0))
    }

    /// How each of `chunk_ids` sits in `person`'s first-hand knowledge. Units outside it
    /// are absent from the map.
    pub fn knowledge_scope(
        &self,
        person: &str,
        chunk_ids: &[String],
    ) -> Result<HashMap<String, Scope>> {
        if chunk_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids_json = serde_json::to_string(chunk_ids).expect("string array serialises");
        let mut stmt = self.conn.prepare_cached(
            "SELECT chunk_id, kind FROM knowledge_scope \
             WHERE person_id = ?1 AND chunk_id IN (SELECT value FROM json_each(?2))",
        )?;
        let rows = stmt.query_map(rusqlite::params![person, ids_json], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, kind) = row?;
            // The schema's CHECK admits exactly these two kinds.
            let scope = if kind == "self" {
                Scope::Own
            } else {
                Scope::Lived
            };
            out.insert(id, scope);
        }
        Ok(out)
    }

    /// Terms that put a lore unit inside `person`'s domain.
    pub fn topic_terms(&self, person: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT term FROM topic_terms WHERE person_id = ?1 ORDER BY term")?;
        let rows = stmt.query_map([person], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn person_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM persons", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// A prompt slot for a subject, with local override support left to the caller.
    pub fn prompt(&self, subject: &str, slot: &str) -> Result<Option<String>> {
        let body: Option<String> = self
            .conn
            .query_row(
                "SELECT body FROM prompts WHERE subject = ?1 AND slot = ?2",
                [subject, slot],
                |row| row.get(0),
            )
            .ok();
        Ok(body)
    }

    /// In-world phrasing for a runtime surface. The client renders these; it never
    /// composes them, or it would start inventing wording that leaks system vocabulary
    /// into the fiction.
    pub fn wording(&self, key: &str) -> Option<&str> {
        self.manifest.wording.get(key).map(String::as_str)
    }

    fn row_to_unit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Unit> {
        Ok(Unit {
            id: row.get(0)?,
            ord: row.get(1)?,
            template: row.get(2)?,
            page: row.get(3)?,
            revid: row.get(4)?,
            title: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            header: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            text: row.get(7)?,
            chars: row.get(8)?,
            span_of: row.get(9)?,
            span_from: row.get(10)?,
            span_to: row.get(11)?,
            persons: serde_json::from_str(&row.get::<_, String>(12)?).unwrap_or_default(),
        })
    }

    fn row_to_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<Person> {
        let forms_json: String = row.get(3)?;
        Ok(Person {
            person_id: row.get(0)?,
            primary_page: row.get(1)?,
            display: row.get(2)?,
            forms: serde_json::from_str(&forms_json).unwrap_or_default(),
            material: row.get(4)?,
            persona_confidence: row.get(5)?,
            birthday: row.get(6)?,
        })
    }
}
