//! Estimated values this crate reads (`roll`, `interject`, `reasoning`, `loop`). `Default`
//! carries the register's current values and is the only place they are written; use
//! sites take the struct as an argument. Missing keys fall back field by field.

use serde::{Deserialize, Serialize};

/// `roll.*`: when a segment is closed and a new one opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roll {
    /// Material tokens a segment may accumulate, times the cached-input price ratio:
    /// the threshold is `k / c`, so cheaper caching rolls less often.
    pub k: f64,
    /// Messages carried into the next segment's opening block after the summary.
    pub tail: u32,
    /// Share of the endpoint's context window at which a segment rolls regardless.
    pub window_margin: f64,
    /// Bytes of UTF-8 per token when material is estimated without a tokenizer.
    pub bytes_per_token: f64,
}

impl Default for Roll {
    fn default() -> Self {
        Self {
            k: 600.0,
            tail: 4,
            window_margin: 0.9,
            bytes_per_token: 3.0,
        }
    }
}

impl Roll {
    /// `roll_threshold` in material tokens.
    pub fn threshold(&self, c: f64) -> f64 {
        if c > 0.0 { self.k / c } else { f64::INFINITY }
    }

    pub fn tokens(&self, bytes: usize) -> u64 {
        (bytes as f64 / self.bytes_per_token.max(0.1)).ceil() as u64
    }
}

/// `interject.*`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interject {
    /// Own-unit score above which an enabled member may add one line. `None` = the
    /// high boundary of `retrieve.confidence`.
    pub min_confidence: Option<f64>,
}

/// `reasoning.*`: one value per session, never switched per call. `None` sends nothing
/// and lets the endpoint default. A value on the chat role in the endpoints file wins
/// for person sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reasoning {
    pub chat: Option<String>,
    pub host: Option<String>,
}

/// `loop.*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Loop {
    /// Model calls one ordinary turn may take (tool rounds included). A request's own
    /// turn budget bounds it instead.
    pub max_steps: u32,
}

impl Default for Loop {
    fn default() -> Self {
        Self { max_steps: 4 }
    }
}

/// Every group this crate reads, as `api` composes them into its one `Params`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub roll: Roll,
    pub interject: Interject,
    pub reasoning: Reasoning,
    #[serde(rename = "loop")]
    pub turn_loop: Loop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_keep_defaults() {
        let p: Params =
            serde_json::from_str(r#"{"roll": {"k": 100}, "loop": {"max_steps": 2}}"#).unwrap();
        assert_eq!(p.roll.k, 100.0);
        assert_eq!(p.roll.tail, Roll::default().tail);
        assert_eq!(p.turn_loop.max_steps, 2);
        assert_eq!(p.interject.min_confidence, None);
        assert_eq!(Roll::default().threshold(0.25), 2400.0);
        assert_eq!(Roll::default().threshold(0.0), f64::INFINITY);
    }
}
