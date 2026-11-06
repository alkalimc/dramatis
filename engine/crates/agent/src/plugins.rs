//! The engine's side of plugins (`plugin` crate, `docs/plugins.md`): the hook points the
//! flows call. Nothing here can fail a turn. A plugin that errors, times out or answers
//! nonsense contributes nothing, and the turn goes on as if it were not installed.
//!
//! Why the `prompt` hook writes into the turn's own entry and nowhere else: a segment's
//! prefix (blocks A, B, C and the tool list) is what the endpoint caches, and it is
//! byte-identical for the segment's life. A plugin's lines change from turn to turn, so
//! they join the harness lines of the entry being appended, after the cached part.
//! Plugin tools are the one thing that reaches the tool list; a change in which are
//! offered rolls the segment over like any other change of that list.

use std::sync::Arc;

use api::tools::ToolSpec;
use api::types::Author;
use api::views::Message;
use plugin::{Audience, Fail, Host};
use serde_json::{Value, json};
use world::{ChannelId, channel};

use crate::agent::Agent;
use crate::error::Result;
use crate::session::{LogKind, Turn};

/// Run blocking work on its own thread so the executor's workers never wait on a pipe.
/// `None`: the thread could not be started, or the work panicked.
pub(crate) async fn off<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .spawn(move || {
            let _ = tx.send(work());
        })
        .ok()?;
    rx.await.ok()
}

fn audience(kind: LogKind) -> Audience {
    match kind {
        LogKind::Host => Audience::Host,
        LogKind::Person => Audience::Persons,
    }
}

impl Agent {
    /// Wire the plugin host in; called once, before the agent is shared.
    pub fn with_plugins(mut self, host: Arc<Host>) -> Self {
        self.plugins = Some(host);
        self
    }

    /// Hook `tool`: the specs of enabled plugins' tools for a session of this kind, to
    /// follow the built-in list.
    pub(crate) fn plugin_tools(&self, kind: LogKind) -> Vec<ToolSpec> {
        let Some(host) = &self.plugins else {
            return Vec::new();
        };
        host.tools(audience(kind))
            .into_iter()
            .map(|t| ToolSpec {
                name: t.name,
                description: t.description,
                parameters: t.parameters,
            })
            .collect()
    }

    /// What a plugin is told about where it is called: the channel and who is in it.
    fn plugin_context(&self, ch: ChannelId) -> Value {
        let conn = self.db();
        match channel::get(&conn, ch) {
            Ok(c) => {
                let kind = if c.is_host_channel() {
                    "host"
                } else if c.kind == world::ChannelKind::Group {
                    "group"
                } else {
                    "direct"
                };
                let people = c.persons();
                let persons: Vec<&str> = people.iter().map(|p| p.as_str()).collect();
                json!({"channel": {"id": ch.0, "kind": kind}, "persons": persons})
            }
            Err(_) => json!({"channel": {"id": ch.0}}),
        }
    }

    /// Hook `prompt`: lines for the entry of a turn the user started.
    pub(crate) async fn plugin_lines(&self, ch: ChannelId, user_text: &str) -> Vec<String> {
        let Some(host) = self.plugins.clone() else {
            return Vec::new();
        };
        let mut params = self.plugin_context(ch);
        params["text"] = Value::String(plugin::host::clip(user_text, plugin::host::MESSAGE_CHARS));
        off(move || host.prompt_lines(&params))
            .await
            .unwrap_or_default()
    }

    /// Hook `tool_result`: the result as the model will read it.
    pub(crate) async fn plugin_result(
        &self,
        turn: &Turn,
        name: &str,
        args: &str,
        output: String,
    ) -> String {
        let Some(host) = self.plugins.clone() else {
            return output;
        };
        let context = self.plugin_context(turn.channel);
        let (name, args, kept) = (name.to_owned(), args.to_owned(), output.clone());
        off(move || host.tool_result(&name, &args, output, &context))
            .await
            .unwrap_or(kept)
    }

    /// Run a tool a plugin added. The text is what the model reads back, in the corpus's
    /// own words for "not available" and "refused".
    pub(crate) async fn plugin_dispatch(
        &self,
        turn: &Turn,
        name: &str,
        arguments: &str,
        names: &crate::text::Names,
    ) -> Result<String> {
        let w = &self.corpus.wording;
        let unavailable = || {
            w.fill(
                "harness.error.unavailable",
                names,
                &[("detail", ""), ("id", "")],
            )
        };
        let Some(host) = self.plugins.clone().filter(|_| !turn.wrapup) else {
            return Ok(unavailable());
        };
        let context = self.plugin_context(turn.channel);
        let (name, arguments) = (name.to_owned(), arguments.to_owned());
        let out = off(move || host.call_tool(&name, &arguments, &context)).await;
        Ok(match out {
            Some(Ok(text)) => text,
            Some(Err(Fail::Plugin(why))) => w.fill(
                "harness.error.refused",
                names,
                &[("detail", &why), ("id", &why)],
            ),
            _ => unavailable(),
        })
    }

    /// Hook `message`: a message was posted. Queued; never waits.
    pub(crate) fn plugin_message(&self, m: &Message) {
        let Some(host) = &self.plugins else { return };
        let author = match &m.author {
            Author::User => json!({"kind": "user"}),
            Author::Host => json!({"kind": "host"}),
            Author::Person { id } => json!({"kind": "person", "id": id.0.as_str()}),
        };
        host.message(json!({
            "id": m.id.0,
            "channel": m.channel.0,
            "author": author,
            "text": plugin::host::clip(&m.text, plugin::host::MESSAGE_CHARS),
            "service": m.service,
            "at": m.at.0,
        }));
    }
}
