//! Replies -> [`Response`] and [`Event`]s, for all three protocols.

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::types::{Event, Part, Protocol, Response, Stop, Usage};

fn n(v: &Value, path: &str) -> u64 {
    v.pointer(path).and_then(Value::as_u64).unwrap_or(0)
}

fn s(v: &Value, path: &str) -> String {
    v.pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn endpoint_error(v: &Value) -> Error {
    let e = v.get("error").unwrap_or(v);
    let text = e
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| e.to_string());
    Error::Endpoint(text)
}

// ---- usage ---------------------------------------------------------------------------

/// Chat: OpenAI-style `prompt_tokens_details.cached_tokens`, or the hit/miss pair some
/// providers report instead. Completion tokens include reasoning.
pub fn chat_usage(u: &Value) -> Usage {
    let (uncached, cached) = match (
        u.get("prompt_cache_hit_tokens"),
        u.get("prompt_cache_miss_tokens"),
    ) {
        (Some(h), Some(m)) if h.is_u64() && m.is_u64() => {
            (m.as_u64().unwrap_or(0), h.as_u64().unwrap_or(0))
        }
        _ => {
            let cached = n(u, "/prompt_tokens_details/cached_tokens");
            (n(u, "/prompt_tokens").saturating_sub(cached), cached)
        }
    };
    Usage {
        uncached_in: uncached,
        cached_in: cached,
        cache_write_in: 0,
        output: n(u, "/completion_tokens"),
        reasoning: n(u, "/completion_tokens_details/reasoning_tokens"),
    }
}

/// Responses: `input_tokens` counts cached ones too; `output_tokens` includes reasoning.
pub fn responses_usage(u: &Value) -> Usage {
    let cached = n(u, "/input_tokens_details/cached_tokens");
    Usage {
        uncached_in: n(u, "/input_tokens").saturating_sub(cached),
        cached_in: cached,
        cache_write_in: 0,
        output: n(u, "/output_tokens"),
        reasoning: n(u, "/output_tokens_details/reasoning_tokens"),
    }
}

/// Messages: `input_tokens` is only the uncached part; `output_tokens` includes thinking.
/// Fields absent in `u` keep their value in `base`, which is how the usage of
/// `message_start` and that of `message_delta` combine.
pub fn messages_usage(base: Usage, u: &Value) -> Usage {
    let pick = |key: &str, old: u64| u.get(key).and_then(Value::as_u64).unwrap_or(old);
    Usage {
        uncached_in: pick("input_tokens", base.uncached_in),
        cached_in: pick("cache_read_input_tokens", base.cached_in),
        cache_write_in: pick("cache_creation_input_tokens", base.cache_write_in),
        output: pick("output_tokens", base.output),
        reasoning: 0,
    }
}

// ---- stop reasons --------------------------------------------------------------------

fn chat_stop(reason: &str) -> Stop {
    match reason {
        "stop" => Stop::EndTurn,
        "tool_calls" | "function_call" => Stop::ToolUse,
        "length" => Stop::MaxTokens,
        "content_filter" => Stop::Refusal,
        other => Stop::Other(other.to_owned()),
    }
}

fn messages_stop(reason: &str) -> Stop {
    match reason {
        "end_turn" | "stop_sequence" => Stop::EndTurn,
        "tool_use" => Stop::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => Stop::MaxTokens,
        "refusal" => Stop::Refusal,
        other => Stop::Other(other.to_owned()),
    }
}

// ---- whole replies -------------------------------------------------------------------

pub fn decode_response(protocol: Protocol, v: &Value) -> Result<Response> {
    match protocol {
        Protocol::OpenAiChat => chat_response(v),
        Protocol::OpenAiResponses => responses_response(v),
        Protocol::AnthropicMessages => messages_response(v),
    }
}

fn chat_response(v: &Value) -> Result<Response> {
    if v.get("error").is_some_and(|e| !e.is_null()) {
        return Err(endpoint_error(v));
    }
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| Error::Decode("no choices".into()))?;
    let msg = &choice["message"];
    let mut parts = Vec::new();
    if let Some(t) = msg
        .get("content")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        parts.push(Part::Text(t.to_owned()));
    }
    for c in msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        parts.push(Part::ToolCall {
            id: s(c, "/id"),
            name: s(c, "/function/name"),
            args_json: s(c, "/function/arguments"),
        });
    }
    let stop = if msg
        .get("refusal")
        .is_some_and(|r| r.as_str().is_some_and(|r| !r.is_empty()))
    {
        Stop::Refusal
    } else {
        chat_stop(
            choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("stop"),
        )
    };
    Ok(Response {
        parts,
        stop,
        usage: v.get("usage").map(chat_usage).unwrap_or_default(),
    })
}

