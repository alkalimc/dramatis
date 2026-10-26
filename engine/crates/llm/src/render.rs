//! Request -> HTTP request, as a pure function: identical inputs give identical bytes.
//!
//! Object keys are sorted (`sort_all_objects`), arrays keep the order of the request, and
//! nothing per-call (timestamps, ids) is added. Each message maps to the same wire items
//! whatever follows it, so appending a message to a request only appends to the body's
//! `messages` / `input` array; every other field is a function of the request's shape.
//! Cache policy per protocol lives here and nowhere else.

use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::types::{Effort, Endpoint, Message, Part, Protocol, Request, Role, ToolChoice};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Messages requires `max_tokens`; thinking counts against it.
pub const DEFAULT_MAX_OUTPUT: u32 = 32_000;
/// Explicit breakpoints on the messages protocol.
const MESSAGES_BREAKPOINTS: u8 = 4;

/// One request, ready for a transport. `headers` never carries the secret: the
/// transport adds it with [`auth_headers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub fn render(ep: &Endpoint, req: &Request) -> Result<Rendered> {
    render_with(ep, req, false)
}

/// The same request asking for a stream of events.
pub fn render_stream(ep: &Endpoint, req: &Request) -> Result<Rendered> {
    render_with(ep, req, true)
}

fn render_with(ep: &Endpoint, req: &Request, stream: bool) -> Result<Rendered> {
    let (path, body) = match ep.protocol {
        Protocol::OpenAiChat => ("chat/completions", chat(ep, req, stream)?),
        Protocol::OpenAiResponses => ("responses", responses(ep, req, stream)?),
        Protocol::AnthropicMessages => ("messages", messages(ep, req, stream)?),
    };
    let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
    if ep.protocol == Protocol::AnthropicMessages {
        headers.push(("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned()));
    }
    Ok(Rendered {
        url: ep.url(path),
        headers,
        body: to_bytes(body),
    })
}

/// The authentication headers of an endpoint; empty for a keyless one.
pub fn auth_headers(ep: &Endpoint) -> Vec<(String, String)> {
    if ep.key.is_empty() {
        return Vec::new();
    }
    match ep.protocol {
        Protocol::AnthropicMessages => vec![("x-api-key".into(), ep.key.expose().to_owned())],
        _ => vec![(
            "authorization".into(),
            format!("Bearer {}", ep.key.expose()),
        )],
    }
}

/// Serialise with every object's keys sorted, whatever the `serde_json` features say.
pub fn to_bytes(mut v: Value) -> Vec<u8> {
    v.sort_all_objects();
    serde_json::to_vec(&v).expect("json values serialise")
}

fn check_choice(ep: &Endpoint, req: &Request) -> Result<()> {
    if let ToolChoice::Tool(_) = req.tool_choice
        && (ep.protocol == Protocol::AnthropicMessages || !ep.caps.forced_tool)
    {
        return Err(Error::ForcedToolNotAllowed);
    }
    Ok(())
}

fn args_value(args_json: &str) -> Value {
    if args_json.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(args_json).unwrap_or(Value::String(args_json.to_owned()))
}

fn native<'a>(ep: &Endpoint, v: &'a Value, accepted: &[&str]) -> Result<&'a Value> {
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
    if accepted.contains(&kind) {
        Ok(v)
    } else {
        Err(Error::ForeignNative {
            kind: kind.to_owned(),
            protocol: ep.protocol,
        })
    }
}

fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn has_image(parts: &[Part]) -> bool {
    parts.iter().any(|p| matches!(p, Part::Image { .. }))
}

// ---- OpenAI chat ---------------------------------------------------------------------

/// Chat takes `low | medium | high`; the higher levels map to `high`.
fn chat_effort(e: Effort) -> &'static str {
    match e {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High | Effort::XHigh | Effort::Max => "high",
    }
}

fn chat(ep: &Endpoint, req: &Request, stream: bool) -> Result<Value> {
    check_choice(ep, req)?;
    let mut messages: Vec<Value> = req
        .system
        .iter()
        .map(|b| json!({"role": "system", "content": b.text}))
        .collect();
    for m in &req.messages {
        chat_message(ep, m, &mut messages)?;
    }
    let mut body = Map::new();
    body.insert("model".into(), ep.model.clone().into());
    body.insert("messages".into(), messages.into());
    if stream {
        body.insert("stream".into(), true.into());
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if let Some(e) = req.effort {
        body.insert("reasoning_effort".into(), chat_effort(e).into());
    }
    if let Some(n) = req.max_output {
        body.insert("max_completion_tokens".into(), n.into());
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.schema}})
            })
            .collect();
        body.insert("tools".into(), tools.into());
        body.insert(
            "tool_choice".into(),
            match &req.tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Tool(n) => json!({"type": "function", "function": {"name": n}}),
            },
        );
    }
    if let Some(schema) = &req.output_schema {
        body.insert(
            "response_format".into(),
            json!({"type": "json_schema",
                   "json_schema": {"name": "output", "schema": schema, "strict": true}}),
        );
    }
    Ok(body.into())
}

