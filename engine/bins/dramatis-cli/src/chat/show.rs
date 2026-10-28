//! What reaches the terminal: the output handle, and the sink that turns the engine's
//! events into streamed, styled lines.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use api::corpus::Corpus;
use api::error::ApiResult;
use api::events::ActivityState;
use api::port::{Event, Sink};
use api::types::{Author, ChannelId, Citation, MessageId, PersonId, PersonRef};
use api::views::{ChannelKind, Message, ToolAction};
use api::{Api, Engine};

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

/// What the terminal needs to put names on things: the corpus's display names and
/// wording, and the user's own name. Read through the same commands the app uses, then
/// kept, so the event sink can look things up without waiting.
pub struct Names {
    corpus: Option<Arc<Corpus>>,
    wording: BTreeMap<String, String>,
    user: RwLock<String>,
}

/// What the speaker is doing while a turn runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Working {
    Typing,
    Searching,
}

/// Most names a group label spells out before it counts the rest.
const LABEL_NAMES: usize = 3;

impl Names {
    pub async fn load(engine: &Engine) -> ApiResult<Self> {
        let names = Self {
            corpus: engine.corpus_handle(),
            wording: engine.wording().await?.entries,
            user: RwLock::new(String::new()),
        };
        names.refresh_user(engine).await?;
        Ok(names)
    }

    /// Read the user's name again, after it was changed.
    pub async fn refresh_user(&self, engine: &Engine) -> ApiResult<()> {
        let name = engine.settings().await?.user_name;
        *self.user.write().unwrap_or_else(|e| e.into_inner()) =
            name.unwrap_or_else(|| self.word("user.you", "you"));
        Ok(())
    }

    pub fn corpus(&self) -> Option<&Arc<Corpus>> {
        self.corpus.as_ref()
    }

    /// The corpus's wording for `key`, else `default`.
    pub fn word(&self, key: &str, default: &str) -> String {
        self.wording
            .get(key)
            .map_or_else(|| default.to_owned(), Clone::clone)
    }

    /// One of the terminal's own sentences: the corpus's wording for `key`, else the
    /// English default in [`super::text`]. `{host}` is the corpus's name for the host.
    pub fn text(&self, key: &str) -> String {
        self.word(key, super::text::default(key))
            .replace("{host}", &self.host())
    }

