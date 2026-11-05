//! Plugins: separate programs that add to a conversation through a small JSON protocol.
//!
//! A plugin is a folder `$DRAMATIS_HOME/plugins/<name>/` with a `plugin.toml` ([`manifest`])
//! and whatever it runs. It is off until the user turns it on (the list is
//! `plugins/enabled.toml`). Enabled, it is started on first use as a child process and
//! spoken to in lines of JSON over its stdin and stdout ([`process`]). The hooks
//! ([`host`]): lines for a turn's harness entry (`prompt`), a say over a tool's result
//! (`tool_result`), tools of its own (`tool`) and a tap on posted messages (`message`).
//!
//! The engine crates never link plugin code, and a plugin never sees the keychain: the
//! process gets a minimal environment. Every call is bounded in time and size, and any
//! failure means "no contribution". The user-facing description is `docs/plugins.md`.

pub mod host;
pub mod manifest;
pub mod process;

pub use host::{Error, Host, Listing, ToolDecl};
pub use manifest::{Audience, Hook, Manifest, PROTOCOL, TOOL_SEP};
pub use process::Fail;
