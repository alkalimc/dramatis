//! A model call in protocol-neutral terms: the session log's entries as stored, and the
//! [`llm::Request`] built from a segment.
//!
//! The log stores [`llm::Message`]s, serialised one per entry. A provider-native block
//! (a thinking block with its signature, a reasoning item) is kept as the JSON value it
//! arrived as and handed back unchanged. `llm::render` is a pure function of the request,
//! so a log that is only appended to yields bodies that each extend the previous one.

use api::endpoints::WireApi;
use llm::{Effort, Endpoint, Message, Part, Protocol, Request, Role, SystemBlock, ToolChoice};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

/// An image the user attached.
#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredRole {
    User,
    Assistant,
    Tool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
enum StoredPart {
    Text {
        text: String,
    },
    Image {
        mime: String,
        data: String,
    },
    ToolCall {
        id: String,
        name: String,
        args_json: String,
    },
    ToolResult {
        call_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    Native {
        block: Value,
    },
}

/// One appended entry of a session log.
#[derive(Serialize, Deserialize)]
struct Stored {
    role: StoredRole,
    parts: Vec<StoredPart>,
}

pub fn to_bytes(m: &Message) -> Vec<u8> {
    let stored = Stored {
        role: match m.role {
            Role::User => StoredRole::User,
            Role::Assistant => StoredRole::Assistant,
            Role::Tool => StoredRole::Tool,
        },
        parts: m
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => StoredPart::Text { text: t.clone() },
                Part::Image { mime, data } => StoredPart::Image {
                    mime: mime.clone(),
                    data: data.clone(),
                },
                Part::ToolCall {
                    id,
                    name,
                    args_json,
                } => StoredPart::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    args_json: args_json.clone(),
                },
                Part::ToolResult {
                    call_id,
                    content,
                    is_error,
                } => StoredPart::ToolResult {
                    call_id: call_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                },
                Part::Native(v) => StoredPart::Native { block: v.clone() },
            })
            .collect(),
    };
    serde_json::to_vec(&stored).expect("entries serialise")
}

pub fn parse(bytes: &[u8]) -> Option<Message> {
    let stored: Stored = serde_json::from_slice(bytes).ok()?;
    Some(Message {
        role: match stored.role {
            StoredRole::User => Role::User,
            StoredRole::Assistant => Role::Assistant,
            StoredRole::Tool => Role::Tool,
        },
        parts: stored
            .parts
            .into_iter()
            .map(|p| match p {
                StoredPart::Text { text } => Part::Text(text),
                StoredPart::Image { mime, data } => Part::Image { mime, data },
                StoredPart::ToolCall {
                    id,
                    name,
                    args_json,
                } => Part::ToolCall {
                    id,
                    name,
                    args_json,
                },
                StoredPart::ToolResult {
                    call_id,
                    content,
                    is_error,
                } => Part::ToolResult {
                    call_id,
                    content,
                    is_error,
                },
                StoredPart::Native { block } => Part::Native(block),
            })
            .collect(),
    })
}

/// A user entry: the words, then the image if there is one.
pub fn user(text: &str, image: Option<&Image>) -> Message {
    let mut m = Message::user(text);
    if let Some(i) = image {
        m.parts.push(Part::Image {
            mime: i.mime.clone(),
            data: i.data_base64.clone(),
        });
    }
    m
}

/// A tool's output for one call.
pub fn tool_result_part(call_id: &str, output: String) -> Part {
    Part::ToolResult {
        call_id: call_id.to_owned(),
        content: output,
        is_error: false,
    }
}

/// The outputs of one reply's calls: one entry, so the messages wire gets them in the
/// single user turn it requires.
pub fn tool_results(parts: Vec<Part>) -> Message {
    Message {
        role: Role::Tool,
        parts,
    }
}

