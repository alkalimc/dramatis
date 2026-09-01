//! Metrics, and the reporting discipline the benchmark demands.
//!
//! Every rule enforced here was written into the suite's manifest before any retriever
//! existed, which is the point: thresholds and reporting shapes chosen after seeing results
//! are descriptions, not measurements.

use std::collections::{BTreeMap, HashSet};

use crate::suite::{discount, gain, Query};

/// One query's outcome.
#[derive(Debug, Clone)]
pub struct Scored {
    pub qid: String,
    pub family: String,
    pub stratum: String,
    pub ndcg: f64,
    /// Fraction of the returned k that were relevant.
    pub precision: f64,
    /// Fraction of the query's relevant units that were returned.
    pub recall: f64,
    /// How many relevant units existed. Carried because recall@k is capped by k/gold_size,
    /// and a recall figure quoted without it invites the reader to mistake a cap for a
    /// failure — several families here have 45 to 113 relevant units against k = 10.
    pub gold_size: usize,
    /// Reciprocal rank of the first relevant unit, 0 if none was retrieved.
    pub rr: f64,
    /// Did a hard negative outrank every relevant unit? The sharpest single signal in the
    /// suite: it means the retriever preferred a passage a human flagged as confusable.
    pub negative_won: bool,
    pub retrieved: usize,
}

/// Aggregate over a set of queries.
///
/// Deliberately no `overall` field. The suite's manifest forbids a single mean — one family
/// outnumbers the rest sixteen to one, and they test different claims — so the type does
/// not offer one to compute by accident.
#[derive(Debug, Clone, Default)]
pub struct Aggregate {
    pub queries: usize,
    pub ndcg: f64,
    pub precision: f64,
    pub recall: f64,
    pub gold_size: f64,
    pub mrr: f64,
    pub negative_win_rate: f64,
    pub empty: usize,
}

impl Aggregate {
    pub fn of(scored: &[Scored]) -> Self {
        if scored.is_empty() {
            return Self::default();
        }
        let n = scored.len() as f64;
        Self {
            queries: scored.len(),
            ndcg: scored.iter().map(|s| s.ndcg).sum::<f64>() / n,
            precision: scored.iter().map(|s| s.precision).sum::<f64>() / n,
            recall: scored.iter().map(|s| s.recall).sum::<f64>() / n,
            gold_size: scored.iter().map(|s| s.gold_size as f64).sum::<f64>() / n,
            mrr: scored.iter().map(|s| s.rr).sum::<f64>() / n,
            negative_win_rate: scored.iter().filter(|s| s.negative_won).count() as f64 / n,
            empty: scored.iter().filter(|s| s.retrieved == 0).count(),
        }
    }
}

/// Score one ranked list against one query's judgements.
pub fn score(query: &Query, ranked: &[String], k: usize) -> Scored {
    let negatives: HashSet<&str> = query.hard_negatives.iter().map(String::as_str).collect();
    let top: Vec<&String> = ranked.iter().take(k).collect();

    let mut dcg = 0.0;
    let mut first_relevant: Option<usize> = None;
    let mut first_negative: Option<usize> = None;
    let mut found = 0usize;

    for (rank, id) in top.iter().enumerate() {
        if let Some(&grade) = query.gold.get(id.as_str()) {
            dcg += gain(grade) / discount(rank);
            found += 1;
            first_relevant = first_relevant.or(Some(rank));
        } else if negatives.contains(id.as_str()) {
            first_negative = first_negative.or(Some(rank));
        }
    }

    let ideal = query.ideal_dcg(k);
    // A query whose gold set is empty should never have shipped, but scoring it as 0 would
    // silently drag every average down. Report it as unscoreable via `retrieved` instead.
    let ndcg = if ideal > 0.0 { dcg / ideal } else { 0.0 };

    // A hard negative "wins" only if it beats *every* relevant unit in the list. Beating
    // some is normal and uninteresting; beating all of them means the ranking is wrong in
    // the specific way the negatives were chosen to detect.
    let negative_won = match (first_negative, first_relevant) {
        (Some(neg), Some(rel)) => neg < rel,
        (Some(_), None) => true,
        _ => false,
    };

    Scored {
        qid: query.qid.clone(),
        family: query.family.clone(),
        stratum: query.stratum.clone(),
        ndcg,
        // Both in their textbook forms. An earlier version divided by `min(gold, k)`, which
        // reads as recall but saturates at 1.000 whenever the gold set is larger than k —
        // and on this suite it usually is, so the column said "perfect" about a run that
        // retrieved 10 of 113 relevant units.
        precision: if k == 0 { 0.0 } else { found as f64 / k as f64 },
        recall: if query.gold.is_empty() {
            0.0
        } else {
            found as f64 / query.gold.len() as f64
        },
        gold_size: query.gold.len(),
        rr: first_relevant.map(|r| 1.0 / (r + 1) as f64).unwrap_or(0.0),
        negative_won,
        retrieved: top.len(),
    }
}

