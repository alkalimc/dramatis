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

/// Ask a colleague to join this conversation, when the matter is clearly outside what you
/// know or can do; anything you can answer yourself, you answer. Give a person, or a topic
/// to find one by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestJoin {
    /// Roster id of the colleague.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// What they should know about, when you do not know who.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Turns to hand over from your own, while looking into a request: at least 1 (the
    /// default), and at most what you have left after this one. Ignored outside a request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub turns: Option<u32>,
}

/// Ask the host to arrange something for the user: a way to talk to colleagues you point
/// the user to, or a group with some colleagues. You help the user yourself first; this is
/// for matters clearly outside what you know or can do, or when the user asks for someone
/// or a group. The user is only shown the suggestion and decides; nothing happens before.
/// Give exactly one of `find` and `group`, at most once per reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskHost {
    /// Names or roster ids of the colleagues you suggest the user goes to, at most three:
    /// the ones your reply names for this. In a group, the host's name has the user invite
    /// the host into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub find: Option<Vec<String>>,
    /// Roster ids of colleagues for the host to propose a group with, besides you; you are
    /// included. Start small: a few people who matter for the matter at hand; more can be
    /// invited later. To continue a group you already have with them, give its name as the
    /// purpose; any other purpose makes a new group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub group: Option<Vec<String>>,
    /// Optional: what it is for, in one short line (about sixty characters), for example
    /// "plan a birthday party". Whoever the user goes to, or every member, is told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// Answer a request the user delegated to you with ask. Never for an ordinary reply: a
/// reply to the user is just written. Ends the request.
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
    /// What is worth remembering: what the user said of their own life, what was decided or
    /// agreed, what passed between you, one or two short lines. Empty only when nothing but
    /// greetings were exchanged.
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

/// Find people who know about a subject. When the subject is one person (a name, an alias,
/// or a description that fits one person on the roster) only that person is offered. The
/// user is shown a way to talk to them (asked in a group, a way to invite them into it);
/// you cannot move the user yourself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindPeople {
    /// The subject, in plain words. The one the user picks is told it as the reason.
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

/// Start a group conversation among two or more people; start small, with a few people who
/// matter for the matter at hand, and invite more later. One person is never a group: the
/// user is then only shown a way to talk to them. The result gives the group id when one
/// was made, and the user is shown a way to open it; you cannot move the user yourself.
/// To continue a group you already have with the same people, give its name as the
/// purpose; any other purpose makes a new group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroup {
    /// Roster ids, at least two.
    #[schemars(length(min = 1))]
    pub members: Vec<String>,
    /// Why the group exists, in one short line (about sixty characters), for example
    /// "plan a birthday party". Every member is told. Leave out for casual talk.
    #[serde(default, alias = "topic", skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
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

/// List the user's conversations, most recent first, each with its id (for set_mode and
/// recent). The user is shown a way to open each; you cannot move the user yourself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListChannels {}

/// Set a reminder for the user: a promise with a due date. When it falls due the person
/// named brings it up as their opening, so a person is required. Only when the user asks.
/// At most twenty are pending at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Promise {
    /// What is promised, as one short sentence.
    pub text: String,
    /// When it is due: an ISO 8601 date or date-time, for example 2026-12-24 or
    /// 2026-12-24T18:00.
    pub due: String,
    /// Roster id of the person who brings it up when it is due.
    pub person: String,
}

/// Read the last few lines of one conversation, by the id `list_channels` shows. No
/// memories are included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recent {
    /// The conversation id, as listed.
    pub channel: String,
}

// ---- dispatch ----

