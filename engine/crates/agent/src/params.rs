//! Estimated values this crate reads (`roll`, `interject`, `group`, `reasoning`, `loop`, `retrieve`, `context`). `Default`
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interject {
    /// Own-unit score above which an enabled member may add one line. `None` = the
    /// high boundary of `retrieve.confidence` times `relax`.
    pub min_confidence: Option<f64>,
    /// Share of the high boundary that counts when `min_confidence` is not set.
    pub relax: f64,
}

impl Default for Interject {
    fn default() -> Self {
        Self {
            min_confidence: None,
            relax: 0.8,
        }
    }
}

/// `group.*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    /// Most replies one user message gets in a group, the first one included, whether
    /// members were addressed together or handed the turn on. A hard bound per user
    /// message; each reply is one more request.
    pub collective_max: u32,
    /// Ask the model who a message speaks to when the cheap rules cannot tell (one small
    /// request). Off, only the corpus's collective address words widen the audience.
    pub classify: bool,
    /// Lines before the message that the audience question shows.
    pub classify_lines: u32,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            collective_max: 5,
            classify: true,
            classify_lines: 4,
        }
    }
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

/// `retrieve.*` beyond what `index` reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retrieve {
    /// Passages sent when retrieval is rated low, marked as possibly unrelated.
    pub low_hits: u32,
}

impl Default for Retrieve {
    fn default() -> Self {
        Self { low_hits: 3 }
    }
}

/// `opening.*`: what a segment's opening block carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Opening {
    /// Newest memories written into the opening block; older ones stay retrievable.
    pub memories: u32,
}

impl Default for Opening {
    fn default() -> Self {
        Self { memories: 8 }
    }
}

/// `cost.*` beyond what `world` reads (`cost.c`, `cost.o`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cost {
    /// Price of writing a cache entry / uncached input price, for a model whose price
    /// list has no cache-write price. Only the messages wire charges for writes.
    pub w: f64,
}

impl Default for Cost {
    fn default() -> Self {
        Self { w: 1.25 }
    }
}

/// `context.*`: the clock line on each turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Context {
    /// Whole hours since the user's previous message from which the line says so.
    pub gap_hours: u32,
}

impl Default for Context {
    fn default() -> Self {
        Self { gap_hours: 3 }
    }
}

/// `picture.*`: how an attached picture is shrunk before it is sent and stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Picture {
    /// Longest side, pixels; larger pictures are scaled down to it.
    pub max_side: u32,
    /// JPEG quality, 1 to 100, of a re-encoded picture.
    pub quality: u8,
}

impl Default for Picture {
    fn default() -> Self {
        Self {
            max_side: 1024,
            quality: 80,
        }
    }
}

/// Every group this crate reads, as `api` composes them into its one `Params`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub roll: Roll,
    pub interject: Interject,
    pub group: Group,
    pub reasoning: Reasoning,
    #[serde(rename = "loop")]
    pub turn_loop: Loop,
    pub opening: Opening,
    pub retrieve: Retrieve,
    pub cost: Cost,
    pub context: Context,
    pub picture: Picture,
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
        assert_eq!(p.group.collective_max, 5);
        assert!(p.group.classify);
        assert_eq!(p.cost.w, 1.25);
        assert_eq!(Roll::default().threshold(0.25), 2400.0);
        assert_eq!(Roll::default().threshold(0.0), f64::INFINITY);
    }
}
