//! What reaches the terminal: the output handle, and the sink that turns the agent's
//! events into streamed, styled lines.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use agent::{Agent, Event, Sink};
use api::events::ActivityState;
use api::types::{Author, MessageId};
use api::views::{Message, ToolAction};

use super::render::{self, Marker, Streamer, Style};

/// Output plus the style it is written in. Shared by the session and the sink; writes are
/// whole strings, so the two never interleave inside one.
pub struct Term {
    out: Mutex<Box<dyn Write + Send>>,
    pub style: Style,
}

impl Term {
    pub fn new(out: Box<dyn Write + Send>, style: Style) -> Self {
        Self {
            out: Mutex::new(out),
            style,
        }
    }

    pub fn print(&self, text: &str) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        // A closed pipe ends the session at the next read; nothing useful to do here.
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    /// A dim line.
    pub fn note(&self, text: &str) {
        self.print(&format!("{}\n", self.style.dim(text)));
    }

    pub fn error(&self, text: &str) {
        self.print(&format!(
            "{}\n",
            self.style.error(&format!("error: {text}"))
        ));
    }
}

/// How an author is named on screen: the user by their name, the host by the corpus's
/// word for it, a person by their display name.
pub fn author_name(agent: &Agent, author: &Author) -> String {
    match author {
        Author::User => agent
            .names()
            .map(|n| n.user().to_owned())
            .unwrap_or_else(|_| "you".into()),
        Author::Host => agent.wording().get("host.name").to_owned(),
        Author::Person { id } => person_name(agent, &id.0),
    }
}

pub fn person_name(agent: &Agent, id: &str) -> String {
    agent
        .with_index(|ix| ix.folio().person(id))
        .ok()
        .flatten()
        .map_or_else(|| id.to_owned(), |p| p.display)
}

/// What the speaker is doing while a turn runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Working {
    Typing,
    Searching,
}

/// The status line for a running turn. One place for the wording, so it can come from the
/// corpus's manifest once the engine's activity event carries it.
pub fn working_line(who: &str, what: Working) -> String {
    match what {
        Working::Typing => format!("{who} is typing..."),
        Working::Searching => format!("{who} is looking things up..."),
    }
}

/// The status line in the corpus's own wording when it has one, else the English line.
pub fn working_text(agent: &Agent, who: &str, what: Working) -> String {
    let key = match what {
        Working::Typing => "typing.writing",
        Working::Searching => "typing.tool",
    };
    match agent.wording().get(key) {
        k if k == key => working_line(who, what),
        template => template.replace("{name}", who),
    }
}

