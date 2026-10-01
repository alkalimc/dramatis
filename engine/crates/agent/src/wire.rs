//! Rendering a [`Request`] into the body of one protocol, byte for byte.
//!
//! A body is a fixed head (model, cache key, reasoning, stream flags, tools), the items
//! of the log, and a tail. Head and tail depend only on the request's shape and choice,
//! and rendering is a pure function of the request, so as long as the log is only
//! appended to, every body starts with the previous body minus its tail: each request
//! extends the last one's cached prefix. The only per-call difference, a forced
//! `tool_choice`, sits in the tail, after every item.

use api::endpoints::WireApi;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::request::{Choice, Image, Item, Part, Request};

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

pub fn render(req: &Request<'_>) -> Result<Body> {
    match req.shape.wire {
        WireApi::Chat | WireApi::Responses => Ok(openai(req)),
        WireApi::Messages => Err(Error::Unsupported("the messages wire")),
    }
}

fn openai(req: &Request<'_>) -> Body {
    let shape = req.shape;
    let wire = shape.wire;
    let mut out = Vec::with_capacity(4096);
    let s = |v: &str| serde_json::to_string(v).expect("strings serialise");
    let tools = api::tools::to_wire(&shape.tools, wire).to_string();
    out.extend_from_slice(b"{\"model\":");
    out.extend_from_slice(s(&shape.model).as_bytes());
    if wire == WireApi::Responses {
        out.extend_from_slice(b",\"prompt_cache_key\":");
        out.extend_from_slice(s(&req.cache_key).as_bytes());
        if let Some(r) = &shape.reasoning {
            out.extend_from_slice(b",\"reasoning\":{\"effort\":");
            out.extend_from_slice(s(r).as_bytes());
            out.push(b'}');
        }
        out.extend_from_slice(b",\"store\":false,\"stream\":true,\"tools\":");
        out.extend_from_slice(tools.as_bytes());
        out.extend_from_slice(b",\"input\":[");
    } else {
        if let Some(r) = &shape.reasoning {
            out.extend_from_slice(b",\"reasoning_effort\":");
            out.extend_from_slice(s(r).as_bytes());
        }
        out.extend_from_slice(
            b",\"stream\":true,\"stream_options\":{\"include_usage\":true},\"tools\":",
        );
        out.extend_from_slice(tools.as_bytes());
        out.extend_from_slice(b",\"messages\":[");
    }
    let mut first = true;
    let mut push = |v: Value, out: &mut Vec<u8>| {
        if !std::mem::take(&mut first) {
            out.push(b',');
        }
        out.extend(serde_json::to_vec(&v).expect("json serialises"));
    };
    for block in req.system.iter().filter(|b| !b.is_empty()) {
        push(json!({"content": block, "role": "system"}), &mut out);
    }
    for item in req.items {
        for v in openai_item(wire, item) {
            push(v, &mut out);
        }
    }
    let open_len = out.len();
    out.push(b']');
    let choice = match (req.choice, wire) {
        (Choice::Auto, _) => None,
        (Choice::None, _) => Some(json!("none")),
        (Choice::Tool(name), WireApi::Responses) => Some(json!({"name": name, "type": "function"})),
        (Choice::Tool(name), _) => Some(json!({"function": {"name": name}, "type": "function"})),
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

/// One log item as OpenAI-style input items (keys sorted, so the bytes are stable).
fn openai_item(wire: WireApi, item: &Item) -> Vec<Value> {
    match item {
        Item::User { text, image } => vec![user(wire, text, image.as_ref())],
        Item::ToolResult { call_id, output } => vec![match wire {
            WireApi::Responses => {
                json!({"call_id": call_id, "output": output, "type": "function_call_output"})
            }
            _ => json!({"content": output, "role": "tool", "tool_call_id": call_id}),
        }],
        Item::Assistant { parts } => {
            let text: String = parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let calls: Vec<_> = parts
                .iter()
                .filter_map(|p| match p {
                    Part::ToolCall(c) => Some(c),
                    _ => None,
                })
                .collect();
            if wire == WireApi::Responses {
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
                return v;
            }
            let mut m = serde_json::Map::new();
            m.insert(
                "content".into(),
                if text.is_empty() && !calls.is_empty() {
                    Value::Null
                } else {
                    Value::String(text)
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
    }
}

fn user(wire: WireApi, text: &str, image: Option<&Image>) -> Value {
    let Some(img) = image else {
        return json!({"content": text, "role": "user"});
    };
    let url = format!("data:{};base64,{}", img.mime, img.data_base64);
    let content = match wire {
        WireApi::Responses => json!([
            {"text": text, "type": "input_text"},
            {"image_url": url, "type": "input_image"},
        ]),
        _ => json!([
            {"text": text, "type": "text"},
            {"image_url": {"url": url}, "type": "image_url"},
        ]),
    };
    json!({"content": content, "role": "user"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Shape, ToolCall};

    fn shape(wire: WireApi) -> Shape {
        Shape {
            wire,
            model: "m".into(),
            reasoning: Some("low".into()),
            tools: api::tools::shared_tools(&Default::default()),
            user: "u".into(),
        }
    }

    fn items() -> Vec<Item> {
        vec![
            Item::User {
                text: "hi \"there\"".into(),
                image: None,
            },
            Item::Assistant {
                parts: vec![
                    Part::Opaque {
                        wire: WireApi::Messages,
                        block: "{}".into(),
                    },
                    Part::ToolCall(ToolCall {
                        id: "c1".into(),
                        name: "search".into(),
                        arguments: "{\"query\":\"x\"}".into(),
                    }),
                ],
            },
            Item::ToolResult {
                call_id: "c1".into(),
                output: "found".into(),
            },
        ]
    }

    fn req<'a>(shape: &'a Shape, items: &'a [Item], choice: Choice<'a>) -> Request<'a> {
        Request {
            shape,
            cache_key: "seg.7".into(),
            system: ["A", "", "C"],
            items,
            choice,
        }
    }

    #[test]
    fn bodies_extend_each_other_and_parse() {
        for wire in [WireApi::Chat, WireApi::Responses] {
            let s = shape(wire);
            let all = items();
            let b1 = render(&req(&s, &all[..1], Choice::Auto)).unwrap();
            let b2 = render(&req(&s, &all, Choice::Tool("wrapup"))).unwrap();
            assert!(b2.bytes.starts_with(b1.open()), "{wire:?}");
            assert_eq!(b2, render(&req(&s, &all, Choice::Tool("wrapup"))).unwrap());
            let v: Value = serde_json::from_slice(&b2.bytes).unwrap();
            let list = v.get("messages").or_else(|| v.get("input")).unwrap();
            // A, C (B is empty), the user turn, the call, its result.
            assert_eq!(list.as_array().unwrap().len(), 5, "{wire:?}");
            assert!(
                !list.to_string().contains("Opaque"),
                "foreign blocks are left out"
            );
            assert_eq!(v["stream"], true);
            assert!(v.get("text").is_none(), "no output format");
            assert!(v["tool_choice"].is_object());
        }
        let s = shape(WireApi::Responses);
        let v: Value =
            serde_json::from_slice(&render(&req(&s, &[], Choice::None)).unwrap().bytes).unwrap();
        assert_eq!(v["prompt_cache_key"], "seg.7");
        assert_eq!(v["reasoning"]["effort"], "low");
        assert_eq!(v["tool_choice"], "none");
        let s = shape(WireApi::Chat);
        let v: Value =
            serde_json::from_slice(&render(&req(&s, &[], Choice::Auto)).unwrap().bytes).unwrap();
        assert_eq!(v["reasoning_effort"], "low");
        assert!(v.get("prompt_cache_key").is_none());
        let s = shape(WireApi::Messages);
        assert!(matches!(
            render(&req(&s, &[], Choice::Auto)),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn images_become_parts() {
        let img = Image {
            mime: "image/png".into(),
            data_base64: "AAAA".into(),
        };
        for wire in [WireApi::Chat, WireApi::Responses] {
            let v = user(wire, "look", Some(&img));
            assert_eq!(v["content"].as_array().unwrap().len(), 2);
        }
    }
}
