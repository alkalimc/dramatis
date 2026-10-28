//! The structured wrap-up on an endpoint that only emulates tools: the model is asked
//! for one JSON document, and an emulating proxy may not enforce the schema. These
//! helpers show the model the expected shape in words and read the reply strictly.

use serde_json::{Map, Value};
use world::clock::{self, Now};
use world::wrapup::{Args, MemoryKind};

/// The top-level keys of the `wrapup` arguments; anything else in a reply is dropped.
const KEYS: [&str; 3] = ["facts", "hurt", "summary"];

/// The JSON document in a reply: the text, or the content of one fenced code block that
/// wraps all of it. Only a JSON object whose known keys survive counts; a reply that
/// names other fields (and so has no `facts`) records nothing, and no field is guessed.
pub(crate) fn document(text: &str) -> Option<String> {
    let doc = unfence(text.trim());
    let Value::Object(map) = serde_json::from_str::<Value>(doc).ok()? else {
        return None;
    };
    let kept: Map<String, Value> = map
        .into_iter()
        .filter(|(k, _)| KEYS.contains(&k.as_str()))
        .collect();
    Some(Value::Object(kept).to_string())
}

pub(crate) fn unfence(text: &str) -> &str {
    let Some(body) = text.strip_prefix("```") else {
        return text;
    };
    // The info string (`json`) ends at the first line break.
    let Some((info, body)) = body.split_once('\n') else {
        return text;
    };
    // Any other kind of block (a call block, say) is left for its own reader.
    if !matches!(info.trim(), "" | "json" | "JSON") {
        return text;
    }
    match body.trim_end().strip_suffix("```") {
        Some(inner) if !inner.contains("```") => inner.trim(),
        _ => text,
    }
}

/// A sample object with the schema's own keys, so the shape in the request text can
/// never drift from the schema. Leaves show their description, or their type.
pub(crate) fn example(schema: &Value) -> String {
    sample(schema, schema, 0).to_string()
}

fn sample(root: &Value, node: &Value, depth: usize) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    if let Some(r) = node.get("$ref").and_then(Value::as_str) {
        let target = r.strip_prefix('#').and_then(|p| root.pointer(p));
        return target.map_or(Value::Null, |t| sample(root, t, depth + 1));
    }
    for key in ["oneOf", "anyOf"] {
        let Some(alts) = node.get(key).and_then(Value::as_array) else {
            continue;
        };
        let consts: Vec<&str> = alts
            .iter()
            .filter_map(|a| a.get("const").and_then(Value::as_str))
            .collect();
        if !consts.is_empty() && consts.len() == alts.len() {
            return Value::String(consts.join("|"));
        }
        let inner = alts
            .iter()
            .find(|a| a.get("type") != Some(&Value::from("null")));
        return inner.map_or(Value::Null, |a| sample(root, a, depth + 1));
    }
    let kind = match node.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("null"),
        _ => "string",
    };
    match kind {
        "object" => {
            let props = node.get("properties").and_then(Value::as_object);
            props
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), sample(root, v, depth + 1)))
                .collect::<Map<_, _>>()
                .into()
        }
        "array" => {
            let item = node
                .get("items")
                .map_or(Value::Null, |i| sample(root, i, depth + 1));
            Value::Array(vec![item])
        }
        _ => {
            let text = node
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or(kind);
            Value::String(format!("<{text}>"))
        }
    }
}

/// A commitment whose due time cannot be read (relative or prose, such as "tomorrow
/// afternoon") is kept as a plain fact with the words it was given, not dropped.
pub(crate) fn keep_undated(args: &mut Args, now: Now) {
    for m in &mut args.facts {
        if m.kind != MemoryKind::Commitment {
            continue;
        }
        let due = m.due.as_deref().map(str::trim).filter(|d| !d.is_empty());
        if due.is_some_and(|d| clock::parse_due(d, now).is_some()) {
            continue;
        }
        m.kind = MemoryKind::Fact;
        if let Some(d) = due {
            m.text = format!("{} ({d})", m.text.trim());
        }
        m.due = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_one_fenced_block() {
        let fenced = "```json\n{\"facts\": []}\n```";
        assert_eq!(document(fenced).unwrap(), "{\"facts\":[]}");
        assert!(document("```\n{\"facts\": []}\n```\n").is_some());
        assert!(document("{\"facts\": []}").is_some());
    }

    #[test]
    fn prose_and_several_blocks_are_not_a_document() {
        assert!(document("here: ```json\n{}\n```").is_none());
        assert!(document("```json\n{}\n```\nand ```json\n{}\n```").is_none());
        assert!(document("* a list").is_none());
        assert!(document("[1]").is_none());
    }

    #[test]
    fn unknown_top_level_keys_are_dropped() {
        let doc = document("{\"summary\": \"s\", \"appointments\": [{\"content\": \"x\"}]}");
        assert_eq!(doc.unwrap(), "{\"summary\":\"s\"}");
    }

    #[test]
    fn the_example_carries_the_schema_keys() {
        let spec = api::tools::shared_tools(&Default::default())
            .into_iter()
            .find(|t| t.name == "wrapup")
            .unwrap();
        let v: Value = serde_json::from_str(&example(&spec.parameters)).unwrap();
        let fact = &v["facts"][0];
        assert_eq!(fact["kind"], "fact|commitment");
        assert!(fact["text"].is_string() && fact["due"].is_string());
        assert!(v["hurt"]["quote"].is_string() && v["summary"].is_string());
        assert_eq!(v.as_object().unwrap().len(), 3);
    }
}
