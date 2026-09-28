//! Reading the benchmark.
//!
//! The suite ships as JSONL because it is meant to be published and read by other
//! people's tooling, not just ours.

use std::collections::{BTreeMap, HashSet};
use std::io::BufRead as _;
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
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.into(),
            source,
        })?;
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
        let mut names: Vec<String> = self.queries.iter().map(|q| q.family.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    /// A deterministic subsample of about `target` queries, stratified by family; 0 or a
    /// target at least the suite's size keeps everything.
    ///
    /// Uniform sampling would be wrong: a small family can hold a handful of queries out
    /// of tens of thousands, so a small uniform sample would usually drop it. Every family
    /// keeps at least one query, and within a family every n-th is taken.
    pub fn stratified(&self, target: usize) -> Suite {
        let picked = stratify(&self.queries, |q| q.family.as_str(), target);
        Suite {
            queries: picked
                .into_iter()
                .map(|i| self.queries[i].clone())
                .collect(),
        }
    }

    /// At most `per_family` queries from each family, every n-th within it: for fitting
    /// where families are weighted equally, so a small family is not a handful of points.
    pub fn per_family(&self, per_family: usize) -> Suite {
        let mut by_family: BTreeMap<&str, Vec<&Query>> = BTreeMap::new();
        for q in &self.queries {
            by_family.entry(q.family.as_str()).or_default().push(q);
        }
        let mut queries = Vec::new();
        for members in by_family.into_values() {
            let want = per_family.min(members.len());
            let step = members.len() as f64 / want.max(1) as f64;
            queries.extend((0..want).map(|i| members[(i as f64 * step) as usize].clone()));
        }
        Suite { queries }
    }

    /// This suite minus the queries of `other`, by qid.
    pub fn without(&self, other: &Suite) -> Suite {
        let taken: HashSet<&str> = other.queries.iter().map(|q| q.qid.as_str()).collect();
        Suite {
            queries: self
                .queries
                .iter()
                .filter(|q| !taken.contains(q.qid.as_str()))
                .cloned()
                .collect(),
        }
    }
}

/// Indices of a stratified subsample of `items`, grouped by `family`, in family order.
fn stratify<T>(items: &[T], family: impl Fn(&T) -> &str, target: usize) -> Vec<usize> {
    if target == 0 || target >= items.len() {
        return (0..items.len()).collect();
    }
    let mut by_family: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        by_family.entry(family(item)).or_default().push(i);
    }
    let mut kept = Vec::with_capacity(target);
    for members in by_family.into_values() {
        let want = ((members.len() as f64 / items.len() as f64) * target as f64).round() as usize;
        let want = want.clamp(1, members.len());
        let step = members.len() as f64 / want as f64;
        kept.extend(
            (0..want).map(|i| members[((i as f64 * step) as usize).min(members.len() - 1)]),
        );
    }
    kept
}

/// Only the text of a stratified subsample, read line by line: for callers that measure
/// memory and must not hold the whole suite while doing it.
pub fn stratified_texts(path: impl AsRef<Path>, target: usize) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Line {
        family: String,
        text: String,
    }
    let path = path.as_ref();
    let io = |source| Error::Io {
        path: path.into(),
        source,
    };
    let reader = std::io::BufReader::new(std::fs::File::open(path).map_err(io)?);
    let mut lines = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(io)?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed: Line = serde_json::from_str(&line).map_err(|source| Error::Parse {
            line: line_no + 1,
            source,
        })?;
        lines.push(parsed);
    }
    let picked = stratify(&lines, |l| l.family.as_str(), target);
    Ok(picked.into_iter().map(|i| lines[i].text.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stratifying_keeps_every_family() {
        let families: Vec<&str> = std::iter::repeat_n("big", 1000).chain(["small"]).collect();
        let picked = stratify(&families, |f| f, 10);
        assert!(picked.iter().any(|&i| families[i] == "small"));
        assert_eq!(picked.iter().filter(|&&i| families[i] == "big").count(), 10);
        assert_eq!(stratify(&families, |f| f, 0).len(), families.len());
    }

    #[test]
    fn per_family_caps_each_family() {
        let query = |qid: usize, family: &str| Query {
            qid: qid.to_string(),
            family: family.into(),
            text: String::new(),
            stratum: String::new(),
            gold: BTreeMap::new(),
            hard_negatives: Vec::new(),
            note: String::new(),
        };
        let mut queries: Vec<Query> = (0..100).map(|i| query(i, "big")).collect();
        queries.push(query(100, "small"));
        let suite = Suite { queries };
        let picked = suite.per_family(10);
        assert_eq!(picked.queries.len(), 11);
        assert_eq!(suite.without(&picked).queries.len(), 90);
    }
}
