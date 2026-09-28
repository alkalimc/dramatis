//! How sure retrieval is that the corpus holds an answer at all.
//!
//! Two signals on **unweighted** scores: the best score, and the normalised entropy of the
//! top-k score distribution. Weighting by the asking person's knowledge reorders hits but
//! must not change this answer, or "the corpus has nothing" and "he would not know" would
//! collapse into one low number and the caller could no longer tell a hand-off from a
//! fallback.

use crate::params::ConfidenceBands;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Confidence {
    pub level: Level,
    /// Best unweighted score, 0 when nothing matched.
    pub top1: f64,
    /// Shannon entropy of the top-k unweighted scores, normalised by `ln(k)` into [0, 1].
    /// 0 for fewer than two scores.
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
    if top1 <= 0.0 || top1 < bands.low_top1 {
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

fn entropy(scores: &[f64]) -> f64 {
    let total: f64 = scores.iter().sum();
    if scores.len() < 2 || total <= 0.0 {
        return 0.0;
    }
    let raw: f64 = scores
        .iter()
        .map(|s| s / total)
        .map(|p| -p * p.ln())
        .sum();
    raw / (scores.len() as f64).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands() -> ConfidenceBands {
        ConfidenceBands {
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
    fn only_the_top_k_are_read() {
        let mut scores = vec![9.0, 1.0];
        scores.extend([8.9; 40]);
        let wide = Confidence::of(&scores, 40, &bands());
        let narrow = Confidence::of(&scores, 1, &bands());
        assert_eq!(narrow.entropy, 0.0);
        assert!(wide.entropy > narrow.entropy);
    }

    #[test]
    fn a_weak_top_score_is_low() {
        assert_eq!(level_of_score(1.5, &bands()), Level::Low);
        assert_eq!(level_of_score(4.0, &bands()), Level::Medium);
        assert_eq!(level_of_score(6.0, &bands()), Level::High);
    }
}