/// A parsed call from a person's session.
#[derive(Debug, Clone, PartialEq)]
pub enum SharedCall {
    Search(Search),
    GetChunk(GetChunk),
    RequestJoin(RequestJoin),
    AskHost(AskHost),
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
    GetChunk(GetChunk),
    Remember(HostRemember),
    ListChannels(ListChannels),
    Promise(Promise),
    Recent(Recent),
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
            "ask_host" => Self::AskHost(args(name, raw)?),
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
            "get_chunk" => Self::GetChunk(args(name, raw)?),
            "remember" => Self::Remember(args(name, raw)?),
            "list_channels" => Self::ListChannels(args(name, raw)?),
            "promise" => Self::Promise(args(name, raw)?),
            "recent" => Self::Recent(args(name, raw)?),
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
/// `tool.host.<name>` for the host list (the two share names but not meaning). A parameter's
/// description is keyed `<tool key>.<param>`, and a parameter inside an object or a list of
/// objects `<tool key>.<param>.<inner>`. Missing keys keep the neutral English default,
/// which is the doc comment.
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
    describe_params(&mut parameters, &format!("{key}.{name}"), overrides);
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

/// Replace the description of each property of `schema` that has an override under `prefix`,
/// then look inside objects and lists of objects.
fn describe_params(schema: &mut Value, prefix: &str, overrides: &Descriptions) {
    let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (param, prop) in props {
        let key = format!("{prefix}.{param}");
        if let (Some(text), Some(obj)) = (overrides.get(&key), prop.as_object_mut()) {
            obj.insert("description".into(), Value::String(text.clone()));
        }
        match prop.get_mut("items") {
            Some(items) => describe_params(items, &key, overrides),
            None => describe_params(prop, &key, overrides),
        }
    }
}

/// The list in every person's session, in a fixed order.
pub fn shared_tools(overrides: &Descriptions) -> Vec<ToolSpec> {
    vec![
        spec::<Search>("tool", "search", overrides),
        spec::<GetChunk>("tool", "get_chunk", overrides),
        spec::<RequestJoin>("tool", "request_join", overrides),
        spec::<Report>("tool", "report", overrides),
        spec::<Wrapup>("tool", "wrapup", overrides),
        spec::<AskHost>("tool", "ask_host", overrides),
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
        spec::<GetChunk>("tool.host", "get_chunk", overrides),
        spec::<HostRemember>("tool.host", "remember", overrides),
        spec::<ListChannels>("tool.host", "list_channels", overrides),
        spec::<Promise>("tool.host", "promise", overrides),
        spec::<Recent>("tool.host", "recent", overrides),
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
    fn parameter_description_seam() {
        let plain = shared_tools(&Descriptions::new());
        let mut o = Descriptions::new();
        o.insert("tool.search.query".into(), "param override".into());
        o.insert("tool.wrapup.facts.due".into(), "nested override".into());
        o.insert("tool.host.search.persons".into(), "host param".into());
        let shared = shared_tools(&o);
        assert_eq!(
            shared[0].parameters["properties"]["query"]["description"],
            "param override"
        );
        assert_ne!(
            plain[0].parameters["properties"]["query"]["description"],
            "param override"
        );
        assert_eq!(
            shared[4].parameters["properties"]["facts"]["items"]["properties"]["due"]["description"],
            "nested override"
        );
        // The same parameter name in the other list is untouched.
        assert_ne!(
            shared[0].parameters["properties"]["persons"]["description"],
            "host param"
        );
        let host = host_tools(&o);
        assert_eq!(
            host[5].parameters["properties"]["persons"]["description"],
            "host param"
        );
        assert_ne!(
            host[5].parameters["properties"]["query"]["description"],
            "param override"
        );
    }

    #[test]
    fn wire_shapes() {
        let tools = shared_tools(&Descriptions::new());
        let chat = to_wire(&tools, WireApi::Chat);
        assert_eq!(chat[0]["function"]["name"], "search");
        let resp = to_wire(&tools, WireApi::Responses);
        assert_eq!(resp[4]["name"], "wrapup");
        // Same bytes every time: part of a cached prefix.
        assert_eq!(chat.to_string(), to_wire(&tools, WireApi::Chat).to_string());
    }
}
