//! `plugin.toml`: what a plugin is, which hooks it takes and which tools it adds.
//!
//! Everything the model could be shown (tool names, descriptions, schemas) is declared
//! here, statically, so the app can list a plugin's reach before the user enables it and
//! so the tool list that opens a conversation never depends on a running process.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// The protocol version this build speaks.
pub const PROTOCOL: u32 = 1;

/// Longest name of a plugin, and of one of its tools.
const NAME_MAX: usize = 32;
/// Most tools one plugin may add.
pub const TOOLS_MAX: usize = 8;
/// Longest tool description, in characters.
const DESCRIPTION_MAX: usize = 1000;
/// Longest manifest file read.
const FILE_MAX: u64 = 64 * 1024;
/// Per-call timeout when the manifest names none, and the most it may name.
const TIMEOUT_DEFAULT_MS: u64 = 1000;
const TIMEOUT_MAX_MS: u64 = 5000;
/// Separates the plugin's name from the tool's in the name the model sees.
pub const TOOL_SEP: &str = "__";

/// A point in the turn where a plugin is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hook {
    /// Lines for the per-turn harness entry of a turn the user starts.
    Prompt,
    /// A tool's result, before the model reads it.
    ToolResult,
    /// A message was posted, by the user or by anyone answering. Observe only.
    Message,
}

impl Hook {
    pub fn name(self) -> &'static str {
        match self {
            Hook::Prompt => "prompt",
            Hook::ToolResult => "tool_result",
            Hook::Message => "message",
        }
    }
}

/// Which sessions are offered a plugin's tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    /// The host's session and every person's.
    #[default]
    All,
    /// The host's session only.
    Host,
    /// People's sessions only.
    Persons,
}

impl Audience {
    /// Whether a session of `want` (`Host` or `Persons`) is offered a tool meant for `self`.
    pub fn includes(self, want: Audience) -> bool {
        self == Audience::All || self == want
    }
}

/// A tool the plugin adds.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The name inside the plugin; the model sees `<plugin>__<name>`.
    pub name: String,
    pub description: String,
    /// A JSON Schema of the arguments, an object schema.
    pub parameters: Value,
    pub audience: Audience,
}

/// A validated manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    /// The program and its arguments, run with the plugin's folder as working directory.
    pub command: Vec<String>,
    pub hooks: Vec<Hook>,
    pub timeout: Duration,
    pub tools: Vec<Tool>,
}

#[derive(Deserialize)]
struct RawTool {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters: Option<toml::Table>,
    #[serde(default)]
    audience: Audience,
}

#[derive(Deserialize)]
struct Raw {
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    protocol: u32,
    command: Vec<String>,
    #[serde(default)]
    hooks: Vec<Hook>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    tools: Vec<RawTool>,
}

/// Lower-case letters and digits, and `-` inside a plugin's name.
fn plugin_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= NAME_MAX
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Lower-case letters, digits and `_` inside a tool's name; no `__`, which is the separator.
fn tool_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= NAME_MAX
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.contains(TOOL_SEP)
        && !s.ends_with('_')
}