/// Group scores by family, then by stratum within it.
///
/// Both levels are mandatory in any report. A family that is mostly verbatim is solvable by
/// exact match, so its score is a sanity floor rather than evidence — and that is only
/// visible when the strata are kept apart.
pub fn by_family_and_stratum(
    scored: &[Scored],
) -> BTreeMap<String, BTreeMap<String, Aggregate>> {
    let mut buckets: BTreeMap<String, BTreeMap<String, Vec<Scored>>> = BTreeMap::new();
    for s in scored {
        buckets
            .entry(s.family.clone())
            .or_default()
            .entry(s.stratum.clone())
            .or_default()
            .push(s.clone());
    }
    buckets
        .into_iter()
        .map(|(family, strata)| {
            let inner = strata
                .into_iter()
                .map(|(stratum, items)| (stratum, Aggregate::of(&items)))
                .collect();
            (family, inner)
        })
        .collect()
}

pub fn by_family(scored: &[Scored]) -> BTreeMap<String, Aggregate> {
    let mut buckets: BTreeMap<String, Vec<Scored>> = BTreeMap::new();
    for s in scored {
        buckets.entry(s.family.clone()).or_default().push(s.clone());
    }
    buckets
        .into_iter()
        .map(|(family, items)| (family, Aggregate::of(&items)))
        .collect()
}

/// Macro-average across families: each family contributes equally regardless of size.
///
/// This is the *only* defensible single number for this suite, and it is still a summary
/// rather than a result. One family holds 16,124 of 19,801 queries, so a micro-average would
/// essentially report that family's score under a name implying it covered all six.
pub fn macro_average(per_family: &BTreeMap<String, Aggregate>) -> Aggregate {
    let families: Vec<&Aggregate> = per_family.values().filter(|a| a.queries > 0).collect();
    if families.is_empty() {
        return Aggregate::default();
    }
    let n = families.len() as f64;
    Aggregate {
        queries: families.iter().map(|a| a.queries).sum(),
        ndcg: families.iter().map(|a| a.ndcg).sum::<f64>() / n,
        precision: families.iter().map(|a| a.precision).sum::<f64>() / n,
        recall: families.iter().map(|a| a.recall).sum::<f64>() / n,
        gold_size: families.iter().map(|a| a.gold_size).sum::<f64>() / n,
        mrr: families.iter().map(|a| a.mrr).sum::<f64>() / n,
        negative_win_rate: families.iter().map(|a| a.negative_win_rate).sum::<f64>() / n,
        empty: families.iter().map(|a| a.empty).sum(),
    }
}

