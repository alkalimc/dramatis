//! A model call in protocol-neutral terms, and the log items it is made of.
//!
//! The session log stores these items, serialised, byte for byte; a request is the
//! segment's shape plus its prefix blocks and items in order. Whatever renders a
//! request for one protocol must be a pure function of it, so a log that is only
//! appended to yields bodies that each extend the previous one.

use api::endpoints::WireApi;
use serde::{Deserialize, Serialize};

/// One appended entry of a session log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum Item {
    /// The user's words or a harness turn, with an optional image.
    User {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<Image>,
    },
    /// The model's reply, verbatim.
    Assistant { parts: Vec<Part> },
    /// A tool's output for one call.
    ToolResult { call_id: String, output: String },
}

/// One piece of a reply, in the order the model produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum Part {
    Text {
        text: String,
    },
    ToolCall(ToolCall),
    /// A block only its own protocol understands (a reasoning block with its
    /// signature), kept exactly as received: `block` is its JSON text, which that
    /// protocol's renderer splices back unchanged and every other leaves out.
    Opaque {
        wire: WireApi,
        block: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as the model wrote them, a JSON string.
    pub arguments: String,
}

/// An image the user attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub mime: String,
    pub data_base64: String,
}

impl From<api::views::ImagePart> for Image {
    fn from(p: api::views::ImagePart) -> Self {
        Self {
            mime: p.mime,
            data_base64: p.data_base64,
        }
    }
}

impl Item {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("items serialise")
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// The text the harness or the user put in a log: what material and ids are
    /// counted from. Replies are the model's and are not material.
    pub fn harness_text(&self) -> Option<&str> {
        match self {
            Self::User { text, .. } => Some(text),
            Self::ToolResult { output, .. } => Some(output),
            Self::Assistant { .. } => None,
        }
    }

    /// Every text in the item, replies included.
    pub fn texts(&self) -> Vec<&str> {
        match self {
            Self::User { text, .. } => vec![text],
            Self::ToolResult { output, .. } => vec![output],
            Self::Assistant { parts } => parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text { text } => Some(text.as_str()),
                    Part::ToolCall(c) => Some(c.arguments.as_str()),
                    Part::Opaque { .. } => None,
                })
                .collect(),
        }
    }
}

/// The forced-tool choice of one call: the only thing that may differ between calls on
/// the same log, so a renderer writes it after every item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    Auto,
    /// No tool this time: the model must answer in text.
    None,
    Tool(&'a str),
}

/// What must stay byte-identical for a segment's lifetime.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub wire: WireApi,
    pub model: String,
    /// The session's one reasoning value (an effort level); `None` sends nothing.
    pub reasoning: Option<String>,
    pub tools: Vec<api::tools::ToolSpec>,
    /// What the user-name placeholder became in this segment's bytes.
    pub user: String,
}

/// A segment's `shape` column: the shape with the tool list as a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub wire: WireApi,
    pub model: String,
    pub reasoning: Option<String>,
    pub tools: String,
    pub user: String,
}

impl Shape {
    pub fn stored(&self) -> Stored {
        Stored {
            wire: self.wire,
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
            tools: digest(&serde_json::to_vec(&self.tools).expect("tools serialise")),
            user: self.user.clone(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.stored()).expect("shape serialises")
    }

    pub fn parse(s: &str) -> Option<Stored> {
        serde_json::from_str(s).ok()
    }
}

/// FNV-1a, 64 bits, hex: enough to tell two tool lists apart.
fn digest(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// One call, protocol-neutral. Cache breakpoints, where a protocol takes explicit ones,
/// go after block A, after block B and on the last item.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub shape: &'a Shape,
    /// The segment id: the same for every call on one log.
    pub cache_key: String,
    /// Blocks A, B and C; empty ones are not sent.
    pub system: [&'a str; 3],
    pub items: &'a [Item],
    pub choice: Choice<'a>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_round_trip_byte_for_byte() {
        let raw = r#"{"type":"thinking","thinking":"","signature":"s1=="}"#.to_owned();
        let items = [
            Item::User {
                text: "hi \"there\"".into(),
                image: None,
            },
            Item::Assistant {
                parts: vec![
                    Part::Opaque {
                        wire: WireApi::Messages,
                        block: raw,
                    },
                    Part::Text {
                        text: "Yes.".into(),
                    },
                    Part::ToolCall(ToolCall {
                        id: "c1".into(),
                        name: "search".into(),
                        arguments: "{\"query\": \"x\"}".into(),
                    }),
                ],
            },
            Item::ToolResult {
                call_id: "c1".into(),
                output: "found".into(),
            },
        ];
        for item in &items {
            let bytes = item.to_bytes();
            let back = Item::parse(&bytes).unwrap();
            assert_eq!(&back, item);
            assert_eq!(back.to_bytes(), bytes, "stable bytes");
        }
        // The opaque block is kept exactly, spacing and key order included.
        let Item::Assistant { parts } = Item::parse(&items[1].to_bytes()).unwrap() else {
            panic!()
        };
        assert!(matches!(&parts[0], Part::Opaque { block, .. }
                if block == r#"{"type":"thinking","thinking":"","signature":"s1=="}"#));
        assert_eq!(items[0].harness_text(), Some("hi \"there\""));
        assert_eq!(items[1].harness_text(), None);
    }
}
