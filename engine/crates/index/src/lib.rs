//! Hybrid retrieval over a `.folio`.
//!
//! ```text
//! query ─ alias expansion ─┬─ lexical: FTS5 + BM25 over pre-segmented tokens ──┐
//!                          └─ dense:   exact SIMD-shaped scan over f16 vectors ┘
//!                                   → RRF fusion → (rerank) → top-k
//!                                                      └→ neighbour expansion, hits only
//! ```
//!
//! Three properties of this pipeline are consequences of measurement rather than taste.
//!
//! **Alias expansion is not optional.** Measured: a query naming an entity by a foreign-
//! language redirect (`バブル`, `Whislash`) matches *zero* units, because the alias occurs
//! nowhere in the corpus text — the wiki records it as a redirect page, not as prose. The
//! table lookup is the only path to those entities, so the retriever without it scores 0.03
//! nDCG on 1,108 such queries.
//!
//! **The dense path is exact.** 58,853 units at 1024 dimensions is 120 MB, which fits the
//! resident budget, so there is no approximate index — one fewer dependency, no build
//! step, and no approximation error to contaminate the dimension-ablation curve.
//!
//! **There is no span-merge stage.** Units are stored exactly once. An earlier corpus
//! design overlapped dialogue windows and needed a merge pass to stop a top-6 being three
//! passages shown twice; storing once and widening ranked hits on demand is cheaper and
//! gives truer context.

pub mod segment;

use std::collections::HashMap;

use folio::{Folio, Unit};

pub use segment::Segmenter;

/// What to do with a query that names something the site records under another name.
///
/// Redirects, real-name tables and alternate-form entries are synonymy the wiki's editors
/// wrote by hand. Using it is a table lookup, so it costs nothing worth measuring; *not*
/// using it means an entity whose only English name is a redirect page is unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Normalise {
    /// Take the query as typed.
    Off,
    /// Add the canonical name's tokens to the query.
    Expand,
    /// Add the tokens, and when the alias names exactly one person, restrict to them.
    #[default]
    ExpandAndFilter,
}

/// What the alias stage found. Carried on the response so a caller can say what it
/// understood the query to mean, rather than silently answering a different question.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub alias: String,
    pub target: String,
    /// The roster id, when the target is a person. Aliases also point at operations,
    /// locations and items, which have no person to filter by.
    pub person: Option<String>,
    /// How the single target was arrived at.
    pub how: How,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// The alias names exactly one thing.
    Unique,
    /// The alias names several, and the rest of the query named one of them.
    Qualified,
}

/// The alias names several things, and nothing in the query said which.
///
/// This is a *reportable state*, not a failure. The site maintains a disambiguation page
/// listing eight different things called 小车; a retriever that silently picked one would be
/// answering a question the user did not ask, and one that returned nothing would be hiding
/// information the corpus has. The right behaviour is to answer broadly and let the caller
/// ask which — which it can only do if the candidates reach it.
#[derive(Debug, Clone)]
pub struct Ambiguity {
    pub alias: String,
    /// Every target the site declares for this name, in a stable order.
    pub candidates: Vec<String>,
}

/// Which paths to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Both paths, fused. Requires vectors.
    #[default]
    Hybrid,
    /// Lexical only — exact nouns, and the only mode available without an encoder.
    Lexical,
    /// Dense only.
    Dense,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub query: String,
    pub mode: Mode,
    pub top_k: usize,
    /// Restrict to these templates. Empty means all.
    pub templates: Vec<String>,
    /// Restrict to these people. A caller should pass `person_id`s already resolved
    /// through the alias dictionary, so an alternate-form name reaches the one person.
    pub persons: Vec<String>,
    /// How many candidates each path contributes before fusion.
    pub candidates: usize,
    /// Whether to consult the alias dictionary. See [`Normalise`].
    pub normalise: Normalise,
    /// Widen each returned hit by this many neighbouring units on each side.
    pub expand: i64,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: Mode::default(),
            top_k: 6,
            templates: Vec::new(),
            persons: Vec::new(),
            candidates: 50,
            normalise: Normalise::default(),
            expand: 0,
        }
    }
}