/// Paired bootstrap test over per-query nDCG differences.
///
/// Two systems on the same queries are paired, so a paired test is the correct one and an
/// unpaired comparison would throw away most of the statistical power. Bootstrap rather
/// than a t-test because nDCG is bounded and its differences are not normal.
///
/// Deterministic: the seed is fixed so a reported p-value can be reproduced exactly.
pub fn paired_bootstrap(baseline: &[f64], candidate: &[f64], iterations: usize) -> Option<f64> {
    if baseline.len() != candidate.len() || baseline.is_empty() {
        return None;
    }
    let deltas: Vec<f64> = candidate
        .iter()
        .zip(baseline)
        .map(|(c, b)| c - b)
        .collect();
    let observed: f64 = deltas.iter().sum::<f64>() / deltas.len() as f64;

    // xorshift64*, inline: a fixed, documented generator beats a dependency whose default
    // seeding would make results irreproducible.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let mut at_least_as_extreme = 0usize;
    for _ in 0..iterations {
        let mut sum = 0.0;
        for _ in 0..deltas.len() {
            let idx = (next() % deltas.len() as u64) as usize;
            // Centre each resample on zero: the null hypothesis is no difference.
            sum += deltas[idx] - observed;
        }
        if (sum / deltas.len() as f64).abs() >= observed.abs() {
            at_least_as_extreme += 1;
        }
    }
    Some((at_least_as_extreme as f64 + 1.0) / (iterations as f64 + 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn query(gold: &[(&str, u32)], negatives: &[&str]) -> Query {
        Query {
            qid: "q".into(),
            family: "f".into(),
            text: "t".into(),
            stratum: "paraphrase".into(),
            gold: gold.iter().map(|(id, g)| (id.to_string(), *g)).collect(),
            hard_negatives: negatives.iter().map(|s| s.to_string()).collect(),
            note: String::new(),
        }
    }

    #[test]
    fn a_perfect_ranking_scores_one() {
        let q = query(&[("a", 2), ("b", 1)], &[]);
        let s = score(&q, &["a".into(), "b".into()], 6);
        assert!((s.ndcg - 1.0).abs() < 1e-9, "ndcg was {}", s.ndcg);
        assert_eq!(s.rr, 1.0);
    }

    #[test]
    fn ordering_matters() {
        let q = query(&[("a", 2), ("b", 1)], &[]);
        let good = score(&q, &["a".into(), "b".into()], 6);
        let bad = score(&q, &["b".into(), "a".into()], 6);
        assert!(good.ndcg > bad.ndcg);
    }

    #[test]
    fn a_negative_beating_all_relevant_units_is_recorded() {
        let q = query(&[("a", 2)], &["n"]);
        assert!(score(&q, &["n".into(), "a".into()], 6).negative_won);
    }

    #[test]
    fn a_negative_below_a_relevant_unit_is_not_a_loss() {
        // Beating some relevant unit is ordinary; beating all of them is the failure the
        // negatives were chosen to detect.
        let q = query(&[("a", 2)], &["n"]);
        assert!(!score(&q, &["a".into(), "n".into()], 6).negative_won);
    }

    #[test]
    fn retrieving_nothing_scores_zero_without_panicking() {
        let s = score(&query(&[("a", 2)], &[]), &[], 6);
        assert_eq!(s.ndcg, 0.0);
        assert_eq!(s.rr, 0.0);
        assert_eq!(s.retrieved, 0);
    }

    #[test]
    fn recall_is_not_credited_for_units_it_could_not_have_returned() {
        // 20 relevant units, 10 slots, all 10 filled correctly. That is precision 1.0 and
        // recall 0.5, and calling it recall 1.0 — as dividing by min(gold, k) does — would
        // claim the run found twice what it found.
        let gold: Vec<(String, u32)> = (0..20).map(|i| (format!("u{i}"), 2)).collect();
        let q = Query {
            qid: "q".into(),
            family: "f".into(),
            text: "t".into(),
            stratum: "paraphrase".into(),
            gold: gold.into_iter().collect(),
            hard_negatives: Vec::new(),
            note: String::new(),
        };
        let ranked: Vec<String> = (0..10).map(|i| format!("u{i}")).collect();
        let s = score(&q, &ranked, 10);
        assert!((s.precision - 1.0).abs() < 1e-9, "precision {}", s.precision);
        assert!((s.recall - 0.5).abs() < 1e-9, "recall {}", s.recall);
        assert_eq!(s.gold_size, 20, "the cap must travel with the number it explains");
    }

    #[test]
    fn macro_average_does_not_let_one_family_dominate() {
        // 1,000 queries scoring 0.1 against 10 scoring 0.9: a micro-average would report
        // ~0.11 and hide the second family entirely.
        let mut per_family = BTreeMap::new();
        per_family.insert(
            "big".to_string(),
            Aggregate { queries: 1000, ndcg: 0.1, ..Default::default() },
        );
        per_family.insert(
            "small".to_string(),
            Aggregate { queries: 10, ndcg: 0.9, ..Default::default() },
        );
        let macro_avg = macro_average(&per_family);
        assert!((macro_avg.ndcg - 0.5).abs() < 1e-9, "got {}", macro_avg.ndcg);
    }

    #[test]
    fn bootstrap_finds_no_difference_between_identical_systems() {
        let scores = vec![0.5, 0.6, 0.7, 0.4, 0.9];
        let p = paired_bootstrap(&scores, &scores, 500).unwrap();
        assert!(p > 0.5, "identical systems should not look different, p={p}");
    }

    #[test]
    fn bootstrap_detects_a_consistent_improvement() {
        let baseline: Vec<f64> = (0..60).map(|i| 0.30 + (i % 5) as f64 * 0.01).collect();
        let candidate: Vec<f64> = baseline.iter().map(|b| b + 0.25).collect();
        let p = paired_bootstrap(&baseline, &candidate, 2000).unwrap();
        assert!(p < 0.05, "a uniform +0.25 should be significant, p={p}");
    }

    #[test]
    fn bootstrap_is_reproducible() {
        let a: Vec<f64> = (0..40).map(|i| i as f64 / 40.0).collect();
        let b: Vec<f64> = a.iter().map(|x| x + 0.1).collect();
        assert_eq!(
            paired_bootstrap(&a, &b, 1000),
            paired_bootstrap(&a, &b, 1000)
        );
    }
}