/// The text the harness or the user put in a log: what material and ids are counted
/// from. Replies are the model's and are not material; an attached image is not text.
pub fn harness_texts(m: &Message) -> impl Iterator<Item = &str> {
    let harness = m.role != Role::Assistant;
    m.parts.iter().filter_map(move |p| match p {
        Part::Text(t) if harness => Some(t.as_str()),
        Part::ToolResult { content, .. } if harness => Some(content.as_str()),
        _ => None,
    })
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
pub struct StoredShape {
    pub wire: WireApi,
    pub model: String,
    pub reasoning: Option<String>,
    pub tools: String,
    pub user: String,
}

impl Shape {
    pub fn stored(&self) -> StoredShape {
        StoredShape {
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

    pub fn parse(s: &str) -> Option<StoredShape> {
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

/// One call on a segment. The forced-tool choice is the only thing that may differ
/// between calls on the same log.
#[derive(Debug, Clone)]
pub struct Call<'a> {
    pub shape: &'a Shape,
    /// The segment id: the same for every call on one log.
    pub cache_key: String,
    /// Blocks A, B and C; empty ones are not sent.
    pub system: [&'a str; 3],
    pub messages: &'a [Message],
    pub choice: ToolChoice,
}

/// The protocol-neutral request. Blocks A and B end cacheable prefixes (the host's log has
/// no B); C and the growing tail are left to the protocol's own moving breakpoint. When
/// the endpoint allows fewer explicit breakpoints than that, the later block keeps its
/// mark.
pub fn build(call: &Call<'_>, ep: &Endpoint) -> Result<Request> {
    if matches!(call.choice, ToolChoice::Tool(_))
        && (ep.protocol == Protocol::AnthropicMessages || !ep.caps.forced_tool)
    {
        return Err(Error::ForcedToolNotAllowed);
    }
    let mut budget = ep
        .caps
        .cache_breakpoints
        .map_or(usize::MAX, |n| usize::from(n.saturating_sub(1)));
    let mut system: Vec<SystemBlock> = Vec::with_capacity(3);
    for (i, text) in call.system.iter().enumerate().rev() {
        if text.is_empty() {
            continue;
        }
        let mark = i < 2 && budget > 0;
        budget -= usize::from(mark);
        system.push(SystemBlock {
            text: (*text).to_owned(),
            cache_mark: mark,
        });
    }
    system.reverse();
    Ok(Request {
        system,
        messages: call.messages.to_vec(),
        tools: call
            .shape
            .tools
            .iter()
            .map(|t| llm::ToolSpec {
                name: t.name.to_owned(),
                description: t.description.clone(),
                schema: t.parameters.clone(),
            })
            .collect(),
        tool_choice: call.choice.clone(),
        effort: call.shape.reasoning.as_deref().and_then(Effort::parse),
        max_output: None,
        cache_key: Some(call.cache_key.clone()),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::Caps;

    fn ep(protocol: Protocol, caps: Caps) -> Endpoint {
        Endpoint {
            protocol,
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            key: Default::default(),
            caps,
        }
    }

    fn shape() -> Shape {
        Shape {
            wire: WireApi::Messages,
            model: "m".into(),
            reasoning: Some("high".into()),
            tools: api::tools::shared_tools(&Default::default()),
            user: "u".into(),
        }
    }

    #[test]
    fn entries_round_trip_byte_for_byte() {
        let raw: Value =
            serde_json::from_str(r#"{"type":"thinking","thinking":"","signature":"s1=="}"#)
                .unwrap();
        let msgs = [
            user(
                "hi \"there\"",
                Some(&Image {
                    mime: "image/png".into(),
                    data_base64: "AAAA".into(),
                }),
            ),
            Message::assistant(vec![
                Part::Native(raw.clone()),
                Part::Text("Yes.".into()),
                Part::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    args_json: "{\"query\": \"x\"}".into(),
                },
            ]),
            tool_results(vec![tool_result_part("c1", "found".into())]),
        ];
        for m in &msgs {
            let bytes = to_bytes(m);
            let back = parse(&bytes).unwrap();
            assert_eq!(&back, m);
            assert_eq!(to_bytes(&back), bytes, "stable bytes");
        }
        let back = parse(&to_bytes(&msgs[1])).unwrap();
        assert_eq!(back.parts[0], Part::Native(raw), "native block kept as is");
        assert_eq!(harness_texts(&msgs[0]).collect::<Vec<_>>(), ["hi \"there\""]);
        assert_eq!(harness_texts(&msgs[1]).count(), 0);
        assert_eq!(harness_texts(&msgs[2]).collect::<Vec<_>>(), ["found"]);
        assert!(parse(b"{\"item\":\"user\",\"text\":\"old\"}").is_none());
    }

    #[test]
    fn requests_mark_a_and_b_and_refuse_forcing_where_it_is_not_allowed() {
        let s = shape();
        let msgs = [Message::user("u")];
        let call = |choice| Call {
            shape: &s,
            cache_key: "seg.7".into(),
            system: ["A", "", "C"],
            messages: &msgs,
            choice,
        };
        let open = ep(Protocol::OpenAiResponses, Caps::default());
        let r = build(&call(ToolChoice::Auto), &open).unwrap();
        assert_eq!(
            r.system,
            [
                SystemBlock {
                    text: "A".into(),
                    cache_mark: true
                },
                SystemBlock {
                    text: "C".into(),
                    cache_mark: false
                }
            ]
        );
        assert_eq!(r.cache_key.as_deref(), Some("seg.7"));
        assert_eq!(r.effort, Some(Effort::High));
        // One explicit breakpoint is left for the moving one: no marks at all.
        let few = ep(
            Protocol::AnthropicMessages,
            Caps {
                cache_breakpoints: Some(1),
                ..Caps::default()
            },
        );
        assert!(
            build(&call(ToolChoice::Auto), &few)
                .unwrap()
                .system
                .iter()
                .all(|b| !b.cache_mark)
        );
        let forced = ToolChoice::Tool("wrapup".into());
        assert!(matches!(
            build(&call(forced.clone()), &open),
            Err(Error::ForcedToolNotAllowed)
        ));
        let can = ep(
            Protocol::OpenAiChat,
            Caps {
                forced_tool: true,
                ..Caps::default()
            },
        );
        assert!(build(&call(forced.clone()), &can).is_ok());
        let messages = ep(
            Protocol::AnthropicMessages,
            Caps {
                forced_tool: true,
                ..Caps::default()
            },
        );
        assert!(matches!(
            build(&call(forced), &messages),
            Err(Error::ForcedToolNotAllowed)
        ));
    }
}
