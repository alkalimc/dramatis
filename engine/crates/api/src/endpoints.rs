//! Endpoint profiles and roles: the shape of `$DRAMATIS_HOME/endpoints.toml`.
//!
//! The user adds **profiles** (an OpenAI-style or an Anthropic-style endpoint) and points
//! each **role** at one model of one profile. The file holds no secret: a profile's API key lives only in the
//! system keychain under service [`KEYCHAIN_SERVICE`], account [`keychain_account`].
//!
//! ```toml
//! [[profile]]
//! name = "example"                     # unique; also the keychain account suffix
//! base_url = "https://api.example.com/v1"
//! wire_api = "responses"               # "chat" (/chat/completions) | "responses" (/responses)
//!                                      # | "messages" (/messages, Anthropic-style)
//!
//! [[profile.model]]                    # zero or more; every key but `id` is optional
//! id = "model-a"
//! forced_tool = true                   # honours a forced `tool_choice`
//! context_window = 128000              # tokens; second trigger for rolling a segment
//! cache_min_prefix = 1024              # shortest cacheable prefix, tokens
//! cache_breakpoints = 0                # 0 = automatic prefix caching, 1..=4 = explicit
//! price_in = 1.0                       # any currency per any token count: only the
//! price_in_cached = 0.1                #   ratios to price_in are used
//! price_in_cache_write = 1.25          # messages only: writing a cache entry
//! price_out = 4.0
//!
//! [roles]                              # absent = no chat model: the app runs as a library
//! chat = { profile = "example", model = "model-a" }
//! tts = { profile = "example", model = "tts-1" }                          # optional
//! persona = { profile = "example", model = "model-a", reasoning = "high" }  # optional
//! ```
//!
//! Absent optional keys stay absent when written back (TOML has no null).
//!
//! The offline persona generator reads the same file (only `roles.persona` and the
//! profile it names), so this doc comment is the shape both sides agree on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Keychain service every profile key is stored under.
pub const KEYCHAIN_SERVICE: &str = "dramatis";

