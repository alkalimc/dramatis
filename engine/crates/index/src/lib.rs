//! Retrieval over a `.folio`.
//!
//! ```text
//! query ─ alias expansion ─ lexical: FTS5 + BM25 over pre-segmented tokens, filters in SQL
//!       → retrieve.candidates ─ weight by the asking person's knowledge ─ merge memories
//!       → drop units already in the session ─ retrieve.top_k ─ neighbours, returned hits only
//! ```
//!
//! Only the lexical path exists. `SearchMode::Hybrid` and `Dense` are part of the request
//! so a dense path can be added without changing callers; until then they fail loudly
//! rather than quietly answering from the lexical path.
//!
//! **Alias expansion is not optional.** Measured: a query naming an entity by an alias in
//! another language or script matches *zero* units when the alias occurs nowhere in the
//! corpus text — the source records it as a redirect, not as prose. The table lookup is the
//! only path to those entities.
//!
//! **Weighting is not filtering.** A person's own units, the scenes they lived through and
//! lore inside their topics rank higher for them, but nothing is removed: "the corpus has
//! nothing" and "this is not his field" must stay distinguishable, which is also why
//! confidence is computed on unweighted scores.
//!
//! **There is no span-merge stage.** Units are stored exactly once; ranked hits are widened
//! to their neighbours on demand, which is cheaper and gives truer context than overlapping
//! windows.

pub mod confidence;
mod memory;
pub mod params;
pub mod segment;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use folio::{Folio, Unit};

pub use confidence::{Confidence, Level};
pub use segment::{Kind as SegmenterKind, Segmenter};

/// Roster id (`persons.person_id`).
pub type PersonId = String;
/// Unit id (`chunks.id`).
pub type ChunkId = String;

/// Which retrieval paths to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Every path the corpus and this build support. Today that is the lexical path.
    #[default]
    Auto,
    /// Lexical and dense, fused. Needs vectors and a query encoder.
    Hybrid,
    Lexical,
    /// Dense only. Needs vectors and a query encoder.
    Dense,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RerankPolicy {
    /// Rerank when a reranker is available. None is, so this never reranks.
    #[default]
    Auto,
    /// Fail if no reranker is available.
    Always,
    Never,
}

/// Restrictions applied in SQL before candidates are cut, so a filter can never empty a
/// result the corpus could fill. Empty lists mean no restriction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub templates: Vec<String>,
    /// Units attributed to any of these people (forms already folded into the person).
    pub persons: Vec<PersonId>,
    pub pages: Vec<String>,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty() && self.persons.is_empty() && self.pages.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    /// Hits to return; 0 means `retrieve.top_k`.
    pub top_k: usize,
    pub filters: Filters,
    /// Whose knowledge weights the ranking, and who may receive memories. `None` is the
    /// maintainer surface: every unit unweighted, no memories.
    pub as_person: Option<PersonId>,
    /// Units (and memory ids) already in the session. They are never returned as hits;
    /// the ones that would have been are listed in [`SearchResponse::excluded`] so the
    /// caller can refer to them by id.
    pub exclude: Vec<ChunkId>,
    pub rerank: RerankPolicy,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }
}

/// Where a hit sits relative to the asking person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A unit attributed to them: they said it, or it is about them.
    Own,
    /// Another unit of a scene they speak in.
    Lived,
    /// A lore unit inside their topics.
    InScope,
    /// Everything else in the corpus. Still retrievable; they would have to look it up.
    OutOfScope,
    /// A caller-supplied memory.
    Memory,
    /// No `as_person`: the maintainer surface, where nothing is weighted.
    Unscoped,
}

#[derive(Debug, Clone)]
pub enum Item {
    Unit(Unit),
    Memory { id: String, text: String },
}

impl Item {
    /// The unit id or memory id.
    pub fn id(&self) -> &str {
        match self {
            Item::Unit(unit) => &unit.id,
            Item::Memory { id, .. } => id,
        }
    }

