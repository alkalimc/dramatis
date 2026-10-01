//! Request bodies, byte for byte.
//!
//! A body is a fixed head (model, cache key, reasoning, stream flags, tools), the items of
//! the log, and a tail. Head and tail depend only on the segment's shape, so as long as
//! the log is only appended to, every body starts with the previous body minus its tail:
//! each request extends the last one's cached prefix. The only per-call difference, a
//! forced `tool_choice`, sits in the tail, after every item.

use api::endpoints::WireApi;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What must stay byte-identical for a segment's lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub wire: WireApi,
    pub model: String,
    pub reasoning: Option<String>,
    /// The `tools` array exactly as sent.
    pub tools: String,
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
    pub fn to_json(&self) -> String {
        let stored = Stored {
            wire: self.wire,
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
            tools: digest(self.tools.as_bytes()),
            user: self.user.clone(),
        };
        serde_json::to_string(&stored).expect("shape serialises")
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

/// A body ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    pub bytes: Vec<u8>,
    /// Length of the head plus items: the part the next body starts with.
    pub open_len: usize,
}

impl Body {
    pub fn open(&self) -> &[u8] {
        &self.bytes[..self.open_len]
    }
}

/// The `tool_choice` of one call: the only part of a body that may differ between calls
/// on the same log, which is why it is written after every item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    Auto,
    /// No tool this time: the model must answer in text.
    None,
    Tool(&'a str),
}

/// The request for one call on a segment. `items` are the stored item bytes, prefix
/// blocks first; empty ones are skipped.
pub fn body(shape: &Shape, cache_key: &str, items: &[&[u8]], choice: Choice<'_>) -> Body {
    let mut out = Vec::with_capacity(4096);
    let s = |v: &str| serde_json::to_string(v).expect("strings serialise");
    out.extend_from_slice(b"{\"model\":");
    out.extend_from_slice(s(&shape.model).as_bytes());
    match shape.wire {
        WireApi::Responses => {
            out.extend_from_slice(b",\"prompt_cache_key\":");
            out.extend_from_slice(s(cache_key).as_bytes());
            if let Some(r) = &shape.reasoning {
                out.extend_from_slice(b",\"reasoning\":{\"effort\":");
                out.extend_from_slice(s(r).as_bytes());
                out.push(b'}');
            }
            out.extend_from_slice(b",\"store\":false,\"stream\":true,\"tools\":");
            out.extend_from_slice(shape.tools.as_bytes());
            out.extend_from_slice(b",\"input\":[");
        }
        WireApi::Chat => {
            if let Some(r) = &shape.reasoning {
                out.extend_from_slice(b",\"reasoning_effort\":");
                out.extend_from_slice(s(r).as_bytes());
            }
            out.extend_from_slice(
                b",\"stream\":true,\"stream_options\":{\"include_usage\":true},\"tools\":",
            );
            out.extend_from_slice(shape.tools.as_bytes());
            out.extend_from_slice(b",\"messages\":[");
        }
    }
    let mut first = true;
    for item in items.iter().filter(|i| !i.is_empty()) {
        if !first {
            out.push(b',');
        }
        first = false;
        out.extend_from_slice(item);
    }
    let open_len = out.len();
    out.push(b']');
    let choice = match (choice, shape.wire) {
        (Choice::Auto, _) => None,
        (Choice::None, _) => Some(json!("none")),
        (Choice::Tool(name), WireApi::Responses) => Some(json!({"name": name, "type": "function"})),
        (Choice::Tool(name), WireApi::Chat) => {
            Some(json!({"function": {"name": name}, "type": "function"}))
        }
    };
    if let Some(choice) = choice {
        out.extend_from_slice(b",\"tool_choice\":");
        out.extend_from_slice(choice.to_string().as_bytes());
    }
    out.push(b'}');
    Body {
        bytes: out,
        open_len,
    }
}

/// An image the user attached, as a data URL part.
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

fn text_message(role: &str, text: &str) -> Vec<u8> {
    // `serde_json` sorts nothing here: keys are written in this fixed order.
    format!(
        "{{\"content\":{},\"role\":\"{role}\"}}",
        serde_json::to_string(text).expect("strings serialise")
    )
    .into_bytes()
}

/// A prefix block (A, B or C). Empty text is an empty block, which is not sent.
pub fn system(text: &str) -> Vec<u8> {
    if text.is_empty() {
        Vec::new()
    } else {
        text_message("system", text)
    }
}

/// A user turn: the user's words or a harness turn, optionally with an image.
pub fn user(wire: WireApi, text: &str, image: Option<&Image>) -> Vec<u8> {
    let Some(img) = image else {
        return text_message("user", text);
    };
    let url = format!("data:{};base64,{}", img.mime, img.data_base64);
    let content = match wire {
        WireApi::Chat => json!([
            {"text": text, "type": "text"},
            {"image_url": {"url": url}, "type": "image_url"},
        ]),
        WireApi::Responses => json!([
            {"text": text, "type": "input_text"},
            {"image_url": url, "type": "input_image"},
        ]),
    };
    let mut v = json!({"content": content, "role": "user"});
    v.sort_all_objects();
    serde_json::to_vec(&v).expect("json serialises")
}