fn chat_message(ep: &Endpoint, m: &Message, out: &mut Vec<Value>) -> Result<()> {
    match m.role {
        Role::User => {
            let content = if has_image(&m.parts) {
                let items: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(json!({"type": "text", "text": t})),
                        Part::Image { mime, data } => Some(json!({"type": "image_url",
                            "image_url": {"url": format!("data:{mime};base64,{data}")}})),
                        _ => None,
                    })
                    .collect();
                Value::Array(items)
            } else {
                Value::String(text_of(&m.parts))
            };
            out.push(json!({"role": "user", "content": content}));
        }
        Role::Tool => {
            for p in &m.parts {
                if let Part::ToolResult {
                    call_id, content, ..
                } = p
                {
                    out.push(json!({"role": "tool", "tool_call_id": call_id, "content": content}));
                }
            }
        }
        Role::Assistant => {
            let mut calls = Vec::new();
            for p in &m.parts {
                match p {
                    Part::ToolCall {
                        id,
                        name,
                        args_json,
                    } => calls.push(json!({
                        "id": id, "type": "function",
                        "function": {"name": name, "arguments": args_json}})),
                    Part::Native(v) => {
                        native(ep, v, &[])?;
                    }
                    _ => {}
                }
            }
            let text = text_of(&m.parts);
            let mut msg = Map::new();
            msg.insert("role".into(), "assistant".into());
            msg.insert(
                "content".into(),
                if text.is_empty() && !calls.is_empty() {
                    Value::Null
                } else {
                    text.into()
                },
            );
            if !calls.is_empty() {
                msg.insert("tool_calls".into(), calls.into());
            }
            out.push(msg.into());
        }
    }
    Ok(())
}

// ---- OpenAI responses ----------------------------------------------------------------

fn responses(ep: &Endpoint, req: &Request, stream: bool) -> Result<Value> {
    check_choice(ep, req)?;
    let mut input: Vec<Value> = req
        .system
        .iter()
        .map(|b| json!({"role": "system", "content": b.text}))
        .collect();
    for m in &req.messages {
        responses_message(ep, m, &mut input)?;
    }
    let mut body = Map::new();
    body.insert("model".into(), ep.model.clone().into());
    body.insert("input".into(), input.into());
    // Nothing is kept server side: the full input is sent every turn and the cache key
    // routes it to the entry the last turn wrote.
    body.insert("store".into(), false.into());
    if let Some(k) = &req.cache_key {
        body.insert("prompt_cache_key".into(), k.clone().into());
    }
    if stream {
        body.insert("stream".into(), true.into());
    }
    if let Some(e) = req.effort {
        body.insert("reasoning".into(), json!({"effort": e.as_str()}));
        // Without server-side storage a reasoning item can only be passed back in its
        // encrypted form.
        body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
    if let Some(n) = req.max_output {
        body.insert("max_output_tokens".into(), n.into());
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "name": t.name,
                       "description": t.description, "parameters": t.schema})
            })
            .collect();
        body.insert("tools".into(), tools.into());
        body.insert(
            "tool_choice".into(),
            match &req.tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Tool(n) => json!({"type": "function", "name": n}),
            },
        );
    }
    if let Some(schema) = &req.output_schema {
        body.insert(
            "text".into(),
            json!({"format": {"type": "json_schema", "name": "output",
                              "schema": schema, "strict": true}}),
        );
    }
    Ok(body.into())
}

