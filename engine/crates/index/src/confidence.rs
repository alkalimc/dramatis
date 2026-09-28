//! How sure retrieval is that the corpus holds an answer at all.
//!
//! Two signals on **unweighted** scores: the best score, and the normalised entropy of the
//! top-k score distribution (softmax over BM25, see [`entropy`]). Weighting by the asking person's knowledge reorders hits but
//! must not change this answer, or "the corpus has nothing" and "he would not know" would
//! collapse into one low number and the caller could no longer tell a hand-off from a
//! fallback.

use serde::{Deserialize, Serialize};

use crate::params::ConfidenceBands;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    pub level: Level,
    /// Best unweighted score, 0 when nothing matched.
    pub top1: f64,
    /// Entropy of the softmax over the top-k unweighted scores, normalised by `ln(k)` into
    /// [0, 1]: 1 when they are level, near 0 when one stands clear. 0 for fewer than two.
    pub entropy: f64,
}

impl Confidence {
    /// `scores` in any order; only the best `top_k` are read.
    pub fn of(scores: &[f64], top_k: usize, bands: &ConfidenceBands) -> Self {
        let mut sorted: Vec<f64> = scores.iter().copied().filter(|s| *s > 0.0).collect();
        sorted.sort_unstable_by(|a, b| b.total_cmp(a));
        sorted.truncate(top_k.max(1));
        let top1 = sorted.first().copied().unwrap_or(0.0);
        let entropy = entropy(&sorted);
        Self {
            level: level(top1, entropy, bands),
            top1,
            entropy,
        }
    }
}

pub fn level(top1: f64, entropy: f64, bands: &ConfidenceBands) -> Level {
    if top1 <= 0.0 || top1 < bands.low_top1 || entropy > bands.low_entropy {
        Level::Low
    } else if top1 >= bands.high_top1 && entropy <= bands.high_entropy {
        Level::High
    } else {
        Level::Medium
    }
}

/// The band one score falls in on its own: the per-person reading used when deciding who
/// is addressed or may interject, where there is no distribution to speak of.
pub fn level_of_score(score: f64, bands: &ConfidenceBands) -> Level {
    level(score, 0.0, bands)
}

/// Entropy of the softmax over `scores`, normalised by `ln(n)`.
///
/// BM25 is a sum of per-term log-odds weights, so a score *difference* is what carries
/// meaning; read as proportions, a top-k window of BM25 scores is nearly always flat
/// because its scores sit within a few percent of each other. The softmax turns a gap of
/// one BM25 point into odds of e : 1.
fn entropy(scores: &[f64]) -> f64 {
    if scores.len() < 2 {
        return 0.0;
    }
    let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = scores.iter().map(|s| (s - max).exp()).collect();
    let total: f64 = weights.iter().sum();
    let raw: f64 = weights
        .iter()
        .map(|w| w / total)
        .filter(|p| *p > 0.0)
        .map(|p| -p * p.ln())
        .sum();
    raw / (scores.len() as f64).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands() -> ConfidenceBands {
        ConfidenceBands {
            low_entropy: 1.0,
            low_top1: 2.0,
            high_top1: 6.0,
            high_entropy: 0.9,
        }
    }

    #[test]
    fn nothing_matched_is_low() {
        let c = Confidence::of(&[], 6, &bands());
        assert_eq!((c.level, c.top1, c.entropy), (Level::Low, 0.0, 0.0));
    }

    #[test]
    fn a_clear_winner_is_high() {
        let c = Confidence::of(&[9.0, 1.0, 0.5], 6, &bands());
        assert_eq!(c.level, Level::High);
        assert!(c.entropy < 0.9);
    }

    #[test]
    fn a_flat_distribution_is_not_high_even_with_a_strong_top_score() {
        let c = Confidence::of(&[7.0; 6], 6, &bands());
        assert!((c.entropy - 1.0).abs() < 1e-9);
        assert_eq!(c.level, Level::Medium);
    }

    #[test]
    fn a_one_point_gap_is_already_a_peak() {
        // Proportions would call 17 vs 16 flat; as log-odds it is e : 1.
        let near = Confidence::of(&[17.0, 16.0], 2, &bands());
        let level = Confidence::of(&[17.0, 17.0], 2, &bands());
        assert!(near.entropy < 0.9 && (level.entropy - 1.0).abs() < 1e-9);
    }

    #[test]
    fn only_the_top_k_are_read() {
        let mut scores = vec![9.0, 1.0];
        scores.extend([8.9; 40]);
        let wide = Confidence::of(&scores, 40, &bands());
        let narrow = Confidence::of(&scores, 1, &bands());
        assert_eq!(narrow.entropy, 0.0);
        assert!(wide.entropy > narrow.entropy);
    }

    #[test]
    fn a_flat_window_can_be_low_on_its_own() {
        let strict = ConfidenceBands {
            low_entropy: 0.5,
            ..bands()
        };
        assert_eq!(level(9.0, 0.95, &strict), Level::Low);
        assert_eq!(level(9.0, 0.95, &bands()), Level::Medium);
        // A single score has no window, so only the score bars apply.
        assert_eq!(level_of_score(9.0, &strict), Level::High);
    }

    #[test]
    fn a_weak_top_score_is_low() {
        assert_eq!(level_of_score(1.5, &bands()), Level::Low);
        assert_eq!(level_of_score(4.0, &bands()), Level::Medium);
        assert_eq!(level_of_score(6.0, &bands()), Level::High);
    }
}
