//! The plugin host: discovery, the enabled list, process supervision and the hooks.
//!
//! Every call into a plugin is blocking, bounded by a timeout and isolated: a failure of
//! any kind is returned to the caller as "no contribution", never as an error that could
//! stop a turn. A plugin that fails [`FAILURES_MAX`] times in a row is stopped until the
//! user turns it off and on again.
//!
//! Callers on an async executor reach these through a worker thread (the agent's
//! `plugins` module); nothing here waits on a lock for longer than one call.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::manifest::{Audience, Hook, Manifest, TOOL_SEP};
use crate::process::{Fail, Proc};

/// The file in the plugins folder that lists the enabled plugins.
pub const ENABLED_FILE: &str = "enabled.toml";
/// Consecutive failures after which a plugin is stopped.
pub const FAILURES_MAX: u32 = 3;
/// How long a read of the plugins folder is reused.
const REFRESH: Duration = Duration::from_millis(1000);
/// What one hook may spend on all plugins together, per turn.
const BUDGET: Duration = Duration::from_millis(4000);
/// Pending `message` notifications; more are dropped.
const EVENTS_QUEUE: usize = 256;

/// Lines one plugin may add to a turn, the longest line, and all plugins' lines together.
pub const LINES_MAX: usize = 5;
pub const LINE_CHARS: usize = 400;
pub const LINES_TOTAL_CHARS: usize = 1500;
/// Longest text a tool or a post-processor returns, in characters.
pub const OUTPUT_CHARS: usize = 8000;
/// Longest message text sent to a plugin, in characters.
pub const MESSAGE_CHARS: usize = 8000;

/// A tool a plugin adds, as the model is offered it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDecl {
    /// `<plugin>__<tool>`.
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// One folder of the plugins folder, as the settings list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The folder's name.
    pub name: String,
    pub version: String,
    pub description: String,
    pub hooks: Vec<String>,
    /// The tools the plugin adds, by the name the model sees.
    pub tools: Vec<String>,
    pub enabled: bool,
    /// Why the manifest is unusable.
    pub problem: Option<String>,
    /// The last failure of the running plugin, or that it was stopped.
    pub status: Option<String>,
}

/// What a failed set-enabled says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unknown(String),
    Invalid(String),
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unknown(n) => write!(f, "no plugin `{n}`"),
            Error::Invalid(e) => write!(f, "the manifest is invalid: {e}"),
            Error::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
struct Health {
    failures: u32,
    stopped: bool,
    last: Option<String>,
}

#[derive(Default)]
struct Slot {
    proc: Mutex<Option<Proc>>,
    health: Mutex<Health>,
}

struct Found {
    folder: String,
    manifest: Result<Arc<Manifest>, String>,
}

#[derive(Default)]
struct Snapshot {
    at: Option<Instant>,
    found: Vec<Found>,
    enabled: BTreeSet<String>,
}

struct Inner {
    dir: PathBuf,
    logs: Option<PathBuf>,
    version: String,
    snapshot: Mutex<Snapshot>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

/// The plugins of one home. Cheap to share behind an `Arc`.
pub struct Host {
    inner: Arc<Inner>,
    events: OnceLock<SyncSender<Value>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Cut to `max` characters.
pub fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// One line of text from a plugin: control characters become spaces, the ends are trimmed.
fn one_line(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    clip(flat.trim(), max)
}

impl Host {
    /// The plugins in `dir` (`$DRAMATIS_HOME/plugins`, which may not exist). `logs` is
    /// where each plugin's stderr goes, `plugin-<name>.log`. `version` is the app's.
    pub fn new(dir: PathBuf, logs: Option<PathBuf>, version: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                dir,
                logs,
                version: version.to_owned(),
                snapshot: Mutex::default(),
                slots: Mutex::default(),
            }),
            events: OnceLock::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    // ---- discovery and settings ----

    /// Every folder of the plugins folder with its state, read afresh.
    pub fn list(&self) -> Vec<Listing> {
        self.inner.refresh(true);
        let snap = lock(&self.inner.snapshot);
        snap.found
            .iter()
            .map(|f| {
                let enabled = snap.enabled.contains(&f.folder);
                let status = lock(&self.inner.slot(&f.folder).health).last.clone();
                match &f.manifest {
                    Ok(m) => Listing {
                        name: f.folder.clone(),
                        version: m.version.clone(),
                        description: m.description.clone(),
                        hooks: m.hooks.iter().map(|h| h.name().to_owned()).collect(),
                        tools: m.tools.iter().map(|t| m.full_name(t)).collect(),
                        enabled,
                        problem: None,
                        status: enabled.then_some(status).flatten(),
                    },
                    Err(e) => Listing {
                        name: f.folder.clone(),
                        version: String::new(),
                        description: String::new(),
                        hooks: Vec::new(),
                        tools: Vec::new(),
                        enabled: false,
                        problem: Some(e.clone()),
                        status: None,
                    },
                }
            })
            .collect()
    }