/// One tool call as a short English line; the contents are the model's and the corpus's.
pub fn action_line(action: &ToolAction) -> String {
    match action {
        ToolAction::Search { query } => format!("searched: {query}"),
        ToolAction::GetChunk { chunk_id } => format!("read passage: {chunk_id}"),
        ToolAction::RequestJoin { person, topic } => match (person, topic) {
            (Some(p), _) => format!("asked to bring in: {}", p.name),
            (None, Some(t)) => format!("asked to bring someone in on: {t}"),
            (None, None) => "asked to bring someone in".into(),
        },
        ToolAction::Remember { text } => format!("remembered: {text}"),
        ToolAction::Report => "reported back".into(),
        ToolAction::Wrapup => "wrapped up the conversation".into(),
        ToolAction::Digest => "looked at the digest".into(),
        ToolAction::FindPeople { topic } => format!("looked for people on: {topic}"),
        ToolAction::Ask { person, question } => format!("asked {}: {question}", person.name),
        ToolAction::CreateGroup { members } => format!(
            "created a group: {}",
            members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ToolAction::SetMode { mode, .. } => format!("set mode: {mode:?}").to_lowercase(),
        ToolAction::Forget { fact } => format!("forgot memory {}", fact.0),
    }
}

/// The reply being written right now.
struct Live {
    id: MessageId,
    streamer: Streamer,
}

#[derive(Default)]
struct State {
    live: Option<Live>,
    /// The message whose name has been written, so a stream resumed after a tool line
    /// does not repeat it.
    named: Option<MessageId>,
}

/// Prints replies as they stream. Built before the agent (which takes it), bound to the
/// agent afterwards so it can look names and sources up.
pub struct Show {
    term: Arc<Term>,
    marker: Marker,
    agent: OnceLock<Weak<Agent>>,
    state: Mutex<State>,
}

impl Show {
    pub fn new(term: Arc<Term>, marker: Marker) -> Self {
        Self {
            term,
            marker,
            agent: OnceLock::new(),
            state: Mutex::new(State::default()),
        }
    }

    pub fn bind(&self, agent: &Arc<Agent>) {
        let _ = self.agent.set(Arc::downgrade(agent));
    }

    fn agent(&self) -> Option<Arc<Agent>> {
        self.agent.get().and_then(Weak::upgrade)
    }

    fn prefix(&self, agent: &Agent, author: &Author) -> String {
        format!("{}: ", self.term.style.bold(&author_name(agent, author)))
    }

    /// Close a reply that was cut off (Ctrl-C), so the next output starts on a fresh line.
    pub fn abort(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.close(&mut st);
        st.named = None;
    }

    fn close(&self, st: &mut State) {
        if let Some(mut live) = st.live.take() {
            self.term.print(&live.streamer.finish());
        }
    }

    /// The sources of a message, compactly: the header of each cited passage.
    fn sources(&self, agent: &Agent, m: &Message) -> Option<String> {
        let mut seen: Vec<String> = Vec::new();
        for c in &m.cites {
            let label = agent
                .with_index(|ix| ix.get(&c.chunk_id, 0))
                .ok()
                .flatten()
                .map(|(u, _)| {
                    if u.header.is_empty() {
                        u.title
                    } else {
                        u.header
                    }
                })
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| c.chunk_id.clone());
            if !seen.contains(&label) {
                seen.push(label);
            }
        }
        // Low confidence is the common case and says nothing a reader can act on, so it
        // gets no label here.
        if seen.is_empty() {
            return None;
        }
        let line = format!("sources: {}", seen.join("; "));
        Some(line)
    }

    /// A stored message in full: name, text, tool lines, sources.
    pub fn whole(&self, agent: &Agent, m: &Message) -> String {
        let style = self.term.style;
        let mut out = String::new();
        for a in &m.actions {
            out.push_str(&style.dim(&format!("· {}", action_line(a))));
            out.push('\n');
        }
        if !m.text.trim().is_empty() {
            out.push_str(&self.prefix(agent, &m.author));
            out.push_str(&render::message(&m.text, &self.marker, style));
        }
        if let Some(s) = self.sources(agent, m) {
            out.push_str(&style.dim(&format!("  {s}")));
            out.push('\n');
        }
        out
    }
}

impl Sink for Show {
    fn emit(&self, event: Event) {
        let Some(agent) = self.agent() else { return };
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Event::TurnActivity(a) = &event {
            // The session already said who is typing; a tool round is the one change worth
            // a line of its own.
            if matches!(a.state, ActivityState::Tool { .. }) && st.live.is_none() {
                let who = author_name(&agent, &a.author);
                self.term
                    .note(&working_text(&agent, &who, Working::Searching));
            }
            return;
        }
        match event {
            Event::MessageDelta(d) => {
                if st.live.as_ref().is_some_and(|l| l.id != d.message) {
                    self.close(&mut st);
                }
                if st.live.is_none() {
                    if st.named.as_ref() != Some(&d.message) {
                        self.term.print(&self.prefix(&agent, &d.author));
                    }
                    st.named = Some(d.message.clone());
                    st.live = Some(Live {
                        id: d.message.clone(),
                        streamer: Streamer::new(self.marker.clone(), self.term.style),
                    });
                }
                if let Some(live) = st.live.as_mut() {
                    self.term.print(&live.streamer.push(&d.delta));
                }
            }
            Event::ToolCalled(t) => {
                self.close(&mut st);
                self.term.note(&format!("· {}", action_line(&t.action)));
            }
            Event::MessageAdded(a) => {
                let m = a.message;
                if m.author == Author::User {
                    return;
                }
                let streamed = st.named.as_ref() == Some(&m.id);
                self.close(&mut st);
                st.named = None;
                if streamed {
                    if let Some(s) = self.sources(&agent, &m) {
                        self.term.note(&format!("  {s}"));
                    }
                } else {
                    self.term.print(&self.whole(&agent, &m));
                }
            }
            _ => {}
        }
    }
}