/// One finished output item of a responses reply: its parts, and whether it refused.
fn responses_item(item: &Value, parts: &mut Vec<Part>) -> bool {
    let mut refused = false;
    match item["type"].as_str().unwrap_or_default() {
        "reasoning" => parts.push(Part::Native(item.clone())),
        "function_call" => parts.push(Part::ToolCall {
            id: s(item, "/call_id"),
            name: s(item, "/name"),
            args_json: s(item, "/arguments"),
        }),
        "message" => {
            for c in item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match c["type"].as_str().unwrap_or_default() {
                    "output_text" => parts.push(Part::Text(s(c, "/text"))),
                    "refusal" => refused = true,
                    _ => {}
                }
            }
        }
        _ => {}
    }
    refused
}

fn responses_stop(v: &Value, parts: &[Part], refused: bool) -> Stop {
    if refused {
        return Stop::Refusal;
    }
    if v.get("status").and_then(Value::as_str) == Some("incomplete") {
        return match s(v, "/incomplete_details/reason").as_str() {
            "max_output_tokens" => Stop::MaxTokens,
            "content_filter" => Stop::Refusal,
            other => Stop::Other(other.to_owned()),
        };
    }
    if parts.iter().any(|p| matches!(p, Part::ToolCall { .. })) {
        Stop::ToolUse
    } else {
        Stop::EndTurn
    }
}

fn responses_response(v: &Value) -> Result<Response> {
    if v.get("error").is_some_and(|e| !e.is_null()) || v["status"] == "failed" {
        return Err(endpoint_error(v));
    }
    let mut parts = Vec::new();
    let mut refused = false;
    for item in v
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        refused |= responses_item(item, &mut parts);
    }
    let stop = responses_stop(v, &parts, refused);
    Ok(Response {
        parts,
        stop,
        usage: v.get("usage").map(responses_usage).unwrap_or_default(),
    })
}

/// A finished content block of a messages reply.
fn messages_block(b: &Value, parts: &mut Vec<Part>) {
    match b["type"].as_str().unwrap_or_default() {
        "text" => parts.push(Part::Text(s(b, "/text"))),
        "tool_use" => parts.push(Part::ToolCall {
            id: s(b, "/id"),
            name: s(b, "/name"),
            args_json: b
                .get("input")
                .map(Value::to_string)
                .unwrap_or_else(|| "{}".into()),
        }),
        "thinking" | "redacted_thinking" => parts.push(Part::Native(b.clone())),
        _ => {}
    }
}

fn messages_response(v: &Value) -> Result<Response> {
    if v["type"] == "error" {
        return Err(endpoint_error(v));
    }
    let stop = messages_stop(
        v.get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("end_turn"),
    );
    let mut parts = Vec::new();
    // A refusal carries no usable content: it is read from the stop reason first.
    if stop != Stop::Refusal {
        for b in v
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            messages_block(b, &mut parts);
        }
    }
    let usage = v
        .get("usage")
        .map(|u| messages_usage(Usage::default(), u))
        .unwrap_or_default();
    Ok(Response { parts, stop, usage })
}

// ---- streams -------------------------------------------------------------------------

/// Incremental Server-Sent Events parser: bytes in, `data` payloads out.
#[derive(Debug, Default)]
pub struct Sse {
    buf: Vec<u8>,
}

impl Sse {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, skip)) = event_end(&self.buf) {
            let raw: Vec<u8> = self.buf.drain(..end + skip).collect();
            if let Some(d) = data_of(&String::from_utf8_lossy(&raw[..end])) {
                out.push(d);
            }
        }
        out
    }

    /// What is left when the body ends without a closing blank line.
    pub fn finish(&mut self) -> Vec<String> {
        let rest = std::mem::take(&mut self.buf);
        data_of(&String::from_utf8_lossy(&rest))
            .into_iter()
            .collect()
    }
}