/// One tool call as the model made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// The model's reply: its text verbatim and its tool calls, as items.
pub fn assistant(wire: WireApi, text: &str, calls: &[ToolCall]) -> Vec<u8> {
    let items: Vec<Value> = match wire {
        WireApi::Chat => {
            let mut m = serde_json::Map::new();
            m.insert(
                "content".into(),
                if text.is_empty() && !calls.is_empty() {
                    Value::Null
                } else {
                    Value::String(text.into())
                },
            );
            m.insert("role".into(), "assistant".into());
            if !calls.is_empty() {
                m.insert(
                    "tool_calls".into(),
                    calls
                        .iter()
                        .map(|c| {
                            json!({
                                "function": {"arguments": c.arguments, "name": c.name},
                                "id": c.id,
                                "type": "function",
                            })
                        })
                        .collect(),
                );
            }
            vec![Value::Object(m)]
        }
        WireApi::Responses => {
            let mut v = Vec::new();
            if !text.is_empty() || calls.is_empty() {
                v.push(json!({"content": text, "role": "assistant"}));
            }
            v.extend(calls.iter().map(|c| {
                json!({
                    "arguments": c.arguments,
                    "call_id": c.id,
                    "name": c.name,
                    "type": "function_call",
                })
            }));
            v
        }
    };
    join(items)
}

/// A tool's result for one call.
pub fn tool_result(wire: WireApi, call_id: &str, output: &str) -> Vec<u8> {
    let v = match wire {
        WireApi::Chat => json!({"content": output, "role": "tool", "tool_call_id": call_id}),
        WireApi::Responses => {
            json!({"call_id": call_id, "output": output, "type": "function_call_output"})
        }
    };
    join(vec![v])
}

fn join(items: Vec<Value>) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, mut v) in items.into_iter().enumerate() {
        v.sort_all_objects();
        if i > 0 {
            out.push(b',');
        }
        out.extend(serde_json::to_vec(&v).expect("json serialises"));
    }
    out
}

/// The text of every string in an entry's items, for scanning a stored log.
pub fn texts(bytes: &[u8]) -> Vec<String> {
    let mut wrapped = Vec::with_capacity(bytes.len() + 2);
    wrapped.push(b'[');
    wrapped.extend_from_slice(bytes);
    wrapped.push(b']');
    let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&wrapped) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        collect(&item, &mut out);
    }
    out
}

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                // Image data is not text.
                if k != "image_url" {
                    collect(x, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(wire: WireApi) -> Shape {
        Shape {
            wire,
            model: "m".into(),
            reasoning: Some("low".into()),
            tools: "[]".into(),
            user: "u".into(),
        }
    }

    #[test]
    fn bodies_extend_each_other_and_parse() {
        for wire in [WireApi::Chat, WireApi::Responses] {
            let s = shape(wire);
            let a = system("rules");
            let u = user(wire, "hi \"there\"", None);
            let r = assistant(
                wire,
                "",
                &[ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: "{\"query\":\"x\"}".into(),
                }],
            );
            let t = tool_result(wire, "c1", "found");
            let b1 = body(&s, "k", &[&a, b"", &u], Choice::Auto);
            let b2 = body(&s, "k", &[&a, b"", &u, &r, &t], Choice::Tool("wrapup"));
            assert!(b2.bytes.starts_with(b1.open()), "{wire:?}");
            let v: Value = serde_json::from_slice(&b2.bytes).unwrap();
            let list = if wire == WireApi::Chat {
                &v["messages"]
            } else {
                &v["input"]
            };
            assert_eq!(list.as_array().unwrap().len(), 4);
            assert_eq!(v["stream"], true);
            assert!(v.get("text").is_none(), "no output format");
            assert!(v["tool_choice"].is_object());
            let texts = texts(&r);
            assert!(texts.iter().any(|t| t.contains("query")));
        }
        let b = body(&shape(WireApi::Responses), "seg.7", &[], Choice::None);
        let v: Value = serde_json::from_slice(&b.bytes).unwrap();
        assert_eq!(v["prompt_cache_key"], "seg.7");
        assert_eq!(v["reasoning"]["effort"], "low");
        assert_eq!(v["tool_choice"], "none");
        let v: Value =
            serde_json::from_slice(&body(&shape(WireApi::Chat), "x", &[], Choice::Auto).bytes)
                .unwrap();
        assert_eq!(v["reasoning_effort"], "low");
        assert!(v.get("prompt_cache_key").is_none());
    }

    #[test]
    fn images_become_parts_and_are_not_scanned() {
        let img = Image {
            mime: "image/png".into(),
            data_base64: "AAAA".into(),
        };
        for wire in [WireApi::Chat, WireApi::Responses] {
            let u = user(wire, "look", Some(&img));
            let v: Value = serde_json::from_slice(&u).unwrap();
            assert_eq!(v["content"].as_array().unwrap().len(), 2);
            assert_eq!(texts(&u).iter().filter(|t| t.contains("AAAA")).count(), 0);
        }
    }
}
