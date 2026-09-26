//! Reading the benchmark.
//!
//! The suite ships as JSONL because it is meant to be published and read by other
//! people's tooling, not just ours.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

/// One graded query.
#[derive(Debug, Clone, Deserialize)]
pub struct Query {
    pub qid: String,
    /// Which relation produced it. Metrics are reported per family; a single mean across
    /// families is not interpretable, because they test different claims.
    pub family: String,
    pub text: String,
    /// `verbatim` when the query string occurs in its gold passage, `paraphrase` otherwise.
    ///
    /// The distinction is the difference between measuring retrieval and measuring string
    /// matching, so it is carried on every query rather than inferred later.
    pub stratum: String,
    /// unit id → graded relevance. 2 = the passage the relation points at, 1 = same entity.
    pub gold: BTreeMap<String, u32>,
    /// Human- or structure-identified confusable units. Not relevant; used to report how
    /// often a retriever prefers a near miss to the answer.
    #[serde(default)]
    pub hard_negatives: Vec<String>,
    #[serde(default)]
    pub note: String,
}

impl Query {
    /// Ideal DCG for this query's grades, for nDCG's denominator.
    pub fn ideal_dcg(&self, k: usize) -> f64 {
        let mut grades: Vec<u32> = self.gold.values().copied().collect();
        grades.sort_unstable_by(|a, b| b.cmp(a));
        grades
            .iter()
            .take(k)
            .enumerate()
            .map(|(i, &g)| gain(g) / discount(i))
            .sum()
    }
}

#[inline]
pub fn gain(grade: u32) -> f64 {
    // 2^g - 1: the standard exponential gain, so a grade-2 passage is worth three times a
    // grade-1 one rather than twice.
    ((1u32 << grade) - 1) as f64
}

#[inline]
pub fn discount(rank: usize) -> f64 {
    ((rank + 2) as f64).log2()
}

pub struct Suite {
    pub queries: Vec<Query>,
}

impl Suite {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|source| Error::Io { path: path.into(), source })?;
        let mut queries = Vec::new();
        for (line_no, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            queries.push(serde_json::from_str(line).map_err(|source| Error::Parse {
                line: line_no + 1,
                source,
            })?);
        }
        Ok(Self { queries })
    }

    pub fn families(&self) -> Vec<String> {
        let mut names: Vec<String> =
            self.queries.iter().map(|q| q.family.clone()).collect();
        names.sort();
        names.dedup();
        names
    }
}
