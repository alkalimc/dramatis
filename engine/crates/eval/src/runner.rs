//! Running a configuration of the retriever over the suite.
//!
//! A configuration is a *system* under test, and the harness exists to compare systems
//! rather than to produce one number. The interesting comparisons are structural: does
//! restricting to a resolved person help the families that were built to test exactly that?
//! Answering it requires two runs over identical queries, which is why this returns
//! per-query scores instead of an aggregate.

use folio::Folio;
use index::{Index, Mode, Normalise, Request};

use crate::error::Result;
use crate::metrics::{score, Scored};
use crate::suite::Suite;

/// One system under test.
#[derive(Debug, Clone)]
pub struct Config {
    /// Cut-off for every metric. Reported in the metric's name, never left implicit.
    pub k: usize,
    /// Candidates each path contributes before fusion.
    pub candidates: usize,
    pub mode: Mode,
    /// How many hard-negative losses to keep for inspection. 0 keeps none.
    pub capture_failures: usize,
    /// What the alias stage is allowed to do. The three settings are the three systems
    /// worth comparing, and the suite's `redirect` / `realname` / `alter` families exist to
    /// separate them.
    pub normalise: Normalise,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            k: 10,
            candidates: 50,
            mode: Mode::Lexical,
            normalise: Normalise::Off,
            capture_failures: 0,
        }
    }
}

impl Config {
    /// A stable name for reports, so a result can never be quoted without its settings.
    pub fn label(&self) -> String {
        let mode = match self.mode {
            Mode::Lexical => "lexical",
            Mode::Dense => "dense",
            Mode::Hybrid => "hybrid",
        };
        let alias = match self.normalise {
            Normalise::Off => "",
            Normalise::Expand => "+expand",
            Normalise::ExpandAndFilter => "+expand+filter",
        };
        format!("{mode}{alias}@{}·c{}", self.k, self.candidates)
    }
}

/// A query where a hard negative outranked every relevant unit.
///
/// Captured because a rate is not a diagnosis. "13% of voice queries lose to a hard
/// negative" is a number to act on only if the cases can be read, and a harness that
/// reports the rate without being able to show one case is a scoreboard, not a tool.
#[derive(Debug, Clone)]
pub struct Failure {
    pub qid: String,
    pub family: String,
    pub text: String,
    /// The winning negative, and where it ranked.
    pub negative: String,
    pub negative_rank: usize,
    /// The best relevant unit's rank, if it was returned at all.
    pub gold_rank: Option<usize>,
    pub ranked: Vec<String>,
}

pub struct Outcome {
    pub scored: Vec<Scored>,
    /// Hard-negative losses, capped by `Config::capture_failures`.
    pub failures: Vec<Failure>,
    /// Wall-clock microseconds per query, in query order. Kept alongside quality because a
    /// configuration that wins on nDCG and loses the latency budget has not won.
    pub latencies_us: Vec<u128>,
    /// Queries whose gold set was empty, and so cannot be scored either way.
    pub unscoreable: usize,
}

impl Outcome {
    pub fn latency_percentile(&self, p: f64) -> u128 {
        if self.latencies_us.is_empty() {
            return 0;
        }
        let mut sorted = self.latencies_us.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[idx]
    }
}

/// Score one configuration over the whole suite.
pub fn run(
    folio: &Folio,
    suite: &Suite,
    config: &Config,
    mut progress: impl FnMut(usize, usize),
) -> Result<Outcome> {
    let index = Index::new(folio);
    let mut scored = Vec::with_capacity(suite.queries.len());
    let mut latencies_us = Vec::with_capacity(suite.queries.len());
    let mut failures: Vec<Failure> = Vec::new();
    let mut unscoreable = 0usize;

    for (i, query) in suite.queries.iter().enumerate() {
        if query.gold.is_empty() {
            // Scoring a query with no gold as 0.0 would be a silent lie in every average.
            unscoreable += 1;
            continue;
        }

        let request = Request {
            query: query.text.clone(),
            mode: config.mode,
            top_k: config.k,
            templates: Vec::new(),
            persons: Vec::new(),
            candidates: config.candidates,
            normalise: config.normalise,
            expand: 0,
        };

        let started = std::time::Instant::now();
        let response = index.search(&request, None)?;
        latencies_us.push(started.elapsed().as_micros());

        let ranked: Vec<String> = response.hits.iter().map(|h| h.unit.id.clone()).collect();
        let outcome = score(query, &ranked, config.k);
        if outcome.negative_won && failures.len() < config.capture_failures {
            let negatives: std::collections::HashSet<&str> =
                query.hard_negatives.iter().map(String::as_str).collect();
            let position = |wanted: &dyn Fn(&str) -> bool| {
                ranked.iter().take(config.k).position(|id| wanted(id.as_str()))
            };
            let negative_rank = position(&|id| negatives.contains(id)).unwrap_or(0);
            failures.push(Failure {
                qid: query.qid.clone(),
                family: query.family.clone(),
                text: query.text.clone(),
                negative: ranked.get(negative_rank).cloned().unwrap_or_default(),
                negative_rank,
                gold_rank: position(&|id| query.gold.contains_key(id)),
                ranked: ranked.iter().take(config.k).cloned().collect(),
            });
        }
        scored.push(outcome);

        if i % 500 == 0 {
            progress(i, suite.queries.len());
        }
    }
    progress(suite.queries.len(), suite.queries.len());

    Ok(Outcome { scored, failures, latencies_us, unscoreable })
}

/// Per-query nDCG aligned across two runs, for the paired test.
///
/// Pairs by qid rather than by position: a configuration that skips a query for any reason
/// would otherwise shift every subsequent pair by one and produce a confident comparison
/// between unrelated queries.
pub fn align(a: &[Scored], b: &[Scored]) -> (Vec<f64>, Vec<f64>) {
    let index_b: std::collections::HashMap<&str, &Scored> =
        b.iter().map(|s| (s.qid.as_str(), s)).collect();
    let mut left = Vec::new();
    let mut right = Vec::new();
    for s in a {
        if let Some(other) = index_b.get(s.qid.as_str()) {
            left.push(s.ndcg);
            right.push(other.ndcg);
        }
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(qid: &str, ndcg: f64) -> Scored {
        Scored {
            qid: qid.into(),
            family: "f".into(),
            stratum: "paraphrase".into(),
            ndcg,
            precision: 0.0,
            recall: 0.0,
            gold_size: 1,
            rr: 0.0,
            negative_won: false,
            retrieved: 1,
        }
    }

    #[test]
    fn alignment_pairs_by_qid_not_position() {
        let a = vec![scored("q1", 0.1), scored("q2", 0.2), scored("q3", 0.3)];
        // The second run is missing q2 and holds the rest in a different order.
        let b = vec![scored("q3", 0.9), scored("q1", 0.5)];
        let (left, right) = align(&a, &b);
        assert_eq!(left, vec![0.1, 0.3]);
        assert_eq!(right, vec![0.5, 0.9]);
    }

    #[test]
    fn a_label_carries_every_setting_that_changes_the_number() {
        let config = Config {
            k: 6,
            candidates: 100,
            mode: Mode::Hybrid,
            normalise: Normalise::ExpandAndFilter,
            ..Config::default()
        };
        assert_eq!(config.label(), "hybrid+expand+filter@6·c100");
    }
}