    pub fn unit(&self) -> Option<&Unit> {
        match self {
            Item::Unit(unit) => Some(unit),
            Item::Memory { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub item: Item,
    pub source: Source,
    /// Ranking score: `unweighted` times the source's `retrieve.scope_weight`.
    pub score: f64,
    /// BM25, as the corpus statistics give it.
    pub unweighted: f64,
    /// Adjacent units of the same source sequence (`retrieve.neighbours` on each side),
    /// minus any that are hits themselves or excluded.
    pub neighbours: Vec<Unit>,
}

#[derive(Debug, Clone)]
pub struct SearchResponse {
    pub hits: Vec<Hit>,
    /// On unweighted scores, excluded units included: having shown the answer already
    /// does not make the corpus less sure of it.
    pub confidence: Confidence,
    /// Excluded ids that ranked inside the returned window, best first.
    pub excluded: Vec<String>,
    /// Set when the query named an entity by an alias.
    pub resolved: Option<Resolved>,
    /// Set when the query is a name the source records as ambiguous. Hits are the
    /// unrestricted best effort.
    pub ambiguous: Option<Ambiguity>,
    pub trace: Trace,
}

/// What to do with a query that names something the source records under another name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Normalise {
    /// Take the query as typed.
    Off,
    /// Add the canonical name's tokens to the query.
    Expand,
    /// Add the tokens, and when the alias names exactly one person and the request has no
    /// person filter, restrict to them.
    #[default]
    ExpandAndFilter,
}

/// What the alias stage found, so a caller can say what it understood the query to mean.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub alias: String,
    pub target: String,
    /// The roster id, when the target is a person.
    pub person: Option<PersonId>,
    pub how: How,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// The alias names exactly one thing.
    Unique,
    /// The alias names several, and the rest of the query named one of them.
    Qualified,
}

/// The alias names several things and nothing in the query said which. A reportable
/// state, not a failure: picking one would answer a question the user did not ask.
#[derive(Debug, Clone)]
pub struct Ambiguity {
    pub alias: String,
    pub candidates: Vec<String>,
}

/// One person found for a topic, by their own units.
#[derive(Debug, Clone)]
pub struct PersonMatch {
    pub person: PersonId,
    /// Best unweighted score among their own units.
    pub score: f64,
    /// `score` read against `retrieve.confidence` on its own. `High` is the interjection
    /// bar (`interject.min_confidence` is the high boundary).
    pub level: Level,
    /// Own units that matched.
    pub matched: usize,
    /// The best-matching own unit: the reason to offer them.
    pub reason: Unit,
}

#[derive(Debug, Clone)]
pub struct People {
    /// Best first; ties broken by id so the order is reproducible.
    pub persons: Vec<PersonMatch>,
    /// Over the returned people's best scores.
    pub confidence: Confidence,
}

impl People {
    pub fn get(&self, person: &str) -> Option<&PersonMatch> {
        self.persons.iter().find(|p| p.person == person)
    }
}

/// Per-stage timings, measured by default: latency is a design constraint here.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub normalise_us: u128,
    pub lexical_us: u128,
    pub scope_us: u128,
    pub memory_us: u128,
    pub fetch_us: u128,
    pub expand_us: u128,
    pub lexical_candidates: usize,
}

