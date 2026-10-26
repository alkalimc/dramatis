//! Provider-neutral types. Nothing here names a wire format except [`Protocol`].

use std::fmt;

use serde_json::Value;

/// The external API protocols. Adding one means one new adapter in `render` and `decode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Protocol {
    /// `POST {base_url}/chat/completions`
    OpenAiChat,
    /// `POST {base_url}/responses`
    OpenAiResponses,
    /// `POST {base_url}/messages`
    AnthropicMessages,
}

/// A key held only as long as a request needs it. Never printed.
#[derive(Clone, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// For the transport only: the value goes into a request header and nowhere else.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// What the caller knows about a model. Unknown means conservative.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps {
    /// The endpoint honours a forced `tool_choice`. Never honoured on the messages
    /// protocol whatever this says: current models reject it and changing `tool_choice`
    /// drops the cached history.
    pub forced_tool: bool,
    /// Cache breakpoints the endpoint accepts. `None`: the protocol's own limit (four on
    /// messages); `Some(0)`: no explicit markers at all.
    pub cache_breakpoints: Option<u8>,
    pub context_window: Option<u32>,
    /// Shortest cacheable prefix, tokens.
    pub cache_min_prefix: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub protocol: Protocol,
    /// API root including the version segment.
    pub base_url: String,
    pub model: String,
    /// Empty: no authentication header (a local server).
    pub key: Secret,
    pub caps: Caps,
}

impl Endpoint {
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "xhigh" => Effort::XHigh,
            "max" => Effort::Max,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemBlock {
    pub text: String,
    /// End of a cacheable prefix. Honoured where the protocol has explicit markers.
    pub cache_mark: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema of the arguments.
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Tool(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    /// `data` is base64.
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
        is_error: bool,
    },
    /// A provider-native assistant block kept verbatim (thinking blocks with their
    /// signature, reasoning items). Only valid on the protocol that produced it.
    Native(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::Text(text.into())],
        }
    }

    pub fn assistant(parts: Vec<Part>) -> Self {
        Self {
            role: Role::Assistant,
            parts,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    pub system: Vec<SystemBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
    pub effort: Option<Effort>,
    pub max_output: Option<u32>,
    /// Stable per conversation segment; the responses protocol routes cache hits by it.
    pub cache_key: Option<String>,
    /// Ask for one JSON document matching this schema.
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
    Other(String),
}

/// Token counts, normalised across protocols. `output` includes `reasoning`, which is
/// the part of it the endpoint reported separately (zero where it does not).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub uncached_in: u64,
    pub cached_in: u64,
    pub cache_write_in: u64,
    pub output: u64,
    pub reasoning: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// In the order the model produced them; feed them back as an assistant message.
    pub parts: Vec<Part>,
    pub stop: Stop,
    pub usage: Usage,
}

impl Response {
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.parts.iter().filter_map(|p| match p {
            Part::ToolCall {
                id,
                name,
                args_json,
            } => Some((id.as_str(), name.as_str(), args_json.as_str())),
            _ => None,
        })
    }

    pub fn into_message(self) -> Message {
        Message::assistant(self.parts)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TextDelta(String),
    ReasoningDelta(String),
    /// A complete call, once its arguments are whole.
    ToolCall {
        id: String,
        name: String,
        args_json: String,
    },
    Done(Response),
}
