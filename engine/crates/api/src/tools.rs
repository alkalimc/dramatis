//! Tool argument types and the JSON schemas the model sees, derived from them.
//!
//! Two lists. The **shared** list sits in the static prefix every person's session starts
//! with, so it is byte-identical across the whole roster and forms one cache line; which
//! calls a given person may make is checked where the call executes, never by varying the
//! list. The **host** list sits only in the host's own session.
//!
//! Things the model never supplies, because the harness fills them: the searching person
//! (`as_person`), a memory's audience, a note's path, the caller's remaining turns.
//!
//! Serialization is deterministic (keys sorted at every level): these bytes are part of a
//! cached prefix, and a reordered key is a cache miss for every session.

use std::collections::BTreeMap;

use schemars::transform::RestrictFormats;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::endpoints::WireApi;
use crate::types::Mode;

// ---- shared ----

/// Search the archive for passages about something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Search {
    /// What to look for, in plain words.
    pub query: String,
    /// Only passages about these people (roster ids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persons: Option<Vec<String>>,
}

/// Read one passage in full, optionally with its neighbours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetChunk {
    /// The passage id, as shown in `[#id]`.
    pub id: String,
    /// How many adjacent passages to include on each side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 3))]
    pub neighbours: Option<u8>,
}

/// Ask a colleague to join this conversation. Give a person, or a topic to find one by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestJoin {
    /// Roster id of the colleague.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// What they should know about, when you do not know who.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Turns to hand over from your own, while looking into a request. Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub turns: Option<u32>,
}

/// Remember one thing from this conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remember {
    /// The thing to remember, as one short sentence.
    pub text: String,
}

/// Answer the request you were asked to look into. Ends the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// Your reply, as you would say it.
    pub text: String,
    /// Ids of the passages the reply rests on.
    #[serde(default)]
    pub cites: Vec<String>,
    /// A longer written note to leave with the reply, in Markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Close the conversation: what to remember from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Wrapup {
    /// Concrete things that happened or were promised. May be empty.
    #[serde(default)]
    pub facts: Vec<WrapupFact>,
    /// Only for clear cruelty, insult, humiliation or betrayal aimed at a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hurt: Option<Hurt>,
    /// A summary of the conversation so far, when asked for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WrapupFact {
    pub kind: WrapupKind,
    /// One short sentence.
    pub text: String,
    /// For a commitment: when it is due, ISO 8601 date or date-time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WrapupKind {
    /// A preference, or a specific thing that happened.
    Fact,
    /// Something someone promised to do, with a time.
    Commitment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hurt {
    /// Roster id of the person it was aimed at; needed only in a group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// The user's words, quoted exactly.
    pub quote: String,
}

// ---- host ----

/// What changed since the user last looked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Digest {}

/// Find people who know about something.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindPeople {
    /// The subject, in plain words.
    pub topic: String,
}

/// Ask a person to look into a question for the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    /// Roster id.
    pub person: String,
    /// The question, as the user would put it.
    pub question: String,
    /// Turn budget. Leave out to use the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub turns: Option<u32>,
}

/// Start a group conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroup {
    /// Roster ids.
    #[schemars(length(min = 1))]
    pub members: Vec<String>,
    /// What the group is about. Leave out for casual talk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// Change whether a person or a group may speak unprompted. Only when the user asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMode {
    pub target_kind: TargetKind,
    /// Roster id, or group id.
    pub target: String,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Person,
    Group,
}

/// Search the whole archive. Sees every passage and no memories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostSearch {
    /// What to look for, in plain words.
    pub query: String,
    /// Only passages about these people (roster ids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persons: Option<Vec<String>>,
}

/// Write down one thing for the user. Only when the user asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostRemember {
    /// The thing to remember, as one short sentence.
    pub text: String,
}

/// Delete one remembered thing for the user. Only when the user asks. Can be restored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Forget {
    /// The id of the remembered thing.
    pub fact_id: String,
}

// ---- dispatch ----

/// A parsed call from a person's session.
#[derive(Debug, Clone, PartialEq)]
pub enum SharedCall {
    Search(Search),
    GetChunk(GetChunk),
    RequestJoin(RequestJoin),
    Remember(Remember),
    Report(Report),
    Wrapup(Wrapup),
}