    /// Turn a plugin on or off. Turning it off ends its process; turning it on clears
    /// any earlier failures.
    pub fn set_enabled(&self, name: &str, on: bool) -> Result<(), Error> {
        self.inner.refresh(true);
        let mut enabled = {
            let snap = lock(&self.inner.snapshot);
            match snap.found.iter().find(|f| f.folder == name) {
                None => return Err(Error::Unknown(name.into())),
                Some(Found {
                    manifest: Err(e), ..
                }) if on => return Err(Error::Invalid(e.clone())),
                Some(_) => {}
            }
            snap.enabled.clone()
        };
        if on {
            enabled.insert(name.to_owned());
        } else {
            enabled.remove(name);
        }
        self.inner.write_enabled(&enabled)?;
        let slot = self.inner.slot(name);
        *lock(&slot.proc) = None;
        *lock(&slot.health) = Health::default();
        self.inner.refresh(true);
        Ok(())
    }

    /// The tools of enabled plugins offered to `session` (`Host` or `Persons`), in a
    /// fixed order: by plugin name, then as declared. Starts no process.
    pub fn tools(&self, session: Audience) -> Vec<ToolDecl> {
        self.inner
            .enabled_manifests()
            .iter()
            .flat_map(|m| {
                m.tools
                    .iter()
                    .filter(move |t| t.audience.includes(session))
                    .map(move |t| ToolDecl {
                        name: m.full_name(t),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    })
            })
            .collect()
    }

    /// Whether `name` has the shape of a plugin tool's (`<plugin>__<tool>`).
    pub fn is_tool_name(name: &str) -> bool {
        name.split_once(TOOL_SEP)
            .is_some_and(|(p, t)| !p.is_empty() && !t.is_empty())
    }

    // ---- hooks ----

    /// The `prompt` hook: lines to add to this turn's harness entry. `params` is the
    /// turn's description. Lines are single, short and few; the rest is dropped.
    pub fn prompt_lines(&self, params: &Value) -> Vec<String> {
        let started = Instant::now();
        let mut all = Vec::new();
        let mut room = LINES_TOTAL_CHARS;
        for m in self.inner.hooked(Hook::Prompt) {
            if started.elapsed() > BUDGET || room == 0 {
                break;
            }
            let Ok(reply) = self.inner.request(&m, "prompt", params.clone()) else {
                continue;
            };
            let Some(lines) = reply.get("lines").and_then(Value::as_array) else {
                continue;
            };
            for line in lines.iter().filter_map(Value::as_str).take(LINES_MAX) {
                let line = one_line(line, LINE_CHARS.min(room));
                if line.is_empty() {
                    continue;
                }
                room = room.saturating_sub(line.chars().count());
                all.push(line);
            }
        }
        all
    }

    /// The `tool_result` hook: each plugin in turn may replace the result. A plugin that
    /// fails, or answers without an `output`, leaves it as it was.
    pub fn tool_result(
        &self,
        tool: &str,
        arguments: &str,
        output: String,
        context: &Value,
    ) -> String {
        let started = Instant::now();
        let arguments = serde_json::from_str::<Value>(arguments)
            .unwrap_or_else(|_| Value::String(clip(arguments, OUTPUT_CHARS)));
        let mut output = output;
        for m in self.inner.hooked(Hook::ToolResult) {
            if started.elapsed() > BUDGET {
                break;
            }
            let params = json!({
                "tool": tool,
                "arguments": arguments,
                "output": clip(&output, OUTPUT_CHARS),
                "context": context,
            });
            if let Ok(reply) = self.inner.request(&m, "tool_result", params)
                && let Some(new) = reply.get("output").and_then(Value::as_str)
            {
                output = clip(new, OUTPUT_CHARS);
            }
        }
        output
    }

    /// Call a tool a plugin added, by the name the model sees. `arguments` is the model's
    /// JSON. Fails with `Off` when the plugin is not enabled or does not have the tool.
    pub fn call_tool(&self, name: &str, arguments: &str, context: &Value) -> Result<String, Fail> {
        let (plugin, tool) = name.split_once(TOOL_SEP).ok_or(Fail::Off)?;
        let m = self
            .inner
            .enabled_manifests()
            .into_iter()
            .find(|m| m.name == plugin && m.tools.iter().any(|t| t.name == tool))
            .ok_or(Fail::Off)?;
        let arguments = serde_json::from_str::<Value>(arguments)
            .map_err(|e| Fail::BadReply(format!("arguments: {e}")))?;
        let params = json!({"tool": tool, "arguments": arguments, "context": context});
        let reply = self.inner.request(&m, "tool", params)?;
        match reply.get("output").and_then(Value::as_str) {
            Some(s) => Ok(clip(s, OUTPUT_CHARS)),
            None => Err(Fail::BadReply("the reply has no `output` text".into())),
        }
    }

    /// The `message` hook: tell plugins a message was posted. Never waits: the event is
    /// queued for a worker thread and dropped when the queue is full.
    pub fn message(&self, event: Value) {
        if self.inner.hooked(Hook::Message).is_empty() {
            return;
        }
        let tx = self.events.get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Value>(EVENTS_QUEUE);
            let inner = self.inner.clone();
            thread::spawn(move || {
                for event in rx {
                    for m in inner.hooked(Hook::Message) {
                        inner.notify(&m, "message", event.clone());
                    }
                }
            });
            tx
        });
        let _ = tx.try_send(event);
    }
}