    pub fn user(&self) -> String {
        self.user.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn host(&self) -> String {
        self.word("host.name", "the host")
    }

    /// Whether the corpus gives the host a persona to talk to.
    pub fn has_host(&self) -> bool {
        self.corpus
            .as_ref()
            .and_then(|c| c.prompt("host", "in_world").ok().flatten())
            .is_some_and(|body| !body.trim().is_empty())
    }

    pub fn person(&self, id: &str) -> String {
        self.corpus
            .as_ref()
            .map_or_else(|| id.to_owned(), |c| c.display(id))
    }

    /// How an author is named on screen: the user by their name, the host by the corpus's
    /// word for it, a person by their display name.
    pub fn author(&self, author: &Author) -> String {
        match author {
            Author::User => self.user(),
            Author::Host => self.host(),
            Author::Person { id } => self.person(&id.0),
        }
    }

    /// The status line for a running turn, in the corpus's wording when it has one.
    pub fn working(&self, who: &str, what: Working) -> String {
        let (key, default) = match what {
            Working::Typing => ("typing.writing", "{name} is typing..."),
            Working::Searching => ("typing.tool", "{name} is looking things up..."),
        };
        self.word(key, default).replace("{name}", who)
    }

    /// Names for a group in the corpus's list separator, the first few and then a count.
    pub fn group_label(&self, names: &[String]) -> String {
        let sep = self.word("list.separator", ", ");
        let shown = names[..names.len().min(LABEL_NAMES)].join(&sep);
        match names.len() - names.len().min(LABEL_NAMES) {
            0 => shown,
            more => self
                .word("list.more", "{names} +{count}")
                .replace("{names}", &shown)
                .replace("{count}", &more.to_string()),
        }
    }

    /// The header of a cited passage, else the chunk id.
    fn source(&self, cite: &Citation) -> String {
        self.corpus
            .as_ref()
            .and_then(|c| c.with_index(|ix| ix.get(&cite.chunk_id, 0)).ok().flatten())
            .map(|(u, _)| {
                if u.header.is_empty() {
                    u.title
                } else {
                    u.header
                }
            })
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| cite.chunk_id.clone())
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
        ToolAction::FindPeople { topic, .. } => format!("looked for people on: {topic}"),
        ToolAction::Ask { person, question } => format!("asked {}: {question}", person.name),
        ToolAction::CreateGroup { members, .. } => format!(
            "created a group: {}",
            members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ToolAction::SetMode { mode, .. } => format!("set mode: {mode:?}").to_lowercase(),
        ToolAction::Forget { fact } => format!("forgot memory {}", fact.0),
        ToolAction::ListChannels { channels } => {
            format!("listed {} conversations", channels.len())
        }
    }
}

/// What a host tool call lets the user click or pick: a found person, a created or listed group.
fn offers_of(action: &ToolAction) -> Vec<Offer> {
    match action {
        ToolAction::FindPeople { persons, .. } => persons
            .iter()
            .map(|p| Offer::Person {
                id: p.id.0.clone(),
                name: p.name.clone(),
            })
            .collect(),
        ToolAction::CreateGroup {
            members,
            channel: Some(channel),
        } => vec![Offer::Group {
            channel: channel.clone(),
            members: members.clone(),
        }],
        // An existing conversation opens by the same two offers a new one does: a direct
        // channel is the person's, a group is entered by its id.
        ToolAction::ListChannels { channels } => channels
            .iter()
            .filter_map(|c| match (c.kind, c.members.as_slice()) {
                (ChannelKind::Direct, [p]) => Some(Offer::Person {
                    id: p.id.0.clone(),
                    name: p.name.clone(),
                }),
                (ChannelKind::Group, members) if !members.is_empty() => Some(Offer::Group {
                    channel: c.id.clone(),
                    members: members.to_vec(),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The reply being written right now.
struct Live {
    id: MessageId,
    streamer: Streamer,
}

/// A place the host offered to take the user to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    Person {
        id: String,
        name: String,
    },
    Group {
        channel: ChannelId,
        members: Vec<PersonRef>,
    },
}

impl Offer {
    pub fn text(&self, names: &Names) -> String {
        match self {
            Offer::Person { name, .. } => format!("talk to {name}"),
            Offer::Group { members, .. } => format!(
                "open group: {}",
                names.group_label(&members.iter().map(|m| m.name.clone()).collect::<Vec<_>>())
            ),
        }
    }

    pub fn members(&self) -> Vec<PersonId> {
        match self {
            Offer::Person { id, .. } => vec![PersonId(id.clone())],
            Offer::Group { members, .. } => members.iter().map(|m| m.id.clone()).collect(),
        }
    }
}

#[derive(Default)]
struct State {
    /// Choices from the host's tool calls since the session last took them.
    offers: Vec<Offer>,
    live: Option<Live>,
    /// The message whose name has been written, so a stream resumed after a tool line
    /// does not repeat it.
    named: Option<MessageId>,
    /// Whose "is typing" line is showing, so one turn says it once.
    typing: Option<Author>,
}

/// What the sink needs once the engine is open.
struct Bound {
    names: Arc<Names>,
    marker: Marker,
}

/// Prints replies as they stream. Built before the engine (which takes it as its sink),
/// bound to the names and the scene marker afterwards.
pub struct Show {
    term: Arc<Term>,
    bound: OnceLock<Bound>,
    state: Mutex<State>,
}

impl Show {
    pub fn new(term: Arc<Term>) -> Self {
        Self {
            term,
            bound: OnceLock::new(),
            state: Mutex::new(State::default()),
        }
    }

    pub fn bind(&self, names: Arc<Names>, marker: Marker) {
        let _ = self.bound.set(Bound { names, marker });
    }

    fn prefix(&self, names: &Names, author: &Author) -> String {
        format!("{}: ", self.term.style.bold(&names.author(author)))
    }

    /// The choices the host offered since the last call, without repeats.
    pub fn take_offers(&self) -> Vec<Offer> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut st.offers)
    }

    /// Close a reply that was cut off (Ctrl-C), so the next output starts on a fresh line.
    pub fn abort(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.close(&mut st);
        st.named = None;
        st.typing = None;
    }

    fn close(&self, st: &mut State) {
        if let Some(mut live) = st.live.take() {
            self.term.print(&live.streamer.finish());
        }
    }

    /// The distinct sources of a message: the header of each cited passage.
    fn source_list(names: &Names, m: &Message) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for c in &m.cites {
            let label = names.source(c);
            if !seen.contains(&label) {
                seen.push(label);
            }
        }
        seen
    }

    /// The sources of a message folded to a count, as printed under a reply. Low
    /// confidence is the common case and says nothing a reader can act on, so it gets no
    /// label here.
    fn sources(&self, names: &Names, m: &Message) -> Option<String> {
        let n = Self::source_list(names, m).len();
        (n > 0).then(|| format!("sources: {n}  (/sources to show)"))
    }

    /// The sources of a message in full, one per line; `None` when it cites nothing.
    pub fn sources_in_full(&self, m: &Message) -> Option<String> {
        let b = self.bound.get()?;
        let list = Self::source_list(&b.names, m);
        if list.is_empty() {
            return None;
        }
        let mut out = format!("sources ({}):\n", list.len());
        for s in list {
            out.push_str(&format!("  {s}\n"));
        }
        Some(out)
    }

    /// A stored message in full: name, text, tool lines, sources.
    pub fn whole(&self, m: &Message) -> String {
        let Some(b) = self.bound.get() else {
            return String::new();
        };
        let style = self.term.style;
        let mut out = String::new();
        for a in &m.actions {
            out.push_str(&style.dim(&format!("· {}", action_line(a))));
            out.push('\n');
        }
        if !m.text.trim().is_empty() {
            out.push_str(&self.prefix(&b.names, &m.author));
            out.push_str(&render::message(&m.text, &b.marker, style));
        }
        if let Some(s) = self.sources(&b.names, m) {
            out.push_str(&style.dim(&format!("  {s}")));
            out.push('\n');
        }
        out
    }
}

impl Sink for Show {
    fn emit(&self, event: Event) {
        let Some(b) = self.bound.get() else { return };
        let names = &b.names;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Event::TurnActivity(a) = &event {
            // Whoever is generating gets the line, not the channel: in a group the one who
            // answers is not known before the turn starts.
            let who = names.author(&a.author);
            match &a.state {
                ActivityState::Thinking | ActivityState::Writing => {
                    if st.typing.as_ref() != Some(&a.author) {
                        st.typing = Some(a.author.clone());
                        self.term.note(&names.working(&who, Working::Typing));
                    }
                }
                ActivityState::Tool { .. } => {
                    if st.live.is_none() {
                        self.term.note(&names.working(&who, Working::Searching));
                    }
                }
                ActivityState::Done | ActivityState::Failed { .. } => {
                    if st.typing.as_ref() == Some(&a.author) {
                        st.typing = None;
                    }
                }
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
                        self.term.print(&self.prefix(names, &d.author));
                    }
                    st.named = Some(d.message.clone());
                    st.live = Some(Live {
                        id: d.message.clone(),
                        streamer: Streamer::new(b.marker.clone(), self.term.style),
                    });
                }
                if let Some(live) = st.live.as_mut() {
                    self.term.print(&live.streamer.push(&d.delta));
                }
            }
            Event::ToolCalled(t) => {
                self.close(&mut st);
                if t.author == Author::Host {
                    for offer in offers_of(&t.action) {
                        if !st.offers.contains(&offer) {
                            st.offers.push(offer);
                        }
                    }
                }
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
                    if let Some(s) = self.sources(names, &m) {
                        self.term.note(&format!("  {s}"));
                    }
                } else {
                    drop(st);
                    self.term.print(&self.whole(&m));
                }
            }
            _ => {}
        }
    }
}