fn responses_message(ep: &Endpoint, m: &Message, out: &mut Vec<Value>) -> Result<()> {
    match m.role {
        Role::User => {
            let content = if has_image(&m.parts) {
                let items: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(json!({"type": "input_text", "text": t})),
                        Part::Image { mime, data } => Some(json!({"type": "input_image",
                            "image_url": format!("data:{mime};base64,{data}")})),
                        _ => None,
                    })
                    .collect();
                Value::Array(items)
            } else {
                Value::String(text_of(&m.parts))
            };
            out.push(json!({"role": "user", "content": content}));
        }
        Role::Tool => {
            for p in &m.parts {
                if let Part::ToolResult {
                    call_id, content, ..
                } = p
                {
                    out.push(json!({"type": "function_call_output",
                                    "call_id": call_id, "output": content}));
                }
            }
        }
        Role::Assistant => {
            // Items keep the order the model produced them in; adjacent text joins.
            let mut text = String::new();
            let flush = |text: &mut String, out: &mut Vec<Value>| {
                if !text.is_empty() {
                    out.push(json!({"role": "assistant", "content": std::mem::take(text)}));
                }
            };
            for p in &m.parts {
                match p {
                    Part::Text(t) => text.push_str(t),
                    Part::ToolCall {
                        id,
                        name,
                        args_json,
                    } => {
                        flush(&mut text, out);
                        out.push(json!({"type": "function_call", "call_id": id,
                                        "name": name, "arguments": args_json}));
                    }
                    Part::Native(v) => {
                        flush(&mut text, out);
                        out.push(native(ep, v, &["reasoning"])?.clone());
                    }
                    _ => {}
                }
            }
            flush(&mut text, out);
        }
    }
    Ok(())
}

// ---- Anthropic messages --------------------------------------------------------------

fn cache_control() -> Value {
    json!({"type": "ephemeral"})
}

fn messages(ep: &Endpoint, req: &Request, stream: bool) -> Result<Value> {
    check_choice(ep, req)?;
    let allowed = ep.caps.cache_breakpoints.unwrap_or(MESSAGES_BREAKPOINTS);
    let marks = req.system.iter().filter(|b| b.cache_mark).count();
    // One breakpoint is always left for the automatic one on the growing tail.
    if marks > usize::from(allowed.saturating_sub(1)) && allowed > 0 {
        return Err(Error::Invalid(format!(
            "{marks} cache marks and the automatic breakpoint exceed the {allowed} allowed"
        )));
    }
    let system: Vec<Value> = req
        .system
        .iter()
        .map(|b| {
            let mut v = json!({"type": "text", "text": b.text});
            if b.cache_mark && allowed > 0 {
                v["cache_control"] = cache_control();
            }
            v
        })
        .collect();
    let mut msgs = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        msgs.push(messages_message(ep, m)?);
    }
    let mut body = Map::new();
    body.insert("model".into(), ep.model.clone().into());
    body.insert(
        "max_tokens".into(),
        req.max_output.unwrap_or(DEFAULT_MAX_OUTPUT).into(),
    );
    body.insert("messages".into(), msgs.into());
    if !system.is_empty() {
        body.insert("system".into(), system.into());
    }
    if stream {
        body.insert("stream".into(), true.into());
    }
    // Moves to the last block of the request on every turn, so the previous turn's
    // entry is read and the new tail is written without touching earlier bytes.
    if allowed > 0 {
        body.insert("cache_control".into(), cache_control());
    }
    // Always on, whatever the effort: switching it off is rejected by current models and
    // would change the request shape.
    body.insert("thinking".into(), json!({"type": "adaptive"}));
    let mut output = Map::new();
    if let Some(e) = req.effort {
        output.insert("effort".into(), e.as_str().into());
    }
    if let Some(schema) = &req.output_schema {
        output.insert(
            "format".into(),
            json!({"type": "json_schema", "schema": schema}),
        );
    }
    if !output.is_empty() {
        body.insert("output_config".into(), output.into());
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(
                |t| json!({"name": t.name, "description": t.description, "input_schema": t.schema}),
            )
            .collect();
        body.insert("tools".into(), tools.into());
        body.insert(
            "tool_choice".into(),
            match &req.tool_choice {
                ToolChoice::Auto => json!({"type": "auto"}),
                ToolChoice::None => json!({"type": "none"}),
                ToolChoice::Tool(_) => unreachable!("rejected by check_choice"),
            },
        );
    }
    Ok(body.into())
}

fn messages_message(ep: &Endpoint, m: &Message) -> Result<Value> {
    let mut content = Vec::with_capacity(m.parts.len());
    for p in &m.parts {
        content.push(match p {
            Part::Text(t) => json!({"type": "text", "text": t}),
            Part::Image { mime, data } => json!({"type": "image",
                "source": {"type": "base64", "media_type": mime, "data": data}}),
            Part::ToolCall {
                id,
                name,
                args_json,
            } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": args_value(args_json)})
            }
            Part::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let mut v = json!({"type": "tool_result", "tool_use_id": call_id,
                                   "content": content});
                if *is_error {
                    v["is_error"] = true.into();
                }
                v
            }
            Part::Native(v) => native(ep, v, &["thinking", "redacted_thinking"])?.clone(),
        });
    }
    // Tool results travel in a user turn on this protocol.
    let role = if m.role == Role::Assistant {
        "assistant"
    } else {
        "user"
    };
    Ok(json!({"role": role, "content": content}))
}
