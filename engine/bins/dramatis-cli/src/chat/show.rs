//! What reaches the terminal: the output handle, and the sink that turns the engine's
//! events into streamed, styled lines.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use api::corpus::Corpus;
use api::error::{ApiError, ApiResult};
use api::events::ActivityState;
use api::port::{Event, Sink};
use api::types::{Author, ChannelId, Citation, MessageId, Mode, PersonId, PersonRef, Target};
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

    /// A line of text.
    pub fn line(&self, text: &str) {
        self.print(&format!("{text}\n"));
    }

    /// A dim line.
    pub fn note(&self, text: &str) {
        self.print(&format!("{}\n", self.style.dim(text)));
    }

    pub fn error(&self, text: &str) {
        self.print(&format!("{}\n", self.style.error(text)));
    }
}

/// `template` with each `{name}` replaced by its value.
fn fill(mut template: String, vars: &[(&str, &dyn Display)]) -> String {
    for (name, value) in vars {
        template = template.replace(&format!("{{{name}}}"), &value.to_string());
    }
    template
}

/// What the terminal needs to put names on things: the corpus's display names and
/// wording, and the user's own name. Read through the same commands the app uses, then
/// kept, so the event sink can look things up without waiting.
pub struct Names {
    corpus: Option<Arc<Corpus>>,
    wording: RwLock<BTreeMap<String, String>>,
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
            wording: RwLock::new(engine.wording().await?.entries),
            user: RwLock::new(String::new()),
        };
        names.refresh_user(engine).await?;
        Ok(names)
    }

    /// Read the wording again, after the interface language changed. The user's name is
    /// read with it: unnamed, the user is called by the wording's word for "you".
    pub async fn reload(&self, engine: &Engine) -> ApiResult<()> {
        *self.wording.write().unwrap_or_else(|e| e.into_inner()) = engine.wording().await?.entries;
        self.refresh_user(engine).await
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
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .map_or_else(|| default.to_owned(), Clone::clone)
    }

    /// [`Self::word`] with its `{field}`s filled.
    pub fn word_with(&self, key: &str, default: &str, vars: &[(&str, &dyn Display)]) -> String {
        fill(self.word(key, default), vars)
    }

    /// One of the terminal's own sentences with its `{field}`s filled.
    pub fn say(&self, key: &str, vars: &[(&str, &dyn Display)]) -> String {
        fill(self.text(key), vars)
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

impl Names {
    /// One tool call as a short line, in the wording the app shows for it when the corpus
    /// has one; the contents are the model's and the corpus's.
    pub fn action_line(&self, action: &ToolAction) -> String {
        let with = |key: &str, default: &str, vars: &[(&str, &dyn Display)]| {
            self.word_with(key, default, vars)
        };
        let list = |members: &[PersonRef]| {
            self.group_label(&members.iter().map(|m| m.name.clone()).collect::<Vec<_>>())
        };
        match action {
            ToolAction::Search { query } => {
                with("action.search", "searched: {query}", &[("query", query)])
            }
            ToolAction::GetChunk { chunk_id } => with(
                "action.get_chunk",
                "read passage: {chunk_id}",
                &[("chunk_id", chunk_id)],
            ),
            ToolAction::RequestJoin { person, topic } => match (person, topic) {
                (Some(p), _) => with(
                    "action.request_join.person",
                    "asked to bring in: {name}",
                    &[("name", &p.name)],
                ),
                (None, Some(t)) => with(
                    "action.request_join.topic",
                    "asked to bring someone in on: {topic}",
                    &[("topic", t)],
                ),
                (None, None) => self.text("cli.action.join"),
            },
            ToolAction::Remember { text } => {
                with("action.remember", "remembered: {text}", &[("text", text)])
            }
            ToolAction::Report => self.word("action.report", "reported back"),
            ToolAction::Wrapup => self.text("cli.action.wrapup"),
            ToolAction::Digest => self.word("action.digest", "looked at the digest"),
            ToolAction::FindPeople { topic, .. } => with(
                "action.find_people",
                "looked for people on: {topic}",
                &[("topic", topic)],
            ),
            ToolAction::Ask { person, question } => with(
                "action.ask",
                "asked {name}: {question}",
                &[("name", &person.name), ("question", question)],
            ),
            // No channel yet: a person's suggestion, which waits for the user.
            ToolAction::CreateGroup {
                members, channel, ..
            } => {
                let (key, default) = if channel.is_some() {
                    ("action.create_group", "created a group: {names}")
                } else {
                    ("action.suggest_group", "suggested a group: {names}")
                };
                with(key, default, &[("names", &list(members))])
            }
            ToolAction::SetMode { target, mode } => {
                let target = match target {
                    Target::Person { id } => self.person(&id.0),
                    Target::Channel { .. } => self.word("action.target.group", "a group"),
                };
                with(
                    "action.set_mode",
                    "set mode: {target} {mode}",
                    &[("target", &target), ("mode", &self.mode_word(*mode))],
                )
            }
            ToolAction::Forget { fact } => self.say("cli.action.forget", &[("n", &fact.0)]),
            ToolAction::ListChannels { channels } => {
                self.say("cli.action.list_channels", &[("n", &channels.len())])
            }
        }
    }

    /// A failed command in words: the app's wording for the error when the corpus has it,
    /// else the engine's own plain text.
    pub fn api_error(&self, e: &ApiError) -> String {
        match e {
            ApiError::Invalid { field } => {
                let key = format!("error.invalid.{field}");
                match self
                    .wording
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&key)
                {
                    Some(text) => text.clone(),
                    None => self.word_with("error.invalid", &e.to_string(), &[("field", field)]),
                }
            }
            ApiError::Disabled { person } => self.word_with(
                "error.disabled",
                &e.to_string(),
                &[("name", &self.person(&person.0))],
            ),
            ApiError::NoPersona { person } => self.word_with(
                "error.no_persona",
                &e.to_string(),
                &[("name", &self.person(&person.0))],
            ),
            ApiError::QuotaExhausted { release_at } => self.word_with(
                "error.quota_exhausted",
                &e.to_string(),
                &[(
                    "when",
                    &release_at
                        .0
                        .chars()
                        .take(16)
                        .collect::<String>()
                        .replace('T', " "),
                )],
            ),
            ApiError::Endpoint { detail }
            | ApiError::Keychain { detail }
            | ApiError::Io { detail } => {
                let code = match e {
                    ApiError::Endpoint { .. } => "error.endpoint",
                    ApiError::Keychain { .. } => "error.keychain",
                    _ => "error.io",
                };
                // The detail is the remote's or the system's own text, shown as it came.
                let lead = self.word(code, "");
                if lead.is_empty() {
                    e.to_string()
                } else {
                    format!("{lead} {detail}")
                }
            }
            ApiError::NotImplemented => self.word("error.not_implemented", &e.to_string()),
            ApiError::NotFound => self.word("error.not_found", &e.to_string()),
            ApiError::NoEndpoint => self.word("error.no_endpoint", &e.to_string()),
            ApiError::Busy => self.word("error.busy", &e.to_string()),
            ApiError::OpenElsewhere => self.word(
                "error.open_elsewhere",
                "this save is open in another window; close it there first",
            ),
            ApiError::Exists => self.word("error.exists", &e.to_string()),
            ApiError::Locked => self.word("error.locked", &e.to_string()),
            ApiError::WrongPassword => self.word("error.wrong_password", &e.to_string()),
        }
    }

    /// A trust mode in words.
    pub fn mode_word(&self, mode: Mode) -> String {
        match mode {
            Mode::Enabled => self.word("mode.enabled", "enabled"),
            Mode::Frozen => self.word("mode.frozen", "frozen"),
            Mode::Disabled => self.word("mode.disabled", "disabled"),
        }
    }
}