/// Keychain account for one profile's key.
pub fn keychain_account(profile: &str) -> String {
    format!("profile.{profile}")
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct Endpoints {
    #[serde(rename = "profile", default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub roles: Option<Roles>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// API root including the version segment (`…/v1`); `GET {base_url}/models` lists
    /// its models on every wire.
    pub base_url: String,
    pub wire_api: WireApi,
    #[serde(rename = "model", default)]
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    /// `POST {base_url}/chat/completions`
    Chat,
    /// `POST {base_url}/responses`
    Responses,
    /// `POST {base_url}/messages`, Anthropic-style: `x-api-key` and `anthropic-version`
    /// headers, explicit `cache_control` breakpoints, `output_config.effort` for reasoning.
    Messages,
}

impl WireApi {
    /// Whether a forced `tool_choice` may be sent at all. On the Anthropic-style wire it is
    /// never sent: current models reject it, and a `tool_choice` change drops the cached
    /// message history, so the wrap-up turn asks for the tool in its text instead.
    pub fn may_force_tool(self) -> bool {
        !matches!(self, WireApi::Messages)
    }
}

/// What is known about one model on one profile. Unknown fields fall back to parameters
/// (`cost.c`, `cost.o`) or to the conservative behaviour (no forced tool: wrap-up skipped).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub forced_tool: Option<bool>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub cache_min_prefix: Option<u32>,
    #[serde(default)]
    pub cache_breakpoints: Option<u8>,
    #[serde(default)]
    pub price_in: Option<f64>,
    #[serde(default)]
    pub price_in_cached: Option<f64>,
    /// Writing a cache entry (Anthropic-style wire); absent: the `cost.w` parameter.
    #[serde(default)]
    pub price_in_cache_write: Option<f64>,
    #[serde(default)]
    pub price_out: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    /// Every runtime call: direct, group, ask, openings, interjections, wrap-up, host.
    pub chat: Role,
    /// Read-aloud (`/audio/speech`). Absent: no play button.
    #[serde(default)]
    pub tts: Option<Role>,
    /// Maintainer-only offline persona generation. The app never reads it.
    #[serde(default)]
    pub persona: Option<Role>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub profile: String,
    pub model: String,
    /// Passed as the endpoint's reasoning parameter; one value per session, never per call.
    /// Absent: send none and let the endpoint default.
    #[serde(default)]
    pub reasoning: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("endpoints file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("endpoints file: {0}")]
    Write(#[from] toml::ser::Error),
    #[error("profile `{0}` is defined twice")]
    DuplicateProfile(String),
    #[error("role `{role}` names profile `{profile}`, which is not defined")]
    UnknownProfile { role: &'static str, profile: String },
}

impl Endpoints {
    /// Parse and check references. Model ids a role names need not be listed on the
    /// profile: a model name may be typed by hand when `/models` is unreachable.
    pub fn from_toml(text: &str) -> Result<Self, Error> {
        let parsed: Self = toml::from_str(text)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn to_toml(&self) -> Result<String, Error> {
        Ok(toml::to_string(self)?)
    }

    pub fn validate(&self) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        for p in &self.profiles {
            if !seen.insert(p.name.as_str()) {
                return Err(Error::DuplicateProfile(p.name.clone()));
            }
        }
        if let Some(roles) = &self.roles {
            let named = [
                ("chat", Some(&roles.chat)),
                ("tts", roles.tts.as_ref()),
                ("persona", roles.persona.as_ref()),
            ];
            for (role, r) in named {
                if let Some(r) = r
                    && !seen.contains(r.profile.as_str())
                {
                    return Err(Error::UnknownProfile {
                        role,
                        profile: r.profile.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

/// First-run presets shipped in code. Models are left empty: they come from
/// `GET {base_url}/models` when the user adds the profile.
pub fn presets() -> Vec<Profile> {
    vec![
        Profile {
            name: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            wire_api: WireApi::Chat,
            models: Vec::new(),
        },
        Profile {
            name: "openai-compatible".into(),
            base_url: "https://api.openai.com/v1".into(),
            wire_api: WireApi::Chat,
            models: Vec::new(),
        },
        Profile {
            name: "anthropic-compatible".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            wire_api: WireApi::Messages,
            models: Vec::new(),
        },
        // Loopback only: a local server must never be reachable from the network.
        Profile {
            name: "local".into(),
            base_url: "http://127.0.0.1:8080/v1".into(),
            wire_api: WireApi::Chat,
            models: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same keys, tables and value kinds as the maintainer's file; neutral names.
    const MAINTAINER_SHAPE: &str = r#"
# comment lines are allowed
[[profile]]
name = "primary"
base_url = "https://llm.example.invalid/v1"
wire_api = "responses"

[[profile.model]]
id = "model-a"
forced_tool = true
cache_breakpoints = 0              # automatic prefix caching

[roles]
chat = { profile = "primary", model = "model-a" }
persona = { profile = "primary", model = "model-a", reasoning = "high" }
"#;

    #[test]
    fn parses_the_maintainer_shape() {
        let e = Endpoints::from_toml(MAINTAINER_SHAPE).unwrap();
        assert_eq!(e.profiles.len(), 1);
        let p = &e.profiles[0];
        assert_eq!(p.wire_api, WireApi::Responses);
        assert_eq!(
            p.models,
            vec![Model {
                id: "model-a".into(),
                forced_tool: Some(true),
                cache_breakpoints: Some(0),
                ..Model::default()
            }]
        );
        let roles = e.roles.as_ref().unwrap();
        assert_eq!(roles.chat.model, "model-a");
        assert_eq!(roles.chat.reasoning, None);
        assert_eq!(
            roles.persona.as_ref().unwrap().reasoning.as_deref(),
            Some("high")
        );
        assert!(roles.tts.is_none());
    }

    #[test]
    fn parses_an_anthropic_style_profile() {
        let e = Endpoints::from_toml(
            r#"
[[profile]]
name = "second"
base_url = "https://llm2.example.invalid/v1"
wire_api = "messages"

[[profile.model]]
id = "model-b"
cache_breakpoints = 3
price_in = 4.0
price_in_cached = 0.2
price_in_cache_write = 5.0
price_out = 20.0

[roles]
chat = { profile = "second", model = "model-b" }
persona = { profile = "second", model = "model-b", reasoning = "high" }
"#,
        )
        .unwrap();
        let p = &e.profiles[0];
        assert_eq!(p.wire_api, WireApi::Messages);
        assert!(!p.wire_api.may_force_tool());
        assert_eq!(p.models[0].price_in_cache_write, Some(5.0));
        assert_eq!(Endpoints::from_toml(&e.to_toml().unwrap()).unwrap(), e);
    }

    #[test]
    fn round_trips() {
        let e = Endpoints::from_toml(MAINTAINER_SHAPE).unwrap();
        let text = e.to_toml().unwrap();
        assert!(
            text.contains("[[profile]]") && text.contains("[[profile.model]]"),
            "{text}"
        );
        assert_eq!(Endpoints::from_toml(&text).unwrap(), e);

        let full = Endpoints {
            profiles: presets()
                .into_iter()
                .map(|mut p| {
                    p.models.push(Model {
                        id: "m".into(),
                        forced_tool: Some(false),
                        context_window: Some(1000),
                        cache_min_prefix: Some(10),
                        cache_breakpoints: Some(4),
                        price_in: Some(1.0),
                        price_in_cached: Some(0.25),
                        price_in_cache_write: Some(1.25),
                        price_out: Some(4.0),
                    });
                    p
                })
                .collect(),
            roles: Some(Roles {
                chat: Role {
                    profile: "local".into(),
                    model: "m".into(),
                    reasoning: None,
                },
                tts: Some(Role {
                    profile: "deepseek".into(),
                    model: "m".into(),
                    reasoning: None,
                }),
                persona: None,
            }),
        };
        assert_eq!(
            Endpoints::from_toml(&full.to_toml().unwrap()).unwrap(),
            full
        );
    }

    #[test]
    fn empty_file_is_library_form() {
        let e = Endpoints::from_toml("").unwrap();
        assert!(e.profiles.is_empty() && e.roles.is_none());
    }

    #[test]
    fn rejects_bad_references_and_unknown_keys() {
        let dup = "[[profile]]\nname='a'\nbase_url='x'\nwire_api='chat'\n".repeat(2);
        assert!(matches!(
            Endpoints::from_toml(&dup),
            Err(Error::DuplicateProfile(_))
        ));
        let dangling = "[roles]\nchat = { profile = 'nope', model = 'm' }\n";
        assert!(matches!(
            Endpoints::from_toml(dangling),
            Err(Error::UnknownProfile { role: "chat", .. })
        ));
        // A key in the file must never be silently accepted and ignored.
        let secret = "[[profile]]\nname='a'\nbase_url='x'\nwire_api='chat'\napi_key='k'\n";
        assert!(matches!(Endpoints::from_toml(secret), Err(Error::Parse(_))));
        let wire = "[[profile]]\nname='a'\nbase_url='x'\nwire_api='grpc'\n";
        assert!(Endpoints::from_toml(wire).is_err());
    }

    #[test]
    fn presets_are_valid_and_local_is_loopback() {
        let e = Endpoints {
            profiles: presets(),
            roles: None,
        };
        e.validate().unwrap();
        assert!(
            e.profile("local")
                .unwrap()
                .base_url
                .starts_with("http://127.0.0.1:")
        );
        assert_eq!(keychain_account("local"), "profile.local");
    }
}