/// How sure we are that the corpus contains an answer at all.
///
/// Three signals, each measuring something the others miss. The third exists because the
/// first two both stay quiet in the case that matters most: when every candidate is
/// middlingly relevant, top-1 looks acceptable and the paths agree, yet the right
/// behaviour is to admit the archive has no clear answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone)]
pub struct Signals {
    /// Best fused score. Low means nothing relevant exists.
    pub top_score: f64,
    /// Overlap between the two paths' candidate sets. Low means lexical and semantic
    /// matching disagree about what the query is even about.
    pub path_agreement: f64,
    /// Normalised entropy of the score distribution. High means flat — many candidates,
    /// none of them clearly right.
    pub score_entropy: f64,
    pub level: Confidence,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub unit: Unit,
    pub score: f64,
    pub lexical_rank: Option<usize>,
    pub dense_rank: Option<usize>,
    /// Units adjacent in the source sequence, when `expand > 0`.
    pub context: Vec<Unit>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub hits: Vec<Hit>,
    pub signals: Signals,
    /// Set when the query named an entity by an alias.
    pub resolved: Option<Resolved>,
    /// Set when the query named something the site records as ambiguous and the query did
    /// not say which. The hits are still the unrestricted best effort.
    pub ambiguous: Option<Ambiguity>,
    pub trace: Trace,
}

/// Per-stage timings. Latency is a design constraint here, so it is measured by default
/// rather than behind a flag: a number nobody collects is a number nobody can defend.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub normalise_us: u128,
    pub lexical_us: u128,
    pub dense_us: u128,
    pub fuse_us: u128,
    pub fetch_us: u128,
    pub expand_us: u128,
    pub lexical_candidates: usize,
    pub dense_candidates: usize,
    pub scanned: usize,
}

impl Trace {
    pub fn total_us(&self) -> u128 {
        self.normalise_us
            + self.lexical_us
            + self.dense_us
            + self.fuse_us
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
    #[error("{mode:?} needs vectors, which this corpus has not had written yet")]
    VectorsRequired { mode: Mode },
    #[error("dense search needs a query embedding")]
    EmbeddingRequired,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Reciprocal-rank-fusion constant. 60 is the value from the original description and is
/// kept because there is no local evidence to justify moving it; the ablation matrix has a
/// slot for revisiting it against the structural benchmark.
const RRF_K: f64 = 60.0;

pub struct Index<'a> {
    folio: &'a Folio,
    segmenter: Segmenter,
}

impl<'a> Index<'a> {
    /// Build an index view over an opened corpus.
    ///
    /// The segmenter is chosen from the corpus manifest, not configured: it must match what
    /// built the lexical index, and making that a caller's choice would invite a mismatch
    /// whose only symptom is quietly worse results.
    pub fn new(folio: &'a Folio) -> Self {
        let segmenter = Segmenter::for_corpus(folio.manifest().segmenter_name());
        Self { folio, segmenter }
    }

    pub fn segmenter(&self) -> &Segmenter {
        &self.segmenter
    }