impl Trace {
    pub fn total_us(&self) -> u128 {
        self.normalise_us
            + self.lexical_us
            + self.scope_us
            + self.memory_us
            + self.fetch_us
            + self.expand_us
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Folio(#[from] folio::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0:?} search needs a dense path, which this build does not have; use Auto or Lexical")]
    DenseUnavailable(SearchMode),
    #[error("reranking was required, but no reranker is available")]
    RerankUnavailable,
    #[error("memories can only be ranked for a person; the maintainer surface sees none")]
    MemoryWithoutPerson,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The retriever: an opened corpus, its segmenter and the `retrieve.*` parameters.
///
/// Owns the corpus connection, so it is `Send` but not `Sync`; share it behind a mutex.
pub struct Index {
    folio: Folio,
    segmenter: Segmenter,
    params: params::Retrieve,
    normalise: Normalise,
    stats: std::cell::OnceCell<memory::CorpusStats>,
}

/// A lexical candidate before weighting.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    ord: i64,
    score: f64,
}

impl Index {
    /// The segmenter and its stopwords come from the corpus manifest, not from the caller:
    /// they must match what built the lexical index, and a mismatch has no symptom other
    /// than quietly worse results.
    pub fn new(folio: Folio, params: params::Retrieve) -> Self {
        let manifest = folio.manifest();
        let segmenter = Segmenter::for_corpus(manifest.segmenter_name(), &manifest.stopwords);
        Self {
            folio,
            segmenter,
            params,
            normalise: Normalise::default(),
            stats: std::cell::OnceCell::new(),
        }
    }

    pub fn open(path: impl AsRef<std::path::Path>, params: params::Retrieve) -> Result<Self> {
        Ok(Self::new(Folio::open(path)?, params))
    }

    pub fn folio(&self) -> &Folio {
        &self.folio
    }

    pub fn segmenter(&self) -> &Segmenter {
        &self.segmenter
    }

    pub fn params(&self) -> &params::Retrieve {
        &self.params
    }

    pub fn set_params(&mut self, params: params::Retrieve) {
        self.params = params;
    }

    /// What the alias stage may do. The default expands and, for a bare person name,
    /// filters to that person; the other settings exist to be measured against it.
    pub fn set_normalise(&mut self, normalise: Normalise) {
        self.normalise = normalise;
    }

    /// Search the corpus. No memories: see [`Index::search_with_memories`].
    pub fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        self.search_with_memories(request, &[])
    }

    /// Search the corpus and rank caller-supplied memories `(id, text)` alongside it.
    ///
    /// The caller decides which memories this person may see: visibility is applied when
    /// the candidate list is built, never here, so a filtered-out memory cannot shrink a
    /// result after the fact. Memories are scored as BM25 against the corpus's own
    /// statistics, so their scores sit on the same scale as units, then weighted by
    /// `retrieve.scope_weight.memory`. Filters describe corpus units, so a request with
    /// any filter set receives no memories.
    pub fn search_with_memories(
        &self,
        request: &SearchRequest,
        memories: &[(&str, &str)],
    ) -> Result<SearchResponse> {
        match request.mode {
            SearchMode::Auto | SearchMode::Lexical => {}
            mode @ (SearchMode::Hybrid | SearchMode::Dense) => {
                return Err(Error::DenseUnavailable(mode));
            }
        }
        if request.rerank == RerankPolicy::Always {
            return Err(Error::RerankUnavailable);
        }
        if request.as_person.is_none() && !memories.is_empty() {
            return Err(Error::MemoryWithoutPerson);
        }
        let top_k = if request.top_k == 0 {
            self.params.top_k
        } else {
            request.top_k
        };
        let mut trace = Trace::default();

        let started = Instant::now();
        let (resolved, ambiguous, query, filters) = self.normalise(&request.query, &request.filters)?;
        trace.normalise_us = started.elapsed().as_micros();

        let started = Instant::now();
        let candidates = self.lexical(&query, &filters, self.params.candidates.max(top_k))?;
        trace.lexical_us = started.elapsed().as_micros();
        trace.lexical_candidates = candidates.len();

        let excluded: HashSet<&str> = request.exclude.iter().map(String::as_str).collect();

        // Memories first: they are cheap, and they feed confidence like any unit.
        let started = Instant::now();
        let mut ranked: Vec<Hit> = Vec::new();
        if !memories.is_empty() && filters.is_empty() {
            let stats = self.corpus_stats()?;
            let terms = memory::terms(&self.segmenter, &query);
            let weight = self.params.scope_weight.memory;
            for (id, text) in memories {
                let unweighted = memory::bm25(&self.segmenter, stats, &terms, text, |t| {
                    self.document_frequency(t)
                })?;
                if unweighted > 0.0 {
                    ranked.push(Hit {
                        item: Item::Memory {
                            id: id.to_string(),
                            text: text.to_string(),
                        },
                        source: Source::Memory,
                        score: unweighted * weight,
                        unweighted,
                        neighbours: Vec::new(),
                    });
                }
            }
        }
        trace.memory_us = started.elapsed().as_micros();

        let started = Instant::now();
        match &request.as_person {
            // Unweighted: order is already final, so only fetch what can be returned —
            // the window plus as many as exclusion might skip.
            None => {
                let excluded_ords: HashSet<i64> = self
                    .folio
                    .ords_for_ids(&request.exclude)?
                    .into_iter()
                    .collect();
                let wanted = top_k + candidates.iter().filter(|c| excluded_ords.contains(&c.ord)).count();
                let ords: Vec<i64> = candidates.iter().take(wanted).map(|c| c.ord).collect();
                let score: HashMap<i64, f64> = candidates.iter().map(|c| (c.ord, c.score)).collect();
                for unit in self.folio.units_by_ord(&ords)? {
                    let unweighted = score[&unit.ord];
                    ranked.push(Hit {
                        item: Item::Unit(unit),
                        source: Source::Unscoped,
                        score: unweighted,
                        unweighted,
                        neighbours: Vec::new(),
                    });
                }
                trace.fetch_us = started.elapsed().as_micros();
            }
            Some(person) => {
                let ords: Vec<i64> = candidates.iter().map(|c| c.ord).collect();
                let units = self.folio.units_by_ord(&ords)?;
                trace.fetch_us = started.elapsed().as_micros();
                let started = Instant::now();
                let score: HashMap<i64, f64> = candidates.iter().map(|c| (c.ord, c.score)).collect();
                let sources = self.sources(person, &units)?;
                for (unit, source) in units.into_iter().zip(sources) {
                    let unweighted = score[&unit.ord];
                    ranked.push(Hit {
                        score: unweighted * self.weight(source),
                        item: Item::Unit(unit),
                        source,
                        unweighted,
                        neighbours: Vec::new(),
                    });
                }
                trace.scope_us = started.elapsed().as_micros();
            }
        }

        let confidence = Confidence::of(
            &ranked.iter().map(|h| h.unweighted).collect::<Vec<_>>(),
            top_k,
            &self.params.confidence,
        );

        // Deterministic order: score, then unweighted score, then id.
        ranked.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(b.unweighted.total_cmp(&a.unweighted))
                .then_with(|| a.item.id().cmp(b.item.id()))
        });
        let mut hits = Vec::with_capacity(top_k);
        let mut skipped = Vec::new();
        for hit in ranked {
            if hits.len() >= top_k {
                break;
            }
            if excluded.contains(hit.item.id()) {
                skipped.push(hit.item.id().to_string());
            } else {
                hits.push(hit);
            }
        }