fn event_end(buf: &[u8]) -> Option<(usize, usize)> {
    (0..buf.len()).find_map(|i| {
        if buf[i..].starts_with(b"\n\n") {
            Some((i, 2))
        } else if buf[i..].starts_with(b"\r\n\r\n") {
            Some((i, 4))
        } else {
            None
        }
    })
}

fn data_of(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(|d| d.strip_prefix(' ').unwrap_or(d))
        .collect();
    let joined = lines.join("\n");
    (!lines.is_empty() && joined != "[DONE]").then_some(joined)
}

/// One decoder per streamed reply.
pub struct Decoder(Inner);

enum Inner {
    Chat(ChatStream),
    Responses(ResponsesStream),
    Messages(MessagesStream),
}

impl Decoder {
    pub fn new(protocol: Protocol) -> Self {
        Self(match protocol {
            Protocol::OpenAiChat => Inner::Chat(ChatStream::default()),
            Protocol::OpenAiResponses => Inner::Responses(ResponsesStream::default()),
            Protocol::AnthropicMessages => Inner::Messages(MessagesStream::default()),
        })
    }

    pub fn push(&mut self, event: &Value) -> Result<Vec<Event>> {
        match &mut self.0 {
            Inner::Chat(d) => d.push(event),
            Inner::Responses(d) => d.push(event),
            Inner::Messages(d) => d.push(event),
        }
    }

    /// The body ended. Chat has no closing event of its own, so `Done` comes here.
    pub fn finish(&mut self) -> Result<Vec<Event>> {
        match &mut self.0 {
            Inner::Chat(d) => Ok(d.finish()),
            Inner::Responses(d) if d.done => Ok(Vec::new()),
            Inner::Messages(d) if d.done => Ok(Vec::new()),
            _ => Err(Error::Decode("the stream ended without a reply".into())),
        }
    }
}

#[derive(Default)]
struct ChatStream {
    text: String,
    calls: Vec<(u64, String, String, String)>,
    stop: Option<Stop>,
    usage: Usage,
    refused: bool,
    flushed: bool,
}

impl ChatStream {
    fn push(&mut self, v: &Value) -> Result<Vec<Event>> {
        if v.get("error").is_some_and(|e| !e.is_null()) {
            return Err(endpoint_error(v));
        }
        let mut out = Vec::new();
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.usage = chat_usage(u);
        }
        let Some(choice) = v.pointer("/choices/0") else {
            return Ok(out);
        };
        let delta = &choice["delta"];
        for tc in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0);
            let at = match self.calls.iter().position(|c| c.0 == index) {
                Some(p) => p,
                None => {
                    self.calls
                        .push((index, String::new(), String::new(), String::new()));
                    self.calls.len() - 1
                }
            };
            let slot = &mut self.calls[at];
            slot.1.push_str(&s(tc, "/id"));
            slot.2.push_str(&s(tc, "/function/name"));
            slot.3.push_str(&s(tc, "/function/arguments"));
        }
        if let Some(t) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            self.text.push_str(t);
            out.push(Event::TextDelta(t.to_owned()));
        }
        // Some providers stream their reasoning under one of these names.
        for key in ["reasoning_content", "reasoning"] {
            if let Some(t) = delta
                .get(key)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                out.push(Event::ReasoningDelta(t.to_owned()));
            }
        }
        if delta
            .get("refusal")
            .is_some_and(|r| r.as_str().is_some_and(|r| !r.is_empty()))
        {
            self.refused = true;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop = Some(chat_stop(reason));
            out.extend(self.flush_calls());
        }
        Ok(out)
    }

    fn flush_calls(&mut self) -> Vec<Event> {
        if std::mem::replace(&mut self.flushed, true) {
            return Vec::new();
        }
        self.calls
            .iter()
            .map(|(_, id, name, args)| Event::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args_json: args.clone(),
            })
            .collect()
    }

    fn finish(&mut self) -> Vec<Event> {
        let mut out = self.flush_calls();
        let mut parts = Vec::new();
        if !self.text.is_empty() {
            parts.push(Part::Text(std::mem::take(&mut self.text)));
        }
        for (_, id, name, args) in std::mem::take(&mut self.calls) {
            parts.push(Part::ToolCall {
                id,
                name,
                args_json: args,
            });
        }
        let stop = if self.refused {
            Stop::Refusal
        } else {
            self.stop.take().unwrap_or(Stop::EndTurn)
        };
        out.push(Event::Done(Response {
            parts,
            stop,
            usage: self.usage,
        }));
        out
    }
}

