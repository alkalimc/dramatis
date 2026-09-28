//! Choosing `retrieve.confidence` from the structural suite.
//!
//! Every suite query has an answer in the corpus, so what can be measured is whether the
//! retriever's best hit *is* one: a query whose top hit is relevant was answered, one whose
//! top hit is not was, as far as the caller can tell, not. The two confidence signals are
//! calibrated against that label:
//!
//! * `low_top1` and `low_entropy` bound the region (best score at least this, window at
//!   most this flat) that best separates answered from unanswered, by maximum Youden's J.
//!   Outside it is `Low`. Both signals are needed: on BM25 a strong best score says little
//!   when the next few units score the same, and a peaked window says little when every
//!   score in it is weak.
//! * `high_top1` and `high_entropy` bound the widest region inside that one whose queries
//!   are answered at least `high_precision` of the time.
//!
//! Families are weighted equally, as everywhere in this harness: one family can outnumber
//! the rest many times over, and a cut tuned to it would describe that family alone.

use std::collections::HashMap;

use index::confidence::level;
use index::params::ConfidenceBands;
use index::{Index, Level, SearchRequest};

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
    /// The bands applied to the samples that chose them.
    pub fit: Assessment,
    pub samples: usize,
}

/// Family-weighted share of queries at each level, and the share of those whose top hit
/// was relevant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assessment {
    pub samples: usize,
    pub high_share: f64,
    pub high_answered: f64,
    pub medium_share: f64,
    pub medium_answered: f64,
    pub low_share: f64,
    pub low_answered: f64,
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
    let weight = family_weights(samples);
    let positive: f64 = samples
        .iter()
        .zip(&weight)
        .filter(|(s, _)| s.answered)
        .map(|(_, w)| w)
        .sum();
    let negative: f64 = weight.iter().sum::<f64>() - positive;

    let mut tops: Vec<f64> = samples
        .iter()
        .map(|s| s.top1)
        .filter(|t| *t > 0.0)
        .collect();
    if tops.is_empty() {
        return None;
    }
    let mut entropies: Vec<f64> = samples.iter().map(|s| s.entropy).collect();
    // Candidate cuts at quantiles: a couple of hundred per axis are plenty and keep the
    // grid search small whatever the sample count.
    let top_cuts = quantiles(&mut tops, 200);
    let mut entropy_cuts = quantiles(&mut entropies, 60);
    entropy_cuts.push(1.0);

    // Weighted mass inside "top1 >= t and entropy <= e", and the answered part of it.
    let inside = |t: f64, e: f64| {
        let (mut all, mut right) = (0.0, 0.0);
        for (s, w) in samples.iter().zip(&weight) {
            if s.top1 >= t && s.entropy <= e {
                all += w;
                if s.answered {
                    right += w;
                }
            }
        }
        (all, right)
    };

    // Low: the complement of the region that best separates answered from not
    // (maximum Youden's J, TPR - FPR).
    let mut low = (top_cuts[0], 1.0, f64::NEG_INFINITY);
    for &t in &top_cuts {
        for &e in &entropy_cuts {
            let (all, right) = inside(t, e);
            let tpr = if positive > 0.0 {
                right / positive
            } else {
                0.0
            };
            let fpr = if negative > 0.0 {
                (all - right) / negative
            } else {
                0.0
            };
            if tpr - fpr > low.2 {
                low = (t, e, tpr - fpr);
            }
        }
    }

    // High: inside the not-low region, the widest one answered at least `high_precision`
    // of the time; failing that, the most precise one.
    let mut best: Option<(f64, f64, f64)> = None; // (top1, entropy, coverage)
    let mut fallback: Option<(f64, f64, f64, f64)> = None; // .. and precision
    for &t in top_cuts.iter().filter(|t| **t >= low.0) {
        for &e in entropy_cuts.iter().filter(|e| **e <= low.1) {
            let (all, right) = inside(t, e);
            if all <= 0.0 {
                continue;
            }
            let precision = right / all;
            if precision >= high_precision && best.is_none_or(|b| all > b.2) {
                best = Some((t, e, all));
            }
            if fallback.is_none_or(|f| precision > f.3 || (precision == f.3 && all > f.2)) {
                fallback = Some((t, e, all, precision));
            }
        }
    }
    let (high_top1, high_entropy) = match (best, fallback) {
        (Some((t, e, _)), _) | (None, Some((t, e, _, _))) => (t, e),
        (None, None) => (low.0, low.1),
    };

    let bands = ConfidenceBands {
        low_top1: low.0,
        low_entropy: low.1,
        high_top1,
        high_entropy,
    };
    Some(Calibration {
        fit: assess(samples, &bands),
        bands,
        low_separation: low.2,
        samples: samples.len(),
    })
}