/// What a tool call lets the user click or pick: a found person, a created or listed group,
/// or a group someone suggested and the user has not made yet.
fn offers_of(action: &ToolAction, line: Line) -> Vec<Offer> {
    match action {
        ToolAction::FindPeople { persons, .. } => persons
            .iter()
            .map(|p| Offer::Person {
                id: p.id.0.clone(),
                name: p.name.clone(),
                line: Some(line.clone()),
            })
            .collect(),
        ToolAction::CreateGroup {
            members,
            channel: Some(channel),
            ..
        } => vec![Offer::Group {
            channel: channel.clone(),
            members: members.clone(),
        }],
        ToolAction::CreateGroup {
            members,
            channel: None,
            ..
        } if members.len() >= 2 => vec![Offer::NewGroup {
            members: members.clone(),
            line,
        }],
        // An existing conversation opens by the same two offers a new one does: a direct
        // channel is the person's, a group is entered by its id.
        ToolAction::ListChannels { channels } => channels
            .iter()
            .filter_map(|c| match (c.kind, c.members.as_slice()) {
                (ChannelKind::Direct, [p]) => Some(Offer::Person {
                    id: p.id.0.clone(),
                    name: p.name.clone(),
                    line: None,
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

/// A place the host offered to take the user to, or a group someone suggested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    Person {
        id: String,
        name: String,
        /// The find suggestion this person came from, so the reason they were found is
        /// written when they are taken; none for an existing conversation.
        line: Option<Line>,
    },
    Group {
        channel: ChannelId,
        members: Vec<PersonRef>,
    },
    /// Nothing exists yet; taking it creates the group, once, and records it on `line`.
    NewGroup { members: Vec<PersonRef>, line: Line },
}

/// Which action line of which message an offer came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub message: MessageId,
    pub index: u32,
}

fn member_names(members: &[PersonRef]) -> Vec<String> {
    members.iter().map(|m| m.name.clone()).collect()
}

impl Offer {
    pub fn text(&self, names: &Names) -> String {
        match self {
            Offer::Person { name, .. } => names.say("cli.offer.person", &[("name", name)]),
            Offer::Group { members, .. } => names.say(
                "cli.offer.group",
                &[("names", &names.group_label(&member_names(members)))],
            ),
            Offer::NewGroup { members, .. } => names.say(
                "cli.offer.new_group",
                &[("names", &names.group_label(&member_names(members)))],
            ),
        }
    }

    pub fn members(&self) -> Vec<PersonId> {
        match self {
            Offer::Person { id, .. } => vec![PersonId(id.clone())],
            Offer::Group { members, .. } | Offer::NewGroup { members, .. } => {
                members.iter().map(|m| m.id.clone()).collect()
            }
        }
    }
}

#[derive(Default)]
struct State {
    /// Choices from the host's tool calls since the session last took them.
    offers: Vec<Offer>,
    /// How many action lines each message has shown so far: the next one's index.
    lines: BTreeMap<MessageId, u32>,
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
    /// The newest message printed per channel since the session last took them, so what
    /// the user has seen counts as read.
    shown: Mutex<BTreeMap<ChannelId, MessageId>>,
}

impl Show {
    pub fn new(term: Arc<Term>) -> Self {
        Self {
            term,
            bound: OnceLock::new(),
            state: Mutex::new(State::default()),
            shown: Mutex::new(BTreeMap::new()),
        }
    }

    fn saw(&self, m: &Message) {
        if m.author == Author::User {
            return;
        }
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        let newer = |a: &MessageId, b: &MessageId| match (a.0.parse::<i64>(), b.0.parse::<i64>()) {
            (Ok(a), Ok(b)) => a > b,
            _ => a > b,
        };
        match shown.get(&m.channel) {
            Some(old) if !newer(&m.id, old) => {}
            _ => {
                shown.insert(m.channel.clone(), m.id.clone());
            }
        }
    }

    /// The newest message printed in each channel since the last call.
    pub fn take_shown(&self) -> Vec<(ChannelId, MessageId)> {
        std::mem::take(&mut *self.shown.lock().unwrap_or_else(|e| e.into_inner()))
            .into_iter()
            .collect()
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
        (n > 0).then(|| names.say("cli.sources.count", &[("n", &n)]))
    }

    /// A stored message in full: name, text, tool lines, sources.
    pub fn whole(&self, m: &Message) -> String {
        let Some(b) = self.bound.get() else {
            return String::new();
        };
        self.saw(m);
        let style = self.term.style;
        let mut out = String::new();
        for a in &m.actions {
            out.push_str(&style.dim(&format!("· {}", b.names.action_line(a))));
            out.push('\n');
        }
        let picture = m.attachment.is_some() || m.picture_lost;
        if !m.text.trim().is_empty() {
            out.push_str(&self.prefix(&b.names, &m.author));
            out.push_str(&render::message(&m.text, &b.marker, style));
        }
        if picture {
            if m.text.trim().is_empty() {
                out.push_str(&self.prefix(&b.names, &m.author));
            }
            let marker = b.names.text(if m.attachment.is_some() {
                "cli.picture"
            } else {
                "cli.picture.lost"
            });
            out.push_str(&style.dim(&marker));
            out.push('\n');
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
                // The host's finds and lists, and a person's suggestion: the same pick list.
                let index = st.lines.entry(t.message.clone()).or_default();
                let line = Line {
                    message: t.message.clone(),
                    index: *index,
                };
                *index += 1;
                for offer in offers_of(&t.action, line) {
                    if !st.offers.contains(&offer) {
                        st.offers.push(offer);
                    }
                }
                self.term
                    .note(&format!("· {}", names.action_line(&t.action)));
            }
            Event::MessageAdded(a) => {
                let m = a.message;
                if m.author == Author::User {
                    return;
                }
                self.saw(&m);
                let streamed = st.named.as_ref() == Some(&m.id);
                self.close(&mut st);
                st.named = None;
                if streamed {
                    if let Some(s) = self.sources(names, &m) {
                        self.term.note(&format!("  {s}"));
                    }
                } else {
                    // A message posted whole (a happening the host tells of) carries its
                    // offers with it; one already offered as it streamed is not offered twice.
                    for (i, action) in m.actions.iter().enumerate() {
                        let line = Line {
                            message: m.id.clone(),
                            index: i as u32,
                        };
                        for offer in offers_of(action, line) {
                            if !st.offers.contains(&offer) {
                                st.offers.push(offer);
                            }
                        }
                    }
                    drop(st);
                    self.term.print(&self.whole(&m));
                }
            }
            _ => {}
        }
    }
}
