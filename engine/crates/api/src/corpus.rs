//! The opened `.folio`, with what the app reads on every screen cached at load.
//!
//! Two connections: a plain [`Folio`] for lookups, and the retrieval [`Index`], built on
//! first use (or by [`Corpus::warm`] in the background) because its segmenter takes
//! longer to set up than the whole first screen may.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use folio::{Folio, Manifest};
use index::Index;
use md5::{Digest, Md5};

use crate::error::{ApiError, ApiResult};
use crate::views::CorpusState;

/// The host's subject id in the prompts table.
pub const HOST: &str = "host";
/// The prompt slots a persona is made of: a person has one only with all of them.
pub const PERSONA_SLOTS: [&str; 3] = ["system", "tone", "capability"];
/// Built at corpus time: the first-run card, `[{person_id, reason}]`.
const COLDSTART_SLOT: &str = "coldstart";
/// Co-appearing people kept per person; screens show fewer, after dropping the disabled.
const COMPANIONS_KEPT: usize = 8;

/// One roster entry as screens need it.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub id: String,
    pub display: String,
    pub primary_page: String,
    pub birthday: Option<world::MonthDay>,
    pub persona_confidence: f64,
}

pub struct Corpus {
    pub manifest: Manifest,
    path: PathBuf,
    folio: Mutex<Folio>,
    retrieve: index::params::Retrieve,
    index: OnceLock<Result<Mutex<Index>, String>>,
    persons: Vec<Person>,
    by_id: HashMap<String, usize>,
    companions: HashMap<String, Vec<String>>,
    personas: HashSet<String>,
    overrides: HashMap<String, BTreeMap<String, String>>,
    coldstart: Vec<(String, String)>,
    revision_url_pattern: String,
    avatar_url_pattern: String,
    player_avatar_pattern: String,
}

impl std::fmt::Debug for Corpus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Corpus").field("path", &self.path).finish()
    }
}

/// How a refused or unreadable corpus shows on the status screen.
pub fn refusal(e: &folio::Error) -> CorpusState {
    match e {
        folio::Error::UnknownFormat { found, .. } => CorpusState::UnsupportedVersion {
            format_version: u32::try_from(*found).unwrap_or(0),
        },
        folio::Error::UnmetRequirement { requirement } => CorpusState::MissingRequirement {
            requirement: requirement.clone(),
        },
        // Absent or unreadable: nothing to show but the open prompt.
        _ => CorpusState::Missing,
    }
}

impl Corpus {
    /// Open and apply the load rules (`Folio::open` refuses an unknown format version and
    /// an unmet requirement). `prompts_dir` holds optional local prompt overrides.
    pub fn open(
        path: &Path,
        retrieve: index::params::Retrieve,
        prompts_dir: &Path,
    ) -> Result<Self, folio::Error> {
        let folio = Folio::open(path)?;
        let manifest = folio.manifest().clone();

        let mut persons = Vec::new();
        for p in folio.persons()? {
            persons.push(Person {
                birthday: p.birthday.as_deref().and_then(world::MonthDay::parse),
                id: p.person_id,
                display: p.display,
                primary_page: p.primary_page,
                persona_confidence: p.persona_confidence,
            });
        }
        let by_id = persons
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();

        let companions = companions(folio.conn())?;
        let overrides = read_overrides(prompts_dir);
        let mut slots: HashMap<String, HashSet<String>> = HashMap::new();
        {
            let mut stmt = folio
                .conn()
                .prepare("SELECT subject, slot FROM prompts WHERE trim(body) <> ''")?;
            for row in
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            {
                let (subject, slot) = row?;
                slots.entry(subject).or_default().insert(slot);
            }
        }
        for (subject, own) in &overrides {
            let entry = slots.entry(subject.clone()).or_default();
            entry.extend(
                own.iter()
                    .filter(|(_, b)| !b.trim().is_empty())
                    .map(|(s, _)| s.clone()),
            );
        }
        let personas: HashSet<String> = slots
            .into_iter()
            .filter(|(subject, have)| {
                subject != HOST && PERSONA_SLOTS.iter().all(|s| have.contains(*s))
            })
            .map(|(subject, _)| subject)
            .collect();

        let coldstart = folio
            .prompt(HOST, COLDSTART_SLOT)?
            .and_then(|body| serde_json::from_str::<Vec<Pick>>(&body).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.person_id, p.reason))
            .collect();
        let revision_url_pattern = optional_key(folio.conn(), "revision_url_pattern");
        let avatar_url_pattern = optional_key(folio.conn(), "avatar_url_pattern");
        let player_avatar_pattern = optional_key(folio.conn(), "player_avatar_url");