impl Manifest {
    /// Read and validate `<dir>/plugin.toml`. The folder's name must be the plugin's name.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let file = dir.join("plugin.toml");
        let meta = std::fs::metadata(&file).map_err(|e| format!("plugin.toml: {e}"))?;
        if meta.len() > FILE_MAX {
            return Err("plugin.toml is too large".into());
        }
        let text = std::fs::read_to_string(&file).map_err(|e| format!("plugin.toml: {e}"))?;
        let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        Self::parse(&text, folder)
    }

    /// Validate manifest text found in a folder called `folder`.
    pub fn parse(text: &str, folder: &str) -> Result<Self, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| format!("plugin.toml: {e}"))?;
        if !plugin_name_ok(&raw.name) {
            return Err(format!(
                "name `{}` must be 1 to {NAME_MAX} characters: a-z, 0-9 and `-`",
                raw.name
            ));
        }
        if raw.name != folder {
            return Err(format!(
                "name `{}` must equal the folder name `{folder}`",
                raw.name
            ));
        }
        if raw.protocol != PROTOCOL {
            return Err(format!(
                "protocol {} is not supported (this build speaks {PROTOCOL})",
                raw.protocol
            ));
        }
        if raw.command.is_empty() || raw.command.iter().any(|a| a.is_empty() || a.len() > 512) {
            return Err("command must be a non-empty list of non-empty strings".into());
        }
        if raw.tools.len() > TOOLS_MAX {
            return Err(format!("at most {TOOLS_MAX} tools"));
        }
        let mut hooks = raw.hooks;
        hooks.sort();
        hooks.dedup();
        let mut tools = Vec::new();
        for t in raw.tools {
            if !tool_name_ok(&t.name) {
                return Err(format!(
                    "tool name `{}` must be 1 to {NAME_MAX} characters: a-z, 0-9 and `_`, starting with a letter, no `{TOOL_SEP}`",
                    t.name
                ));
            }
            if tools.iter().any(|x: &Tool| x.name == t.name) {
                return Err(format!("tool `{}` is declared twice", t.name));
            }
            if t.description.trim().is_empty() || t.description.chars().count() > DESCRIPTION_MAX {
                return Err(format!(
                    "tool `{}` needs a description of at most {DESCRIPTION_MAX} characters",
                    t.name
                ));
            }
            let mut parameters = match t.parameters {
                Some(table) => serde_json::to_value(table).map_err(|e| e.to_string())?,
                None => serde_json::json!({}),
            };
            let obj = parameters
                .as_object_mut()
                .ok_or("parameters must be a table")?;
            match obj.get("type").and_then(Value::as_str) {
                None | Some("object") => {}
                Some(_) => {
                    return Err(format!(
                        "tool `{}`: parameters must be an object schema",
                        t.name
                    ));
                }
            }
            obj.insert("type".into(), "object".into());
            obj.entry("properties")
                .or_insert_with(|| serde_json::json!({}));
            tools.push(Tool {
                name: t.name,
                description: t.description,
                parameters,
                audience: t.audience,
            });
        }
        let timeout_ms = raw
            .timeout_ms
            .unwrap_or(TIMEOUT_DEFAULT_MS)
            .clamp(50, TIMEOUT_MAX_MS);
        Ok(Self {
            name: raw.name,
            version: raw.version,
            description: raw.description,
            command: raw.command,
            hooks,
            timeout: Duration::from_millis(timeout_ms),
            tools,
        })
    }

    pub fn takes(&self, hook: Hook) -> bool {
        self.hooks.contains(&hook)
    }

    /// Whether the plugin is ever consulted: it takes a hook or adds a tool.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty() && self.tools.is_empty()
    }

    /// The tool as the model names it.
    pub fn full_name(&self, tool: &Tool) -> String {
        format!("{}{TOOL_SEP}{}", self.name, tool.name)
    }

    /// The program to run. A first word with a path separator is a file in the plugin's
    /// folder (never outside it); any other is looked up on `PATH`.
    pub fn program(&self, dir: &Path) -> Result<PathBuf, String> {
        let first = &self.command[0];
        if !first.contains('/') && !first.contains('\\') {
            return Ok(PathBuf::from(first));
        }
        let rel = Path::new(first);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            return Err("command must stay inside the plugin folder".into());
        }
        Ok(dir.join(rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
name = "dice"
version = "0.1.0"
protocol = 1
command = ["python3", "dice.py"]
hooks = ["prompt", "message", "prompt"]
timeout_ms = 99999

[[tools]]
name = "roll"
description = "Roll dice."
[tools.parameters.properties.sides]
type = "integer"
"#;

    #[test]
    fn parses_and_clamps() {
        let m = Manifest::parse(GOOD, "dice").unwrap();
        assert_eq!(m.hooks, vec![Hook::Prompt, Hook::Message]);
        assert_eq!(m.timeout, Duration::from_millis(TIMEOUT_MAX_MS));
        assert_eq!(m.full_name(&m.tools[0]), "dice__roll");
        assert_eq!(m.tools[0].parameters["type"], "object");
        assert_eq!(
            m.tools[0].parameters["properties"]["sides"]["type"],
            "integer"
        );
    }

    #[test]
    fn rejects_bad_manifests() {
        let bad = |t: &str, folder: &str| Manifest::parse(t, folder).unwrap_err();
        assert!(bad(GOOD, "other").contains("folder"));
        assert!(bad(&GOOD.replace("protocol = 1", "protocol = 2"), "dice").contains("protocol"));
        assert!(bad(&GOOD.replace("\"roll\"", "\"a__b\""), "dice").contains("tool name"));
        assert!(
            bad(&GOOD.replace("hooks = [", "hooks = [\"nope\", "), "dice").contains("plugin.toml")
        );
        assert!(
            bad(
                &GOOD.replace("command = [\"python3\", \"dice.py\"]", "command = []"),
                "dice"
            )
            .contains("command")
        );
        assert!(bad(&GOOD.replace("name = \"dice\"", "name = \"Dice\""), "Dice").contains("name"));
    }

    #[test]
    fn program_stays_inside() {
        let dir = Path::new("/p/dice");
        let mut m = Manifest::parse(GOOD, "dice").unwrap();
        assert_eq!(m.program(dir).unwrap(), PathBuf::from("python3"));
        m.command = vec!["./run.sh".into()];
        assert_eq!(m.program(dir).unwrap(), dir.join("./run.sh"));
        m.command = vec!["../x".into()];
        assert!(m.program(dir).is_err());
        m.command = vec!["/bin/x".into()];
        assert!(m.program(dir).is_err());
    }
}