/// A parsed call from the host's session.
#[derive(Debug, Clone, PartialEq)]
pub enum HostCall {
    Digest(Digest),
    FindPeople(FindPeople),
    Ask(Ask),
    CreateGroup(CreateGroup),
    SetMode(SetMode),
    Search(HostSearch),
    Remember(HostRemember),
    Forget(Forget),
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("unknown tool `{0}`")]
    Unknown(String),
    #[error("bad arguments for `{tool}`: {source}")]
    Arguments {
        tool: String,
        source: serde_json::Error,
    },
}

fn args<T: for<'de> Deserialize<'de>>(tool: &str, raw: &str) -> Result<T, CallError> {
    // Some endpoints send `""` for a tool with no parameters.
    let raw = if raw.trim().is_empty() { "{}" } else { raw };
    serde_json::from_str(raw).map_err(|source| CallError::Arguments {
        tool: tool.to_owned(),
        source,
    })
}

impl SharedCall {
    /// Parse a tool call as the endpoint returned it: a name and a JSON argument string.
    pub fn parse(name: &str, raw: &str) -> Result<Self, CallError> {
        Ok(match name {
            "search" => Self::Search(args(name, raw)?),
            "get_chunk" => Self::GetChunk(args(name, raw)?),
            "request_join" => Self::RequestJoin(args(name, raw)?),
            "remember" => Self::Remember(args(name, raw)?),
            "report" => Self::Report(args(name, raw)?),
            "wrapup" => Self::Wrapup(args(name, raw)?),
            _ => return Err(CallError::Unknown(name.to_owned())),
        })
    }
}

impl HostCall {
    pub fn parse(name: &str, raw: &str) -> Result<Self, CallError> {
        Ok(match name {
            "digest" => Self::Digest(args(name, raw)?),
            "find_people" => Self::FindPeople(args(name, raw)?),
            "ask" => Self::Ask(args(name, raw)?),
            "create_group" => Self::CreateGroup(args(name, raw)?),
            "set_mode" => Self::SetMode(args(name, raw)?),
            "search" => Self::Search(args(name, raw)?),
            "remember" => Self::Remember(args(name, raw)?),
            "forget" => Self::Forget(args(name, raw)?),
            _ => return Err(CallError::Unknown(name.to_owned())),
        })
    }
}

// ---- schemas ----

/// One tool as offered to a model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
}

/// Replacement tool descriptions keyed `tool.<name>` for the shared list and
/// `tool.host.<name>` for the host list (the two share names but not meaning). Parameter
/// descriptions are part of the schema and are not overridable. Missing keys keep the
/// neutral English default, which is the struct's doc comment.
pub type Descriptions = BTreeMap<String, String>;

fn spec<T: JsonSchema>(key: &str, name: &'static str, overrides: &Descriptions) -> ToolSpec {
    let generator = SchemaSettings::draft2020_12()
        .with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        })
        // `uint8` and friends are schemars' own formats; strict endpoints reject them.
        .with_transform({
            let mut only_standard = RestrictFormats::default();
            only_standard.infer_from_meta_schema = false;
            only_standard
        })
        .into_generator();
    let mut parameters = generator.into_root_schema_for::<T>().to_value();
    let obj = parameters
        .as_object_mut()
        .expect("tool arguments are objects");
    let default = obj
        .remove("description")
        .and_then(|d| d.as_str().map(str::to_owned));
    obj.remove("title");
    // An argument-less tool still needs an explicit empty object for some endpoints.
    obj.entry("properties").or_insert_with(|| json!({}));
    parameters.sort_all_objects();
    let description = overrides
        .get(&format!("{key}.{name}"))
        .cloned()
        .or(default)
        .unwrap_or_default();
    ToolSpec {
        name,
        description,
        parameters,
    }
}

/// The list in every person's session, in a fixed order.
pub fn shared_tools(overrides: &Descriptions) -> Vec<ToolSpec> {
    vec![
        spec::<Search>("tool", "search", overrides),
        spec::<GetChunk>("tool", "get_chunk", overrides),
        spec::<RequestJoin>("tool", "request_join", overrides),
        spec::<Remember>("tool", "remember", overrides),
        spec::<Report>("tool", "report", overrides),
        spec::<Wrapup>("tool", "wrapup", overrides),
    ]
}