        Ok(Self {
            manifest,
            path: path.to_path_buf(),
            folio: Mutex::new(folio),
            retrieve,
            index: OnceLock::new(),
            persons,
            by_id,
            companions,
            personas,
            overrides,
            coldstart,
            revision_url_pattern,
            avatar_url_pattern,
            player_avatar_pattern,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The name the status line shows: the pack, else the file name.
    pub fn name(&self) -> String {
        if self.manifest.pack.is_empty() {
            self.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            self.manifest.pack.clone()
        }
    }

    /// Every roster entry, ordered by id.
    pub fn persons(&self) -> &[Person] {
        &self.persons
    }

    /// Everyone with a persona, by id.
    pub fn persona_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.personas.iter().cloned().collect();
        ids.sort();
        ids
    }

    pub fn person(&self, id: &str) -> Option<&Person> {
        self.by_id.get(id).map(|&i| &self.persons[i])
    }

    /// Display name, falling back to the id for someone the roster does not know.
    pub fn display(&self, id: &str) -> String {
        self.person(id)
            .map(|p| p.display.clone())
            .unwrap_or_else(|| id.to_owned())
    }

    /// Whether the person has a synthesised persona: without one nobody can speak as them.
    pub fn has_persona(&self, id: &str) -> bool {
        self.personas.contains(id)
    }

    /// Co-appearing people, most shared scenes first.
    pub fn companions(&self, id: &str) -> &[String] {
        self.companions.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The built first-run picks, `(person_id, reason)`, in their built order.
    pub fn coldstart(&self) -> &[(String, String)] {
        &self.coldstart
    }

    /// A prompt slot: the local override (`prompts/<subject>.yaml`) wins over the folio.
    pub fn prompt(&self, subject: &str, slot: &str) -> ApiResult<Option<String>> {
        if let Some(body) = self.overrides.get(subject).and_then(|s| s.get(slot)) {
            return Ok(Some(body.clone()));
        }
        self.with_folio(|f| f.prompt(subject, slot))
    }

    /// Link to one stored revision of a page; empty when the corpus has no pattern.
    pub fn revision_url(&self, page: &str, revid: u32) -> String {
        if self.revision_url_pattern.is_empty() {
            return String::new();
        }
        self.revision_url_pattern
            .replace("{revid}", &revid.to_string())
            .replace("{page}", &page.replace(' ', "_"))
    }

    /// Where a person's avatar lives on the source site, from the optional manifest key
    /// `avatar_url_pattern`; `None` without one. `{page}` stands for the page title (spaces
    /// as underscores). MediaWiki upload hosts store a file under a path built from the md5
    /// of its name, so `{md5_1}` and `{md5_2}` stand for the first one and two hex digits of
    /// that hash, taken over the last path segment once `{page}` is filled in. The segment is
    /// percent-encoded after hashing.
    pub fn avatar_url(&self, page: &str) -> Option<String> {
        (!self.avatar_url_pattern.is_empty()).then(|| expand_avatar(&self.avatar_url_pattern, page))
    }

    /// Where the player's default picture lives, from the optional manifest key
    /// `player_avatar_url`: a full URL in the same form as `avatar_url_pattern` (the hash
    /// placeholders work; there is no page title). `None` without one.
    pub fn player_avatar_url(&self) -> Option<String> {
        (!self.player_avatar_pattern.is_empty())
            .then(|| expand_avatar(&self.player_avatar_pattern, ""))
    }

    /// The first line of a person's profile text, if the corpus has a profile unit for them.
    pub fn summary(&self, id: &str) -> ApiResult<Option<String>> {
        let templates: Vec<&str> = self
            .manifest
            .template_shapes
            .iter()
            .filter(|(_, shape)| shape.as_str() == "profile")
            .map(|(t, _)| t.as_str())
            .collect();
        if templates.is_empty() {
            return Ok(None);
        }
        let marks = vec!["?"; templates.len()].join(",");
        let sql = format!(
            "SELECT c.text FROM chunks c JOIN unit_persons u ON u.chunk_id = c.id \
             WHERE u.person_id = ?1 AND c.template IN ({marks}) ORDER BY c.ord LIMIT 1"
        );
        self.with_folio(|f| {
            let mut args: Vec<&dyn rusqlite::ToSql> = vec![&id];
            args.extend(templates.iter().map(|t| t as &dyn rusqlite::ToSql));
            let text: Option<String> = f.conn().query_row(&sql, args.as_slice(), |r| r.get(0)).ok();
            Ok(text.and_then(|t| {
                t.lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .map(str::to_owned)
            }))
        })
    }

    pub fn with_folio<R>(&self, f: impl FnOnce(&Folio) -> folio::Result<R>) -> ApiResult<R> {
        let folio = self.folio.lock().unwrap_or_else(|e| e.into_inner());
        f(&folio).map_err(io)
    }

    /// Run against the retrieval index, building it first if nobody has yet.
    pub fn with_index<R>(&self, f: impl FnOnce(&Index) -> index::Result<R>) -> ApiResult<R> {
        let slot = self.index.get_or_init(|| {
            Index::open(&self.path, self.retrieve.clone())
                .map(Mutex::new)
                .map_err(|e| e.to_string())
        });
        let index = slot.as_ref().map_err(|detail| ApiError::Io {
            detail: detail.clone(),
        })?;
        let index = index.lock().unwrap_or_else(|e| e.into_inner());
        f(&index).map_err(|e| ApiError::Io {
            detail: e.to_string(),
        })
    }

    /// Build the index now, so the first search does not wait for it.
    pub fn warm(&self) {
        let _ = self.with_index(|_| Ok(()));
    }
}

/// A manifest key holding a JSON string; empty when absent.
fn optional_key(conn: &rusqlite::Connection, key: &str) -> String {
    conn.query_row("SELECT value FROM manifest WHERE key = ?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default()
}

fn io(e: folio::Error) -> ApiError {
    ApiError::Io {
        detail: e.to_string(),
    }
}

#[derive(serde::Deserialize)]
struct Pick {
    person_id: String,
    reason: String,
}

/// Everyone's co-appearing people in one pass over the pair table.
fn companions(conn: &rusqlite::Connection) -> rusqlite::Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare("SELECT a, b, scenes FROM cooccur")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut all: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for row in rows {
        let (a, b, n) = row?;
        all.entry(a.clone()).or_default().push((n, b.clone()));
        all.entry(b).or_default().push((n, a));
    }
    Ok(all
        .into_iter()
        .map(|(p, mut v)| {
            v.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| x.1.cmp(&y.1)));
            v.truncate(COMPANIONS_KEPT);
            (p, v.into_iter().map(|(_, q)| q).collect())
        })
        .collect())
}