/// Each sample's weight when every family counts equally; the weights sum to 1.
fn family_weights(samples: &[Sample]) -> Vec<f64> {
    let mut per_family: HashMap<&str, usize> = HashMap::new();
    for s in samples {
        *per_family.entry(&s.family).or_default() += 1;
    }
    let families = per_family.len() as f64;
    samples
        .iter()
        .map(|s| 1.0 / (per_family[s.family.as_str()] as f64 * families))
        .collect()
}

/// About `n` values spread evenly over the sorted distinct values.
fn quantiles(values: &mut Vec<f64>, n: usize) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values.dedup();
    if values.len() <= n {
        return values.clone();
    }
    (0..n)
        .map(|i| values[i * (values.len() - 1) / (n - 1)])
        .collect()
}

/// How a set of bands sorts `samples`: family-weighted share at each level and how often
/// the top hit was relevant there. Used on the fitting subset and again on a wider run,
/// so the bands are never reported only on the data that chose them.
pub fn assess(samples: &[Sample], bands: &ConfidenceBands) -> Assessment {
    let mut share = [0.0; 3];
    let mut right = [0.0; 3];
    for (s, &w) in samples.iter().zip(&family_weights(samples)) {
        let at = match level(s.top1, s.entropy, bands) {
            Level::High => 0,
            Level::Medium => 1,
            Level::Low => 2,
        };
        share[at] += w;
        if s.answered {
            right[at] += w;
        }
    }
    let rate = |i: usize| {
        if share[i] > 0.0 {
            right[i] / share[i]
        } else {
            0.0
        }
    };
    Assessment {
        samples: samples.len(),
        high_share: share[0],
        high_answered: rate(0),
        medium_share: share[1],
        medium_answered: rate(1),
        low_share: share[2],
        low_answered: rate(2),
    }
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
            // Same entropy: only the best score tells the two apart.
            samples.push(sample("f", 1.0 + i as f64 * 0.01, 0.5, false));
            samples.push(sample("f", 5.0 + i as f64 * 0.01, 0.5, true));
        }
        let fit = fit(&samples, 0.9).unwrap();
        assert!(
            fit.bands.low_top1 > 1.49 && fit.bands.low_top1 <= 5.0,
            "{fit:?}"
        );
        assert!((fit.low_separation - 1.0).abs() < 1e-9);
        assert!(fit.fit.high_answered >= 0.9);
        assert!(fit.bands.high_top1 >= fit.bands.low_top1);
        assert!(fit.fit.low_answered < 0.5, "{fit:?}");
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
        assert!(fit.fit.high_answered >= 0.9);
    }

    #[test]
    fn families_are_weighted_equally() {
        // A huge family that is always answered cannot hide a small one that never is.
        let mut samples: Vec<Sample> = (0..1000)
            .map(|i| sample("big", 8.0 + (i % 10) as f64 * 0.1, 0.5, true))
            .collect();
        samples.extend((0..10).map(|i| sample("small", 8.0 + i as f64 * 0.1, 0.5, false)));
        let fit = fit(&samples, 0.9).unwrap();
        assert!(
            fit.fit.high_answered < 0.9,
            "the small family is half the weight"
        );
    }

    #[test]
    fn a_flat_window_is_low_when_that_is_what_separates() {
        // Strong scores throughout; only a flat window marks the wrong answers.
        let mut samples = Vec::new();
        for i in 0..40 {
            let top1 = 20.0 + i as f64 * 0.1;
            samples.push(sample("f", top1, 0.1, true));
            samples.push(sample("f", top1, 0.8, false));
        }
        let fit = fit(&samples, 0.9).unwrap();
        assert!(fit.bands.low_entropy < 0.8, "{fit:?}");
        assert!((fit.low_separation - 1.0).abs() < 1e-9);
        assert_eq!(fit.fit.low_answered, 0.0);
    }

    #[test]
    fn shares_cover_every_sample_once() {
        let samples = [
            sample("a", 9.0, 0.1, true),
            sample("a", 1.0, 0.9, false),
            sample("b", 4.0, 0.9, true),
        ];
        let bands = ConfidenceBands {
            low_top1: 2.0,
            low_entropy: 1.0,
            high_top1: 6.0,
            high_entropy: 0.5,
        };
        let a = assess(&samples, &bands);
        assert!((a.high_share + a.medium_share + a.low_share - 1.0).abs() < 1e-9);
        assert_eq!(
            (a.high_share, a.medium_share, a.low_share),
            (0.25, 0.5, 0.25)
        );
        assert_eq!((a.high_answered, a.low_answered), (1.0, 0.0));
    }

    #[test]
    fn nothing_to_fit_is_none() {
        assert!(fit(&[], 0.9).is_none());
        assert!(fit(&[sample("f", 0.0, 0.0, false)], 0.9).is_none());
    }
}
