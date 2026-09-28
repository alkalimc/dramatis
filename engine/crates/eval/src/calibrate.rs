//! Choosing `retrieve.confidence` from the structural suite.
//!
//! Every suite query has an answer in the corpus, so what can be measured is whether the
//! retriever's best hit *is* one: a query whose top hit is relevant was answered, one whose
//! top hit is not was, as far as the caller can tell, not. The two confidence signals are
//! calibrated against that label:
//!
//! * `low_top1` is the best-score cut that best separates answered from unanswered
//!   (maximum Youden's J): below it, the top hit is more likely wrong than right.
//! * `high_top1` and `high_entropy` are the pair, among those whose selected queries are
//!   answered at least `high_precision` of the time, that selects the most queries.
//!
//! Families are weighted equally, as everywhere in this harness: one family can outnumber
//! the rest many times over, and a cut tuned to it would describe that family alone.

use std::collections::HashMap;

use index::params::ConfidenceBands;
use index::{Index, SearchRequest};

use crate::error::Result;
use crate::runner::{Config, configure};
use crate::suite::Suite;

/// One query's signals and whether its top hit was relevant.
#[derive(Debug, Clone)]
pub struct Sample {
    pub family: String,
    pub top1: f64,
    pub entropy: f64,
    pub answered: bool,
}

#[derive(Debug, Clone)]
pub struct Calibration {
    pub bands: ConfidenceBands,
    /// Youden's J at `low_top1`, family-weighted.
    pub low_separation: f64,
    /// Family-weighted share of queries at `High`, and how often those were answered.
    pub high_coverage: f64,
    pub high_precision: f64,
    /// Family-weighted share of queries at `Low`, and how often those were answered.
    pub low_share: f64,
    pub low_answered: f64,
    pub samples: usize,
}

/// Run the suite through `index` as `config` describes and collect one sample per query.
pub fn collect(index: &mut Index, suite: &Suite, config: &Config) -> Result<Vec<Sample>> {
    configure(index, config);
    let mut out = Vec::with_capacity(suite.queries.len());
    for query in &suite.queries {
        if query.gold.is_empty() {
            continue;
        }
        let response = index.search(&SearchRequest::new(query.text.as_str()))?;
        let answered = response
            .hits
            .first()
            .is_some_and(|h| query.gold.contains_key(h.item.id()));
        out.push(Sample {
            family: query.family.clone(),
            top1: response.confidence.top1,
            entropy: response.confidence.entropy,
            answered,
        });
    }
    Ok(out)
}