/// `prompts/<subject>.yaml` files, each a map of slot to body. An unreadable file is
/// skipped: an override is a convenience, never a reason not to start.
fn read_overrides(dir: &Path) -> HashMap<String, BTreeMap<String, String>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        let yaml = path.extension().is_some_and(|x| x == "yaml" || x == "yml");
        let Some(subject) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !yaml {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_norway::from_str::<BTreeMap<String, String>>(&t).ok());
        match parsed {
            Some(slots) => {
                out.insert(subject.to_owned(), slots);
            }
            None => eprintln!(
                "dramatis: ignoring unreadable prompt override {}",
                path.display()
            ),
        }
    }
    out
}

fn expand_avatar(pattern: &str, page: &str) -> String {
    let page = page.replace(' ', "_");
    let (head, name) = match pattern.rfind('/') {
        Some(i) => pattern.split_at(i + 1),
        None => ("", pattern),
    };
    let raw = name.replace("{page}", &page);
    let hex: String = Md5::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let head = head
        .replace("{page}", &page)
        .replace("{md5_1}", &hex[..1])
        .replace("{md5_2}", &hex[..2]);
    // The page title is encoded whole; the pack's own text only where it is not ASCII,
    // so a query or fragment it wrote stays intact.
    let name = name
        .split("{page}")
        .map(|lit| percent_encode(lit, |b| b.is_ascii() && b != b' '))
        .collect::<Vec<_>>()
        .join(&percent_encode(&page, |b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
        }));
    format!("{head}{name}")
}

/// Percent-encode every byte `keep` rejects.
fn percent_encode(s: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::expand_avatar;

    #[test]
    fn plain_pattern_substitutes_the_page() {
        assert_eq!(
            expand_avatar("https://img.example/{page}.png", "Amy Lee"),
            "https://img.example/Amy_Lee.png"
        );
    }

    #[test]
    fn hashed_upload_path_uses_the_md5_of_the_file_name() {
        // Known vector: md5("abc") = 900150983cd24fb0d6963f7d28e17f72.
        assert_eq!(
            expand_avatar("https://m.example/{md5_1}/{md5_2}/{page}", "abc"),
            "https://m.example/9/90/abc"
        );
    }

    #[test]
    fn the_hash_covers_the_literal_prefix_and_the_name_is_encoded() {
        // The hash is taken over the raw name; encoding comes after.
        let url = expand_avatar(
            "https://m.example/{md5_1}/{md5_2}/\u{e9}_{page}.png",
            "a b&c",
        );
        assert!(url.ends_with("/%C3%A9_a_b%26c.png"), "{url}");
    }

    #[test]
    fn a_query_in_the_pattern_is_left_alone() {
        assert_eq!(
            expand_avatar("https://s.example/f?name={page}&w=1", "x y"),
            "https://s.example/f?name=x_y&w=1"
        );
    }
}