impl Inner {
    fn slot(&self, name: &str) -> Arc<Slot> {
        lock(&self.slots)
            .entry(name.to_owned())
            .or_default()
            .clone()
    }

    fn read_enabled(&self) -> BTreeSet<String> {
        std::fs::read_to_string(self.dir.join(ENABLED_FILE))
            .ok()
            .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
            .and_then(|t| {
                t.get("enabled")?.as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    fn write_enabled(&self, enabled: &BTreeSet<String>) -> Result<(), Error> {
        let io = |e: std::io::Error| Error::Io(e.to_string());
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        let list: Vec<&str> = enabled.iter().map(String::as_str).collect();
        let text = format!(
            "enabled = {}\n",
            serde_json::to_string(&list).expect("names serialise")
        );
        let tmp = self
            .dir
            .join(format!("{ENABLED_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, self.dir.join(ENABLED_FILE)).map_err(io)
    }

    /// Read the folder again if the last read is old (or `force`). A plugin that is no
    /// longer enabled, or whose folder is gone, loses its process.
    fn refresh(&self, force: bool) {
        let mut snap = lock(&self.snapshot);
        if !force && snap.at.is_some_and(|t| t.elapsed() < REFRESH) {
            return;
        }
        let mut found = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let path = e.path();
                let Some(folder) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !path.is_dir() || folder.starts_with('.') {
                    continue;
                }
                found.push(Found {
                    folder: folder.to_owned(),
                    manifest: Manifest::load(&path).map(Arc::new),
                });
            }
        }
        found.sort_by(|a, b| a.folder.cmp(&b.folder));
        let enabled = self.read_enabled();
        for (name, slot) in lock(&self.slots).iter() {
            let live = enabled.contains(name)
                && found
                    .iter()
                    .any(|f| &f.folder == name && f.manifest.is_ok());
            if !live {
                if let Ok(mut p) = slot.proc.try_lock() {
                    *p = None;
                }
                *lock(&slot.health) = Health::default();
            }
        }
        *snap = Snapshot {
            at: Some(Instant::now()),
            found,
            enabled,
        };
    }

    fn enabled_manifests(&self) -> Vec<Arc<Manifest>> {
        self.refresh(false);
        let snap = lock(&self.snapshot);
        snap.found
            .iter()
            .filter(|f| snap.enabled.contains(&f.folder))
            .filter_map(|f| f.manifest.as_ref().ok().cloned())
            .collect()
    }

    /// Enabled, running-capable plugins that take `hook`.
    fn hooked(&self, hook: Hook) -> Vec<Arc<Manifest>> {
        self.enabled_manifests()
            .into_iter()
            .filter(|m| m.takes(hook) && !lock(&self.slot(&m.name).health).stopped)
            .collect()
    }

    fn with_proc<T>(
        &self,
        m: &Manifest,
        f: impl FnOnce(&mut Proc) -> Result<T, Fail>,
    ) -> Result<T, Fail> {
        let slot = self.slot(&m.name);
        if lock(&slot.health).stopped {
            return Err(Fail::Stopped);
        }
        let mut guard = lock(&slot.proc);
        let out = (|| {
            if guard.is_none() {
                let log = self
                    .logs
                    .as_ref()
                    .map(|d| d.join(format!("plugin-{}.log", m.name)));
                *guard = Some(Proc::start(
                    m,
                    &self.dir.join(&m.name),
                    log.as_deref(),
                    &self.version,
                )?);
            }
            f(guard.as_mut().expect("started above"))
        })();
        let mut health = lock(&slot.health);
        match &out {
            Ok(_) => health.failures = 0,
            // The plugin answered, with an error of its own: it is alive.
            Err(Fail::Plugin(_)) => health.failures = 0,
            Err(e) => {
                *guard = None;
                health.failures += 1;
                health.last = Some(e.to_string());
                if health.failures >= FAILURES_MAX {
                    health.stopped = true;
                    health.last = Some(format!("stopped after repeated failures: {e}"));
                }
            }
        }
        out
    }

    fn request(&self, m: &Manifest, method: &str, params: Value) -> Result<Value, Fail> {
        let timeout = m.timeout;
        self.with_proc(m, |p| p.call(method, params, timeout))
    }

    fn notify(&self, m: &Manifest, method: &str, params: Value) {
        let _ = self.with_proc(m, |p| p.notify(method, params));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_flattens_and_cuts() {
        assert_eq!(one_line("  a\nb\tc \u{7} ", 10), "a b c");
        assert_eq!(one_line("abcdef", 3), "abc");
        assert_eq!(one_line("\n\n", 3), "");
    }

    #[test]
    fn tool_names() {
        assert!(Host::is_tool_name("dice__roll"));
        assert!(!Host::is_tool_name("search"));
        assert!(!Host::is_tool_name("__x"));
    }
}