#[derive(Default)]
struct ResponsesStream {
    parts: Vec<Part>,
    refused: bool,
    done: bool,
}

impl ResponsesStream {
    fn push(&mut self, v: &Value) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        match v["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                let t = s(v, "/delta");
                if !t.is_empty() {
                    out.push(Event::TextDelta(t));
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let t = s(v, "/delta");
                if !t.is_empty() {
                    out.push(Event::ReasoningDelta(t));
                }
            }
            "response.output_item.done" => {
                let before = self.parts.len();
                self.refused |= responses_item(&v["item"], &mut self.parts);
                for p in &self.parts[before..] {
                    if let Part::ToolCall {
                        id,
                        name,
                        args_json,
                    } = p
                    {
                        out.push(Event::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            args_json: args_json.clone(),
                        });
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                let r = &v["response"];
                let usage = r.get("usage").map(responses_usage).unwrap_or_default();
                let stop = responses_stop(r, &self.parts, self.refused);
                self.done = true;
                out.push(Event::Done(Response {
                    parts: std::mem::take(&mut self.parts),
                    stop,
                    usage,
                }));
            }
            "response.failed" => return Err(endpoint_error(v.pointer("/response").unwrap_or(v))),
            "error" => return Err(endpoint_error(v)),
            _ => {}
        }
        Ok(out)
    }
}

#[derive(Default)]
struct MessagesStream {
    /// The block being assembled, and the argument text of a tool call.
    block: Option<Value>,
    partial: String,
    parts: Vec<Part>,
    stop: Option<Stop>,
    usage: Usage,
    done: bool,
}

impl MessagesStream {
    fn push(&mut self, v: &Value) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        match v["type"].as_str().unwrap_or_default() {
            "message_start" => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.usage = messages_usage(self.usage, u);
                }
            }
            "content_block_start" => {
                self.block = Some(v["content_block"].clone());
                self.partial.clear();
            }
            "content_block_delta" => {
                let d = &v["delta"];
                let Some(block) = self.block.as_mut() else {
                    return Ok(out);
                };
                match d["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let t = s(d, "/text");
                        let cur = block["text"].as_str().unwrap_or_default().to_owned();
                        block["text"] = Value::String(cur + &t);
                        out.push(Event::TextDelta(t));
                    }
                    "thinking_delta" => {
                        let t = s(d, "/thinking");
                        let cur = block["thinking"].as_str().unwrap_or_default().to_owned();
                        block["thinking"] = Value::String(cur + &t);
                        out.push(Event::ReasoningDelta(t));
                    }
                    "signature_delta" => block["signature"] = json!(s(d, "/signature")),
                    "input_json_delta" => self.partial.push_str(&s(d, "/partial_json")),
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(mut block) = self.block.take() {
                    if block["type"] == "tool_use" && !self.partial.is_empty() {
                        block["input"] = serde_json::from_str(&self.partial)
                            .map_err(|e| Error::Decode(format!("tool arguments: {e}")))?;
                    }
                    let before = self.parts.len();
                    messages_block(&block, &mut self.parts);
                    if let Some(Part::ToolCall {
                        id,
                        name,
                        args_json,
                    }) = self.parts[before..].first()
                    {
                        out.push(Event::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            args_json: args_json.clone(),
                        });
                    }
                }
            }
            "message_delta" => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(messages_stop(r));
                }
                if let Some(u) = v.get("usage") {
                    self.usage = messages_usage(self.usage, u);
                }
            }
            "message_stop" => {
                self.done = true;
                let stop = self.stop.take().unwrap_or(Stop::EndTurn);
                let parts = if stop == Stop::Refusal {
                    Vec::new()
                } else {
                    std::mem::take(&mut self.parts)
                };
                out.push(Event::Done(Response {
                    parts,
                    stop,
                    usage: self.usage,
                }));
            }
            "error" => return Err(endpoint_error(v)),
            _ => {}
        }
        Ok(out)
    }
}
