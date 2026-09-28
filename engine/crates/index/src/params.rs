//! Estimated values this crate reads, one serde struct per register group (`retrieve`,
//! `find_people`). `Default` carries the register's current values and is the only place
//! they are written; use sites take the struct as an argument. Missing keys in a
//! `params.toml` fall back to these defaults field by field.

use serde::Deserialize;

/// `retrieve.*`
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Retrieve {
    /// Hits returned when a request does not say.
    pub top_k: usize,
    /// Lexical candidates taken before weighting, memory merge and truncation.
    pub candidates: usize,
    /// Reciprocal-rank-fusion constant. Read once a second path exists.
    pub rrf_k: f64,
    /// Candidates handed to a reranker. Read once a reranker exists.
    pub rerank_in: usize,
    pub confidence: ConfidenceBands,
    pub scope_weight: ScopeWeights,
    /// Neighbouring units fetched on each side of every returned hit.
    pub neighbours: i64,
}

impl Default for Retrieve {
    fn default() -> Self {
        Self {
            top_k: 6,
            candidates: 50,
            rrf_k: 60.0,
            rerank_in: 20,
            confidence: ConfidenceBands::default(),
            scope_weight: ScopeWeights::default(),
            neighbours: 1,
        }
    }
}

/// `retrieve.confidence`: boundaries on the two unweighted signals.
///
/// `Low` when the best score is under `low_top1` or the window is flatter than
/// `low_entropy`; `High` when the best score reaches `high_top1` and the window is at least
/// as peaked as `high_entropy`; `Medium` otherwise. `top1` is a raw BM25 score, so its
/// scale belongs to the corpus. These defaults are uncalibrated starting points; the
/// register's values come from `dramatis-cli calibrate` on the structural suite and are
/// supplied through `params.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ConfidenceBands {
    /// Below this best score the corpus is taken to hold nothing on the query.
    pub low_top1: f64,
    /// Above this normalised entropy the window is too flat to name an answer. 1 is
    /// perfectly flat, so 1 disables the entropy half of `Low`.
    pub low_entropy: f64,
    /// At or above this best score, with a peaked window, the answer is clear.
    pub high_top1: f64,
    /// At or below this normalised entropy the window counts as peaked.
    pub high_entropy: f64,
}

impl Default for ConfidenceBands {
    fn default() -> Self {
        Self {
            low_top1: 5.0,
            low_entropy: 1.0,
            high_top1: 10.0,
            high_entropy: 0.95,
        }
    }
}

/// `retrieve.scope_weight`: multipliers on a hit's score by where it sits relative to the
/// asking person's knowledge. Weighting reorders; it never removes a hit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ScopeWeights {
    pub own: f64,
    pub lived: f64,
    pub in_scope: f64,
    pub out_of_scope: f64,
    /// Caller-supplied memories that match the query.
    pub memory: f64,
}

impl Default for ScopeWeights {
    fn default() -> Self {
        Self {
            own: 1.5,
            lived: 1.3,
            in_scope: 1.2,
            out_of_scope: 1.0,
            memory: 1.5,
        }
    }
}

/// `find_people.*`
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct FindPeople {
    /// People returned.
    pub k: usize,
}

impl Default for FindPeople {
    fn default() -> Self {
        Self { k: 5 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_fall_back_field_by_field() {
        let parsed: Retrieve =
            serde_json::from_str(r#"{"top_k": 3, "confidence": {"high_top1": 7.5}}"#).unwrap();
        assert_eq!(parsed.top_k, 3);
        assert_eq!(parsed.candidates, Retrieve::default().candidates);
        assert_eq!(parsed.confidence.high_top1, 7.5);
        assert_eq!(
            parsed.confidence.low_top1,
            ConfidenceBands::default().low_top1
        );
    }
}