        let started = Instant::now();
        self.expand(&mut hits, &excluded)?;
        trace.expand_us = started.elapsed().as_micros();

        Ok(SearchResponse {
            hits,
            confidence,
            excluded: skipped,
            resolved,
            ambiguous,
            trace,
        })
    }

    /// One unit by id, with `neighbours` adjacent units on each side.
    pub fn get(&self, id: &str, neighbours: i64) -> Result<Option<(Unit, Vec<Unit>)>> {
        let Some(unit) = self.folio.unit_by_id(id)? else {
            return Ok(None);
        };
        let around = if neighbours > 0 {
            self.folio.neighbours(&unit, neighbours, neighbours)?
        } else {
            Vec::new()
        };
        Ok(Some((unit, around)))
    }

    /// People whose own units match `topic`, best first, at most `k` (`find_people.k`).
    ///
    /// `scope` is the candidate set: `None` for the whole roster, or the participants of a
    /// group. `exclude` removes people the caller must never offer (disabled ones); it is
    /// applied in SQL before the cut, so excluding someone never shortens the list.
    /// The alias stage expands the topic but never filters, since the point is to find
    /// people other than the one named.
    pub fn find_people(
        &self,
        topic: &str,
        scope: Option<&[PersonId]>,
        exclude: &[PersonId],
        k: usize,
    ) -> Result<People> {
        let (_, _, query, _) = self.normalise_with(topic, &Filters::default(), Normalise::Expand)?;
        let Some(expression) = self.segmenter.match_expression(&query) else {
            return Ok(self.people(Vec::new()));
        };
        if k == 0 || scope.is_some_and(<[PersonId]>::is_empty) {
            return Ok(self.people(Vec::new()));
        }
        let scope_json = scope.map(|s| serde_json::to_string(s).expect("strings serialise"));
        let exclude_json =
            (!exclude.is_empty()).then(|| serde_json::to_string(exclude).expect("strings serialise"));

        // The MATCH is materialised once, then joined to attribution. Ranking by the best
        // own unit (rather than a sum) keeps a person with one exact line ahead of one who
        // merely mentions the topic often.
        let mut stmt = self.folio.conn().prepare_cached(
            "WITH h AS MATERIALIZED (\
                 SELECT rowid AS ord, -bm25(chunks_fts) AS score \
                 FROM chunks_fts WHERE chunks_fts MATCH ?1), \
             p AS (\
                 SELECT u.person_id AS person, h.ord AS ord, h.score AS score, \
                        COUNT(*) OVER (PARTITION BY u.person_id) AS matched, \
                        ROW_NUMBER() OVER (PARTITION BY u.person_id \
                                           ORDER BY h.score DESC, h.ord) AS r \
                 FROM h JOIN chunks c ON c.ord = h.ord \
                        JOIN unit_persons u ON u.chunk_id = c.id \
                 WHERE (?2 IS NULL OR u.person_id IN (SELECT value FROM json_each(?2))) \
                   AND (?3 IS NULL OR u.person_id NOT IN (SELECT value FROM json_each(?3)))) \
             SELECT person, ord, score, matched FROM p WHERE r = 1 \
             ORDER BY score DESC, person LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![expression, scope_json, exclude_json, k as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let ords: Vec<i64> = rows.iter().map(|r| r.1).collect();
        let units: HashMap<i64, Unit> = self
            .folio
            .units_by_ord(&ords)?
            .into_iter()
            .map(|u| (u.ord, u))
            .collect();
        let bands = &self.params.confidence;
        let persons = rows
            .into_iter()
            .filter_map(|(person, ord, score, matched)| {
                // Several people can share one best unit, so clone rather than take.
                let reason = units.get(&ord)?.clone();
                Some(PersonMatch {
                    person,
                    score,
                    level: confidence::level_of_score(score, bands),
                    matched: matched as usize,
                    reason,
                })
            })
            .collect();
        Ok(self.people(persons))
    }

    /// Participants ranked by their best own-unit match for `text`. A participant with no
    /// matching own unit is absent.
    ///
    /// One call answers both group questions: who is addressed (the first entry, when
    /// any) and who may interject (the others at or above `interject.min_confidence`,
    /// i.e. `Level::High`).
    pub fn rank_participants(&self, text: &str, participants: &[PersonId]) -> Result<People> {
        self.find_people(text, Some(participants), &[], participants.len())
    }

    fn people(&self, persons: Vec<PersonMatch>) -> People {
        let scores: Vec<f64> = persons.iter().map(|p| p.score).collect();
        People {
            confidence: Confidence::of(&scores, scores.len().max(1), &self.params.confidence),
            persons,
        }
    }

    fn weight(&self, source: Source) -> f64 {
        let w = &self.params.scope_weight;
        match source {
            Source::Own => w.own,
            Source::Lived => w.lived,
            Source::InScope => w.in_scope,
            Source::OutOfScope | Source::Unscoped => w.out_of_scope,
            Source::Memory => w.memory,
        }
    }

    /// Classify units against a person's knowledge: own and lived from the materialised
    /// scope (attribution also counts as own, which is what `self` means), then topic terms
    /// for lore-shaped units.
    fn sources(&self, person: &str, units: &[Unit]) -> Result<Vec<Source>> {
        let ids: Vec<String> = units.iter().map(|u| u.id.clone()).collect();
        let scope = self.folio.knowledge_scope(person, &ids)?;
        let manifest = self.folio.manifest();
        let mut terms: Option<Vec<String>> = None;
        let mut out = Vec::with_capacity(units.len());
        for unit in units {
            let source = if unit.persons.iter().any(|p| p == person) {
                Source::Own
            } else {
                match scope.get(&unit.id) {
                    Some(folio::Scope::Own) => Source::Own,
                    Some(folio::Scope::Lived) => Source::Lived,
                    None if manifest.shape_of(&unit.template) == "lore" => {
                        if terms.is_none() {
                            terms = Some(
                                self.folio
                                    .topic_terms(person)?
                                    .into_iter()
                                    .map(|t| t.to_lowercase())
                                    .collect(),
                            );
                        }
                        let terms = terms.as_deref().unwrap_or_default();
                        let haystack = format!("{}\n{}\n{}", unit.title, unit.header, unit.text)
                            .to_lowercase();
                        if terms.iter().any(|t| !t.is_empty() && haystack.contains(t.as_str())) {
                            Source::InScope
                        } else {
                            Source::OutOfScope
                        }
                    }
                    None => Source::OutOfScope,
                }
            };
            out.push(source);
        }
        Ok(out)
    }

    /// Widen returned unit hits to their neighbours. Neighbours that are themselves hits,
    /// or already in the session, are dropped: they would be sent twice.
    fn expand(&self, hits: &mut [Hit], excluded: &HashSet<&str>) -> Result<()> {
        let n = self.params.neighbours;
        if n <= 0 {
            return Ok(());
        }
        let returned: HashSet<String> = hits.iter().map(|h| h.item.id().to_string()).collect();
        for hit in hits.iter_mut() {
            let Item::Unit(unit) = &hit.item else {
                continue;
            };
            let mut around = self.folio.neighbours(unit, n, n)?;
            around.retain(|u| !returned.contains(&u.id) && !excluded.contains(u.id.as_str()));
            hit.neighbours = around;
        }
        Ok(())
    }

    fn corpus_stats(&self) -> Result<&memory::CorpusStats> {
        if let Some(stats) = self.stats.get() {
            return Ok(stats);
        }
        let stats = memory::CorpusStats::read(self.folio.conn())?;
        Ok(self.stats.get_or_init(|| stats))
    }

    fn document_frequency(&self, term: &str) -> Result<i64> {
        memory::document_frequency(self.folio.conn(), term)
    }

    fn normalise(
        &self,
        query: &str,
        filters: &Filters,
    ) -> Result<(Option<Resolved>, Option<Ambiguity>, String, Filters)> {
        self.normalise_with(query, filters, self.normalise)
    }

    /// The alias stage: turn a query that names an entity by another name into one that
    /// names it by the name the corpus actually uses.
    ///
    /// Two rules, both lookups against relations the source declares; neither infers
    /// anything (no name similarity, no edit distance), because a wrong inference about who
    /// someone is would be undiscoverable in the output.
    ///
    /// **Whole query is an alias.** Deliberately narrow: a name can be both a redirect and
    /// an ordinary noun, so substituting it mid-sentence would rewrite queries that were
    /// already right.
    ///
    /// **Alias plus a qualifier naming one of its own candidates.** When the source lists
    /// several things sharing a name, `<name> <qualifier>` with the qualifier one of those
    /// candidates has named one of them.
    ///
    /// Expansion *adds* to the query rather than replacing it, so an alias that is also
    /// real text still matches where it literally occurs.
    fn normalise_with(
        &self,
        query: &str,
        filters: &Filters,
        normalise: Normalise,
    ) -> Result<(Option<Resolved>, Option<Ambiguity>, String, Filters)> {
        let unchanged = || (None, None, query.to_string(), filters.clone());
        let trimmed = query.trim();
        if normalise == Normalise::Off || trimmed.is_empty() {
            return Ok(unchanged());
        }

        let (alias, target, how) = match &self.folio.resolve_alias(trimmed)?[..] {
            [only] => (trimmed.to_string(), only.clone(), How::Unique),
            [] => match self.qualified(trimmed)? {
                Some((alias, target)) => (alias, target, How::Qualified),
                None => return Ok(unchanged()),
            },
            several => {
                let (_, _, query, filters) = unchanged();
                return Ok((
                    None,
                    Some(Ambiguity {
                        alias: trimmed.to_string(),
                        candidates: several.to_vec(),
                    }),
                    query,
                    filters,
                ));
            }
        };

        let person = match how {
            // A qualified hit names one target out of several, so the person is that
            // target when it is on the roster, not the alias's whole candidate set.
            How::Qualified => self.folio.person(&target)?.map(|p| p.person_id),
            How::Unique => match &self.folio.persons_for_alias(&alias)?[..] {
                [only] => Some(only.clone()),
                _ => None,
            },
        };

        let mut effective = filters.clone();
        if normalise == Normalise::ExpandAndFilter
            && filters.persons.is_empty()
            && let Some(person) = &person
        {
            effective.persons = vec![person.clone()];
        }
        Ok((
            Some(Resolved {
                alias,
                target: target.clone(),
                person,
                how,
            }),
            None,
            format!("{trimmed} {target}"),
            effective,
        ))
    }

    /// An ambiguous alias whose query also names one of its declared candidates.
    ///
    /// The candidate must appear in the query as written. Matching on anything looser would
    /// be inference, and the whole point of using the source's disambiguation pages is that the
    /// answer is already enumerated.
    fn qualified(&self, query: &str) -> Result<Option<(String, String)>> {
        for token in self.segmenter.segment_for_query(query) {
            let candidates = self.folio.resolve_alias(&token)?;
            if candidates.len() < 2 {
                continue;
            }
            let rest = query.replacen(token.as_str(), " ", 1);
            // Longest candidate first, so a name wins over another that is its prefix.
            let mut sorted = candidates;
            sorted.sort_by_key(|c| std::cmp::Reverse(c.chars().count()));
            if let Some(candidate) = sorted.into_iter().find(|c| rest.contains(c.as_str())) {
                return Ok(Some((token, candidate)));
            }
        }
        Ok(None)
    }

    /// BM25 candidates, best first, with every filter applied in SQL before the cut.
    ///
    /// FTS5 returns `bm25()` negative-is-better; it is negated so scores improve upward.
    /// Filtering after the cut is the failure this shape exists to prevent: a query naming
    /// a person can place that person's first unit just outside the window, and a list
    /// filtered afterwards is then empty.
    ///
    /// Two fixed statement shapes, both cached. The unfiltered one skips the join to
    /// `chunks`, which costs a noticeable share of the budget even when every filter
    /// predicate short-circuits on NULL.
    fn lexical(&self, query: &str, filters: &Filters, limit: usize) -> Result<Vec<Candidate>> {
        let Some(expression) = self.segmenter.match_expression(query) else {
            return Ok(Vec::new());
        };
        let limit = limit as i64;
        let read = |row: &rusqlite::Row<'_>| {
            Ok(Candidate {
                ord: row.get(0)?,
                score: row.get(1)?,
            })
        };

        if filters.is_empty() {
            let mut stmt = self.folio.conn().prepare_cached(
                "SELECT rowid, -bm25(chunks_fts) AS score \
                 FROM chunks_fts WHERE chunks_fts MATCH ?1 \
                 ORDER BY score DESC, rowid LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![expression, limit], read)?;
            return rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into);
        }

        // An empty list is bound as SQL NULL and its predicate short-circuits, so one
        // shape covers every combination.
        let json = |list: &[String]| {
            (!list.is_empty()).then(|| serde_json::to_string(list).expect("strings serialise"))
        };
        let mut stmt = self.folio.conn().prepare_cached(
            "SELECT f.rowid, -bm25(chunks_fts) AS score \
             FROM chunks_fts f JOIN chunks c ON c.ord = f.rowid \
             WHERE chunks_fts MATCH ?1 \
               AND (?3 IS NULL OR c.template IN (SELECT value FROM json_each(?3))) \
               AND (?4 IS NULL OR EXISTS (SELECT 1 FROM unit_persons u \
                    WHERE u.chunk_id = c.id \
                      AND u.person_id IN (SELECT value FROM json_each(?4)))) \
               AND (?5 IS NULL OR c.page IN (SELECT value FROM json_each(?5))) \
             ORDER BY score DESC, f.rowid LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                expression,
                limit,
                json(&filters.templates),
                json(&filters.persons),
                json(&filters.pages)
            ],
            read,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }
}