/// The list in the host's session, in a fixed order.
pub fn host_tools(overrides: &Descriptions) -> Vec<ToolSpec> {
    vec![
        spec::<Digest>("tool.host", "digest", overrides),
        spec::<FindPeople>("tool.host", "find_people", overrides),
        spec::<Ask>("tool.host", "ask", overrides),
        spec::<CreateGroup>("tool.host", "create_group", overrides),
        spec::<SetMode>("tool.host", "set_mode", overrides),
        spec::<HostSearch>("tool.host", "search", overrides),
        spec::<HostRemember>("tool.host", "remember", overrides),
        spec::<Forget>("tool.host", "forget", overrides),
    ]
}

/// The `tools` array in the request shape of the given wire API.
pub fn to_wire(tools: &[ToolSpec], wire: WireApi) -> Value {
    let mut out = Value::Array(
        tools
            .iter()
            .map(|t| match wire {
                WireApi::Chat => json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                }),
                WireApi::Responses => json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }),
                WireApi::Messages => json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                }),
            })
            .collect(),
    );
    out.sort_all_objects();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_person_and_paths_are_never_model_arguments() {
        for t in shared_tools(&Descriptions::new())
            .iter()
            .chain(&host_tools(&Descriptions::new()))
        {
            let props = t.parameters["properties"].as_object().unwrap();
            for forbidden in ["as_person", "audience", "path", "note_path", "turns_left"] {
                assert!(
                    !props.contains_key(forbidden),
                    "{} exposes {forbidden}",
                    t.name
                );
            }
            assert!(!t.description.is_empty(), "{} has no description", t.name);
            let text = t.parameters.to_string();
            assert!(!text.contains("$ref"), "{} is not inlined", t.name);
            assert!(
                !text.contains("\"format\""),
                "{} has a non-standard format",
                t.name
            );
        }
    }

    #[test]
    fn parses_calls_and_rejects_unknowns() {
        let call = SharedCall::parse(
            "wrapup",
            r#"{"facts":[{"kind":"commitment","text":"t","due":"2026-01-02"}],"hurt":{"quote":"q"}}"#,
        )
        .unwrap();
        let SharedCall::Wrapup(w) = call else {
            panic!()
        };
        assert_eq!(w.facts[0].kind, WrapupKind::Commitment);
        assert_eq!(w.summary, None);

        assert!(matches!(
            HostCall::parse("digest", ""),
            Ok(HostCall::Digest(_))
        ));
        assert!(matches!(
            SharedCall::parse("digest", "{}"),
            Err(CallError::Unknown(_))
        ));
        assert!(matches!(
            SharedCall::parse("report", r#"{"text":"t","path":"/etc/x"}"#),
            Err(CallError::Arguments { .. })
        ));
        let SharedCall::Report(r) = SharedCall::parse("report", r#"{"text":"t"}"#).unwrap() else {
            panic!()
        };
        assert!(r.cites.is_empty());
        assert!(matches!(
            HostCall::parse(
                "set_mode",
                r#"{"target_kind":"group","target":"g","mode":"frozen"}"#
            ),
            Ok(HostCall::SetMode(SetMode {
                mode: Mode::Frozen,
                ..
            }))
        ));
    }

    #[test]
    fn description_seam() {
        let mut o = Descriptions::new();
        o.insert("tool.search".into(), "override".into());
        let shared = shared_tools(&o);
        assert_eq!(shared[0].description, "override");
        assert_ne!(shared[1].description, "override");
        assert_ne!(host_tools(&o)[5].description, "override");
        o.insert("tool.host.search".into(), "host override".into());
        assert_eq!(host_tools(&o)[5].description, "host override");
    }

    #[test]
    fn wire_shapes() {
        let tools = shared_tools(&Descriptions::new());
        let chat = to_wire(&tools, WireApi::Chat);
        assert_eq!(chat[0]["function"]["name"], "search");
        let resp = to_wire(&tools, WireApi::Responses);
        assert_eq!(resp[5]["name"], "wrapup");
        // Same bytes every time: part of a cached prefix.
        assert_eq!(chat.to_string(), to_wire(&tools, WireApi::Chat).to_string());
    }
}
