//! What `message.attachment` holds for a message the engine wrote: the reply's
//! citations, how sure retrieval was, the request it answers, whether it retells
//! something, and the request's note. One type, so the side that writes it (the agent)
//! and the side that shows it (the history) cannot drift apart.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageMeta {
    /// The passages the reply cites, in order.
    pub cites: Vec<world::Citation>,
    /// How sure retrieval was about the material the reply had.
    pub confidence: Option<index::Level>,
    /// The request this message answers.
    pub task: Option<world::TaskId>,
    /// A retelling of something said elsewhere.
    pub relayed: bool,
    /// The request's note, relative to the office directory.
    pub note: Option<String>,
}

impl MessageMeta {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The meta a stored message carries; none (or unreadable) is the default.
    pub fn of(attachment: Option<&serde_json::Value>) -> Self {
        attachment
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_writer_stores() {
        let v = serde_json::json!({
            "cites": [{"page": "P", "revid": 3, "span_from": 1, "span_to": 2, "chunk_id": "dialogue:x"}],
            "confidence": "low",
            "task": 4,
            "note": "requests/4.md",
        });
        let m = MessageMeta::of(Some(&v));
        assert_eq!(m.cites[0].chunk_id, "dialogue:x");
        assert_eq!(m.confidence, Some(index::Level::Low));
        assert_eq!(m.task, Some(world::TaskId(4)));
        assert!(!m.relayed);
        assert!(MessageMeta::of(Some(&serde_json::json!("junk"))).is_empty());
        assert!(MessageMeta::of(None).is_empty());
    }
}