/// Pick the bands. `None` when there is nothing to calibrate on.
pub fn fit(samples: &[Sample], high_precision: f64) -> Option<Calibration> {
    if samples.is_empty() {
        return None;
    }
    let mut per_family: HashMap<&str, usize> = HashMap::new();
    for s in samples {
        *per_family.entry(&s.family).or_default() += 1;
    }
    let families = per_family.len() as f64;
    let weight: Vec<f64> = samples
        .iter()
        .map(|s| 1.0 / (per_family[s.family.as_str()] as f64 * families))
        .collect();

    let positive: f64 = samples
        .iter()
        .zip(&weight)
        .filter(|(s, _)| s.answered)
        .map(|(_, w)| w)
        .sum();
    let negative: f64 = 1.0 - positive;

    let mut cuts: Vec<f64> = samples.iter().map(|s| s.top1).filter(|t| *t > 0.0).collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    if cuts.is_empty() {
        return None;
    }
    // Thin the candidate cuts to quantiles: a few hundred are plenty and keep the pair
    // search below quadratic in the sample count.
    let thin = |values: &[f64], n: usize| -> Vec<f64> {
        if values.len() <= n {
            return values.to_vec();
        }
        (0..n)
            .map(|i| values[i * (values.len() - 1) / (n - 1)])
            .collect()
    };
    let top_cuts = thin(&cuts, 400);

    // Low: maximise TPR - FPR for "answered iff top1 >= cut".
    let mut low = (top_cuts[0], f64::NEG_INFINITY);
    for &cut in &top_cuts {
        let (mut tp, mut fp) = (0.0, 0.0);
        for (s, w) in samples.iter().zip(&weight) {
            if s.top1 >= cut {
                if s.answered {
                    tp += w;
                } else {
                    fp += w;
                }
            }
        }
        let tpr = if positive > 0.0 { tp / positive } else { 0.0 };
        let fpr = if negative > 0.0 { fp / negative } else { 0.0 };
        if tpr - fpr > low.1 {
            low = (cut, tpr - fpr);
        }
    }

    // High: most coverage subject to precision; fall back to the most precise pair.
    let mut entropies: Vec<f64> = samples.iter().map(|s| s.entropy).collect();
    entropies.sort_by(f64::total_cmp);
    entropies.dedup();
    let mut entropy_cuts = thin(&entropies, 60);
    entropy_cuts.push(1.0);
    let mut best: Option<(f64, f64, f64, f64)> = None; // (top1, entropy, coverage, precision)
    let mut fallback: Option<(f64, f64, f64, f64)> = None;
    for &cut in top_cuts.iter().filter(|c| **c >= low.0) {
        for &e in &entropy_cuts {
            let (mut selected, mut right) = (0.0, 0.0);
            for (s, w) in samples.iter().zip(&weight) {
                if s.top1 >= cut && s.entropy <= e {
                    selected += w;
                    if s.answered {
                        right += w;
                    }
                }
            }
            if selected <= 0.0 {
                continue;
            }
            let precision = right / selected;
            let candidate = (cut, e, selected, precision);
            if precision >= high_precision && best.is_none_or(|b| selected > b.2) {
                best = Some(candidate);
            }
            if fallback.is_none_or(|f| precision > f.3 || (precision == f.3 && selected > f.2)) {
                fallback = Some(candidate);
            }
        }
    }
    let (high_top1, high_entropy, high_coverage, precision) = best.or(fallback)?;

    let (mut low_share, mut low_right) = (0.0, 0.0);
    for (s, w) in samples.iter().zip(&weight) {
        if s.top1 < low.0 {
            low_share += w;
            if s.answered {
                low_right += w;
            }
        }
    }
    Some(Calibration {
        bands: ConfidenceBands {
            low_top1: low.0,
            high_top1,
            high_entropy,
        },
        low_separation: low.1,
        high_coverage,
        high_precision: precision,
        low_share,
        low_answered: if low_share > 0.0 {
            low_right / low_share
        } else {
            0.0
        },
        samples: samples.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(family: &str, top1: f64, entropy: f64, answered: bool) -> Sample {
        Sample {
            family: family.into(),
            top1,
            entropy,
            answered,
        }
    }

    #[test]
    fn the_low_cut_separates_wrong_from_right() {
        let mut samples = Vec::new();
        for i in 0..50 {
            samples.push(sample("f", 1.0 + i as f64 * 0.01, 0.9, false));
            samples.push(sample("f", 5.0 + i as f64 * 0.01, 0.5, true));
        }
        let fit = fit(&samples, 0.9).unwrap();
        assert!(fit.bands.low_top1 > 1.49 && fit.bands.low_top1 <= 5.0, "{fit:?}");
        assert!((fit.low_separation - 1.0).abs() < 1e-9);
        assert!(fit.high_precision >= 0.9);
        assert!(fit.bands.high_top1 >= fit.bands.low_top1);
    }

    #[test]
    fn high_requires_a_peaked_distribution_when_that_is_what_separates() {
        // Same scores; only entropy tells answered from not.
        let mut samples = Vec::new();
        for i in 0..40 {
            let top1 = 6.0 + i as f64 * 0.05;
            samples.push(sample("f", top1, 0.3, true));
            samples.push(sample("f", top1, 0.95, false));
        }
        let fit = fit(&samples, 0.9).unwrap();
        assert!(fit.bands.high_entropy < 0.95, "{fit:?}");
        assert!(fit.high_precision >= 0.9);
    }

    #[test]
    fn families_are_weighted_equally() {
        // A huge family that is always answered cannot hide a small one that never is.
        let mut samples: Vec<Sample> = (0..1000)
            .map(|i| sample("big", 8.0 + (i % 10) as f64 * 0.1, 0.5, true))
            .collect();
        samples.extend((0..10).map(|i| sample("small", 8.0 + i as f64 * 0.1, 0.5, false)));
        let fit = fit(&samples, 0.9).unwrap();
        assert!(fit.high_precision < 0.9, "the small family is half the weight");
    }

    #[test]
    fn nothing_to_fit_is_none() {
        assert!(fit(&[], 0.9).is_none());
        assert!(fit(&[sample("f", 0.0, 0.0, false)], 0.9).is_none());
    }
}
