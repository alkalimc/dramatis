//! Decoding a streamed reply, for both OpenAI-style wire APIs, into text deltas, tool
//! calls and usage.

use api::endpoints::WireApi;
use serde_json::Value;
use world::quota::Usage;

use crate::error::{Error, Result};
use crate::request::{Part, ToolCall};

/// What one decoded event contributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Nothing,
}

/// A finished reply: its parts in the order they were produced, and its usage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    pub parts: Vec<Part>,
    pub usage: Usage,
}

impl Reply {
    /// Every text part, joined.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn calls(&self) -> Vec<&ToolCall> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    fn push_text(&mut self, t: &str) {
        match self.parts.last_mut() {
            Some(Part::Text { text }) => text.push_str(t),
            _ => self.parts.push(Part::Text { text: t.to_owned() }),
        }
    }
}

/// Folds stream events into a [`Reply`].
#[derive(Debug)]
pub struct Decoder {
    wire: WireApi,
    reply: Reply,
    /// Chat streams tool calls in fragments keyed by index.
    partial: Vec<(u64, ToolCall)>,
    done: bool,
}

impl Decoder {
    pub fn new(wire: WireApi) -> Self {
        Self {
            wire,
            reply: Reply::default(),
            partial: Vec::new(),
            done: false,
        }
    }

    pub fn push(&mut self, event: &Value) -> Result<Piece> {
        match self.wire {
            WireApi::Chat => self.push_chat(event),
            WireApi::Responses => self.push_responses(event),
            WireApi::Messages => Err(Error::Unsupported("the messages wire")),
        }
    }

    fn push_chat(&mut self, v: &Value) -> Result<Piece> {
        if let Some(err) = v.get("error") {
            return Err(endpoint_error(err));
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.reply.usage = chat_usage(u);
            self.done = true;
        }
        let Some(delta) = v.pointer("/choices/0/delta") else {
            return Ok(Piece::Nothing);
        };
        for tc in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0);
            let slot = match self.partial.iter().position(|(i, _)| *i == index) {
                Some(p) => &mut self.partial[p].1,
                None => {
                    self.partial.push((
                        index,
                        ToolCall {
                            id: String::new(),
                            name: String::new(),
                            arguments: String::new(),
                        },
                    ));
                    &mut self.partial.last_mut().expect("just pushed").1
                }
            };
            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                slot.id.push_str(id);
            }
            if let Some(n) = tc.pointer("/function/name").and_then(Value::as_str) {
                slot.name.push_str(n);
            }
            if let Some(a) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                slot.arguments.push_str(a);
            }
        }
        match delta.get("content").and_then(Value::as_str) {
            Some(t) if !t.is_empty() => {
                self.reply.push_text(t);
                Ok(Piece::Text(t.to_owned()))
            }
            _ => Ok(Piece::Nothing),
        }
    }

    fn push_responses(&mut self, v: &Value) -> Result<Piece> {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "response.output_text.delta" => {
                let t = v.get("delta").and_then(Value::as_str).unwrap_or_default();
                if !t.is_empty() {
                    self.reply.push_text(t);
                }
                Ok(if t.is_empty() {
                    Piece::Nothing
                } else {
                    Piece::Text(t.to_owned())
                })
            }
            "response.output_item.done" => {
                let item = &v["item"];
                if item["type"] == "function_call" {
                    let s = |k: &str| item[k].as_str().unwrap_or_default().to_owned();
                    self.reply.parts.push(Part::ToolCall(ToolCall {
                        id: s("call_id"),
                        name: s("name"),
                        arguments: s("arguments"),
                    }));
                }
                Ok(Piece::Nothing)
            }
            "response.completed" | "response.incomplete" => {
                if let Some(u) = v.pointer("/response/usage").filter(|u| u.is_object()) {
                    self.reply.usage = responses_usage(u);
                }
                self.done = true;
                Ok(Piece::Nothing)
            }
            "response.failed" => Err(endpoint_error(
                v.pointer("/response/error").unwrap_or(&Value::Null),
            )),
            "error" => Err(endpoint_error(v)),
            _ => Ok(Piece::Nothing),
        }
    }

    /// The reply, once the stream has ended.
    pub fn finish(mut self) -> Result<Reply> {
        if self.wire == WireApi::Chat {
            let calls = self.partial.drain(..).map(|(_, c)| Part::ToolCall(c));
            self.reply.parts.extend(calls);
        }
        if !self.done && self.reply.parts.is_empty() {
            return Err(Error::Endpoint {
                detail: "the stream ended without a reply".into(),
            });
        }
        Ok(self.reply)
    }
}