    /// Search. `embedding` is required for any mode that uses the dense path.
    pub fn search(&self, request: &Request, embedding: Option<&[f32]>) -> Result<Response> {
        let mut trace = Trace::default();

        let started = std::time::Instant::now();
        let (resolved, ambiguous, effective) = self.normalise(request)?;
        trace.normalise_us = started.elapsed().as_micros();
        let request = &effective;

        let lexical = match request.mode {
            Mode::Dense => Vec::new(),
            _ => {
                let started = std::time::Instant::now();
                let hits = self.lexical(request)?;
                trace.lexical_us = started.elapsed().as_micros();
                trace.lexical_candidates = hits.len();
                hits
            }
        };

        let dense = match request.mode {
            Mode::Lexical => Vec::new(),
            _ => {
                let vectors = self
                    .folio
                    .vectors()
                    .ok_or(Error::VectorsRequired { mode: request.mode })?;
                let embedding = embedding.ok_or(Error::EmbeddingRequired)?;
                let started = std::time::Instant::now();
                let hits = vectors.top_k(embedding, request.candidates)?;
                trace.dense_us = started.elapsed().as_micros();
                trace.dense_candidates = hits.len();
                trace.scanned = vectors.count();
                // Widen to f64 here, at the boundary: the scan works in f32 because that is what
                // the vectors are, and everything downstream of fusion works in f64 because
                // BM25 does. Converting once at the seam beats sprinkling casts through both.
                hits.into_iter()
                    .map(|(ord, score)| (ord as i64, score as f64))
                    .collect()
            }
        };

        let started = std::time::Instant::now();
        let fused = fuse(&lexical, &dense);
        let signals = signals(&fused, &lexical, &dense);
        trace.fuse_us = started.elapsed().as_micros();

        let started = std::time::Instant::now();
        // Filtering must happen before truncating to top_k — filtering an already-cut list
        // silently returns fewer results than asked for, or none when the filter excludes
        // whatever the paths ranked highest (found by asking for two dialogue units and
        // getting zero, from a corpus holding 32,322).
        //
        // But it must not mean fetching every candidate either: doing that cost 8 ms of the
        // 13 ms budget. Filtering is expressed in SQL so the database applies it while
        // fetching, and only the ordinals actually needed come back.
        let ords: Vec<i64> = fused.iter().map(|f| f.ord).collect();
        let units = self.folio.units_filtered(
            &ords,
            &request.templates,
            &request.persons,
            request.top_k,
        )?;
        trace.fetch_us = started.elapsed().as_micros();

        let by_ord: HashMap<i64, &Fused> = fused.iter().map(|f| (f.ord, f)).collect();
        let mut hits: Vec<Hit> = units
            .into_iter()
            .map(|unit| {
                let entry = by_ord.get(&unit.ord);
                Hit {
                    score: entry.map(|f| f.score).unwrap_or(0.0),
                    lexical_rank: entry.and_then(|f| f.lexical_rank),
                    dense_rank: entry.and_then(|f| f.dense_rank),
                    context: Vec::new(),
                    unit,
                }
            })
            .collect();

        if request.expand > 0 {
            let started = std::time::Instant::now();
            for hit in &mut hits {
                hit.context = self
                    .folio
                    .neighbours(&hit.unit, request.expand, request.expand)?;
            }
            trace.expand_us = started.elapsed().as_micros();
        }

        Ok(Response { hits, signals, resolved, ambiguous, trace })
    }

    /// The alias stage: turn a query that names an entity by another name into one that
    /// names it by the name the corpus actually uses.
    ///
    /// Two rules, both lookups against relations the site declares. Neither infers anything:
    /// no name similarity, no edit distance. A wrong inference about who someone is would be
    /// undiscoverable in the output, which is why the contract forbids it outright
    /// (`identity.md`).
    ///
    /// **Whole query is an alias.** Deliberately narrow. `锋刃` is both a redirect to an
    /// operator and an ordinary noun, so substituting it mid-sentence would rewrite queries
    /// that were already right. The narrow form fires on bare-name lookups — the case that is
    /// otherwise unanswerable, since a foreign-language redirect appears nowhere in the text.
    ///
    /// **Alias plus a qualifier naming one of its own candidates.** The site's
    /// disambiguation page lists eight things called 小车; a query saying `小车 Castle-3`
    /// has named one of them. Reading that is not guessing, because the candidate list came
    /// from the site.
    ///
    /// Expansion *adds* to the query rather than replacing it. An alias that is also real
    /// text — a nickname a character is called in dialogue — should still match where it
    /// literally occurs.
    fn normalise(
        &self,
        request: &Request,
    ) -> Result<(Option<Resolved>, Option<Ambiguity>, Request)> {
        if request.normalise == Normalise::Off {
            return Ok((None, None, request.clone()));
        }
        let query = request.query.trim();
        if query.is_empty() {
            return Ok((None, None, request.clone()));
        }

        let (alias, target, how) = match &self.folio.resolve_alias(query)?[..] {
            [only] => (query.to_string(), only.clone(), How::Unique),
            [] => match self.qualified(query)? {
                Some((alias, target)) => (alias, target, How::Qualified),
                None => return Ok((None, None, request.clone())),
            },
            several => {
                // The whole query is a name the site itself records as ambiguous. Report the
                // candidates and leave the query alone: picking one would answer a different
                // question, and the disambig family measures exactly this.
                return Ok((
                    None,
                    Some(Ambiguity {
                        alias: query.to_string(),
                        candidates: several.to_vec(),
                    }),
                    request.clone(),
                ));
            }
        };

        let person = match how {
            // A qualified hit names one target out of several, so the person is that target
            // when it is on the roster — not the alias's whole candidate set.
            How::Qualified => self.folio.person(&target)?.map(|p| p.person_id),
            How::Unique => match &self.folio.persons_for_alias(&alias)?[..] {
                [only] => Some(only.clone()),
                _ => None,
            },
        };

        let mut effective = request.clone();
        effective.query = format!("{query} {target}");
        if request.normalise == Normalise::ExpandAndFilter
            && request.persons.is_empty()
            && let Some(person) = &person
        {
            effective.persons = vec![person.clone()];
        }
        Ok((Some(Resolved { alias, target, person, how }), None, effective))
    }