fn n(v: &Value, path: &str) -> Option<u64> {
    v.pointer(path).and_then(Value::as_u64)
}

/// Chat-completions usage: OpenAI-style `prompt_tokens_details.cached_tokens`, or the
/// hit/miss pair some providers report instead. Completion tokens include reasoning.
fn chat_usage(u: &Value) -> Usage {
    let prompt = n(u, "/prompt_tokens").unwrap_or(0);
    let (uncached, cached) = match (
        n(u, "/prompt_cache_hit_tokens"),
        n(u, "/prompt_cache_miss_tokens"),
    ) {
        (Some(hit), Some(miss)) => (miss, hit),
        _ => {
            let cached = n(u, "/prompt_tokens_details/cached_tokens").unwrap_or(0);
            (prompt.saturating_sub(cached), cached)
        }
    };
    Usage {
        uncached,
        cached,
        output: n(u, "/completion_tokens").unwrap_or(0),
    }
}

/// Responses usage. Output tokens include reasoning tokens.
fn responses_usage(u: &Value) -> Usage {
    let input = n(u, "/input_tokens").unwrap_or(0);
    let cached = n(u, "/input_tokens_details/cached_tokens").unwrap_or(0);
    Usage {
        uncached: input.saturating_sub(cached),
        cached,
        output: n(u, "/output_tokens").unwrap_or(0),
    }
}

fn endpoint_error(v: &Value) -> Error {
    let detail = v
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string());
    Error::Endpoint { detail }
}

/// Split a Server-Sent Events body into its `data` payloads, `[DONE]` excluded.
pub fn sse_data(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut data: Vec<&str> = Vec::new();
    for line in body.lines().chain(std::iter::once("")) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !data.is_empty() {
                let joined = data.join("\n");
                if joined != "[DONE]" {
                    out.push(joined);
                }
                data.clear();
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(wire: WireApi, sse: &str) -> Result<(Reply, String)> {
        let mut d = Decoder::new(wire);
        let mut streamed = String::new();
        for data in sse_data(sse) {
            let v: Value = serde_json::from_str(&data).unwrap();
            if let Piece::Text(t) = d.push(&v)? {
                streamed.push_str(&t);
            }
        }
        Ok((d.finish()?, streamed))
    }

    #[test]
    fn chat_text_tool_calls_and_both_usage_forms() {
        let (r, streamed) = decode(
            WireApi::Chat,
            include_str!("../tests/fixtures/chat_tool.sse"),
        )
        .unwrap();
        assert_eq!(r.text(), "Let me check.");
        assert_eq!(streamed, r.text());
        let calls = r.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments, r#"{"query":"tea"}"#);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(
            r.usage,
            Usage {
                uncached: 30,
                cached: 1000,
                output: 12
            }
        );
        let (r, _) = decode(
            WireApi::Chat,
            include_str!("../tests/fixtures/chat_hit_miss.sse"),
        )
        .unwrap();
        assert_eq!(r.text(), "Hello.");
        assert_eq!(
            r.usage,
            Usage {
                uncached: 64,
                cached: 1920,
                output: 40
            }
        );
    }

    #[test]
    fn responses_text_calls_and_usage() {
        let (r, streamed) = decode(
            WireApi::Responses,
            include_str!("../tests/fixtures/responses_text.sse"),
        )
        .unwrap();
        assert_eq!(r.text(), "Good evening.");
        assert_eq!(streamed, "Good evening.");
        assert_eq!(
            r.usage,
            Usage {
                uncached: 200,
                cached: 8960,
                output: 57
            }
        );
        let (r, _) = decode(
            WireApi::Responses,
            include_str!("../tests/fixtures/responses_tool.sse"),
        )
        .unwrap();
        assert_eq!(r.calls()[0].name, "wrapup");
        assert_eq!(r.calls()[0].id, "call_w");
        assert!(r.text().is_empty());
    }

    #[test]
    fn errors_and_truncation() {
        let err = decode(
            WireApi::Responses,
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"overloaded\"}}}\n\n",
        );
        assert!(matches!(err, Err(Error::Endpoint { detail }) if detail == "overloaded"));
        assert!(decode(WireApi::Chat, "data: [DONE]\n\n").is_err());
        assert_eq!(
            sse_data("event: x\ndata: a\ndata: b\n\ndata: [DONE]\n\n"),
            ["a\nb"]
        );
    }
}