    /// An ambiguous alias whose query also names one of its declared candidates.
    ///
    /// The candidate must appear in the query as written. Matching on anything looser would
    /// be inference, and the whole point of using the site's disambiguation pages is that the
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

    /// BM25 over the pre-segmented column.
    ///
    /// FTS5 returns `bm25()` as a negative number where more negative is better; it is
    /// negated here so every score in this crate improves upward.
    ///
    /// **Filters are applied here, not after fusion.** Measured: a query naming 梅尔 puts her
    /// first unit at candidate rank 52, so a top-50 candidate list filtered afterwards is
    /// empty — the retriever returns nothing about a person it holds 56 units on. This is
    /// the same failure P11 found between fusion and truncation, one stage earlier, and it
    /// has the same fix: the filter belongs where the candidates are chosen.
    ///
    /// There are two statement shapes, not one, and both are cached. The unfiltered shape
    /// is the common case and measurably cheaper: joining `chunks` costs 0.3 ms even when
    /// the filter predicate short-circuits on NULL, because SQLite still probes the index
    /// per posting-list row. Two fixed shapes keep preparation cached while letting the
    /// common path stay at the 1.6 ms P11 measured.
    fn lexical(&self, request: &Request) -> Result<Vec<(i64, f64)>> {
        let Some(expression) = self.segmenter.match_expression(&request.query) else {
            return Ok(Vec::new());
        };
        let limit = request.candidates as i64;
        let read = |row: &rusqlite::Row<'_>| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?));

        if request.templates.is_empty() && request.persons.is_empty() {
            let mut stmt = self.folio.conn().prepare_cached(
                "SELECT rowid, -bm25(chunks_fts) AS score \
                 FROM chunks_fts WHERE chunks_fts MATCH ?1 \
                 ORDER BY score DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![expression, limit], read)?;
            return rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into);
        }

        // One filtered shape covers all three remaining combinations: an empty list is bound
        // as SQL NULL and the predicate short-circuits, so this prepares once rather than
        // once per combination.
        let templates_json = (!request.templates.is_empty())
            .then(|| serde_json::to_string(&request.templates).expect("strings serialise"));
        let persons_json = (!request.persons.is_empty())
            .then(|| serde_json::to_string(&request.persons).expect("strings serialise"));
        let mut stmt = self.folio.conn().prepare_cached(
            "SELECT f.rowid, -bm25(chunks_fts) AS score \
             FROM chunks_fts f JOIN chunks c ON c.ord = f.rowid \
             WHERE chunks_fts MATCH ?1 \
               AND (?3 IS NULL OR c.template IN (SELECT value FROM json_each(?3))) \
               AND (?4 IS NULL OR c.person IN (SELECT value FROM json_each(?4))) \
             ORDER BY score DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![expression, limit, templates_json, persons_json],
            read,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }
}

#[derive(Debug, Clone)]
struct Fused {
    ord: i64,
    score: f64,
    lexical_rank: Option<usize>,
    dense_rank: Option<usize>,
}

/// Reciprocal rank fusion.
///
/// Ranks rather than raw scores, because BM25 and cosine are not on comparable scales and
/// never will be — normalising them against each other would mean inventing a conversion
/// with no basis. Rank fusion needs no such invention.
fn fuse(lexical: &[(i64, f64)], dense: &[(i64, f64)]) -> Vec<Fused> {
    let mut merged: HashMap<i64, Fused> = HashMap::new();

    for (rank, (ord, _)) in lexical.iter().enumerate() {
        let entry = merged.entry(*ord).or_insert(Fused {
            ord: *ord,
            score: 0.0,
            lexical_rank: None,
            dense_rank: None,
        });
        entry.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        entry.lexical_rank = Some(rank);
    }
    for (rank, (ord, _)) in dense.iter().enumerate() {
        let entry = merged.entry(*ord).or_insert(Fused {
            ord: *ord,
            score: 0.0,
            lexical_rank: None,
            dense_rank: None,
        });
        entry.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        entry.dense_rank = Some(rank);
    }

    let mut out: Vec<Fused> = merged.into_values().collect();
    // Tie-break by ordinal so the same query on the same corpus always returns the same
    // order. Reproducibility is not optional when the ablation matrix depends on it.
    out.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then(a.ord.cmp(&b.ord)));
    out
}

fn signals(fused: &[Fused], lexical: &[(i64, f64)], dense: &[(i64, f64)]) -> Signals {
    let top_score = fused.first().map(|f| f.score).unwrap_or(0.0);

    // Jaccard over the two candidate sets.
    let lexical_set: std::collections::HashSet<i64> = lexical.iter().map(|(ord, _)| *ord).collect();
    let dense_set: std::collections::HashSet<i64> = dense.iter().map(|(ord, _)| *ord).collect();
    let path_agreement = if lexical_set.is_empty() || dense_set.is_empty() {
        // Only one path ran, so agreement is not measurable. Reporting 0 would read as
        // "the paths disagree", which is a different and false claim.
        f64::NAN
    } else {
        let intersection = lexical_set.intersection(&dense_set).count() as f64;
        let union = lexical_set.union(&dense_set).count() as f64;
        intersection / union
    };

    // Shannon entropy of the fused scores, normalised to [0, 1] by log(n).
    let total: f64 = fused.iter().map(|f| f.score).sum();
    let score_entropy = if total <= 0.0 || fused.len() < 2 {
        0.0
    } else {
        let raw: f64 = fused
            .iter()
            .map(|f| f.score / total)
            .filter(|p| *p > 0.0)
            .map(|p| -p * p.ln())
            .sum();
        raw / (fused.len() as f64).ln()
    };

    // Thresholds are placeholders pending calibration against the three benchmark suites;
    // the shape of the rule is what is being fixed here, not the constants.
    let level = if fused.is_empty() || top_score < 0.012 {
        Confidence::Low
    } else if score_entropy > 0.92 || (!path_agreement.is_nan() && path_agreement < 0.05) {
        Confidence::Medium
    } else {
        Confidence::High
    };

    Signals {
        top_score,
        path_agreement,
        score_entropy,
        level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fusion_rewards_appearing_in_both_paths() {
        let lexical = vec![(1, 5.0), (2, 4.0)];
        let dense = vec![(2, 0.9), (3, 0.8)];
        let fused = fuse(&lexical, &dense);
        assert_eq!(fused[0].ord, 2, "the unit both paths found should lead");
        assert!(fused[0].lexical_rank.is_some() && fused[0].dense_rank.is_some());
    }

    #[test]
    fn fusion_is_deterministic_on_ties() {
        let lexical = vec![(7, 1.0), (3, 1.0)];
        let first = fuse(&lexical, &[]);
        let second = fuse(&lexical, &[]);
        assert_eq!(
            first.iter().map(|f| f.ord).collect::<Vec<_>>(),
            second.iter().map(|f| f.ord).collect::<Vec<_>>()
        );
    }

    #[test]
    fn agreement_is_not_a_number_when_only_one_path_ran() {
        let signals = signals(&fuse(&[(1, 1.0)], &[]), &[(1, 1.0)], &[]);
        assert!(
            signals.path_agreement.is_nan(),
            "reporting 0 would claim the paths disagreed"
        );
    }

    #[test]
    fn no_candidates_means_low_confidence() {
        let signals = signals(&[], &[], &[]);
        assert_eq!(signals.level, Confidence::Low);
    }

    #[test]
    fn a_flat_distribution_is_not_high_confidence() {
        // Fifty candidates all scoring alike: nothing is clearly right, which is the case
        // top-1 and path agreement both fail to notice.
        let lexical: Vec<(i64, f64)> = (0..50).map(|i| (i, 1.0)).collect();
        let dense: Vec<(i64, f64)> = (0..50).map(|i| (i, 1.0)).collect();
        let signals = signals(&fuse(&lexical, &dense), &lexical, &dense);
        assert_ne!(signals.level, Confidence::High);
    }
}
