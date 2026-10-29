//! The state of one terminal session and what each command does with it. Everything it
//! shows or does is a seam command the app issues too; the one exception is `/stats`,
//! which reads the meter through the bridge.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use api::error::{ApiError, ApiResult};
use api::types::{Author, ChannelId, FactId, Mode, PersonId, Target, Tier, Timestamp};
use api::views::{
    Audience, ChannelKind, ChannelSummary, FactFilter, FactScope, FactView, InterfaceLanguage,
    Passage, QuietHours, RosterRow, Settings, StorageKind, StorageUsage, TaskStatus,
};
use api::{Api, Engine};
use runtime::{AgentBridge, Call};

use super::command::{Command, PasswordAct, Save, StorageAct, parse};
use super::complete::{Completions, Snapshot};
use super::interrupt::Interrupt;
use super::show::{Names, Offer, Show, Term, Working};

#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

enum Kind {
    Person(PersonId),
    Host,
    Group(Vec<PersonId>),
}

struct Current {
    channel: ChannelId,
    kind: Kind,
    label: String,
}

/// Someone in the roster.
struct Entry {
    id: String,
    display: String,
    /// Names of the other forms the corpus records for them.
    forms: Vec<String>,
    persona: bool,
}

/// One model call of this session, with the time it took.
struct Timed {
    call: Call,
    /// Time since the previous call finished, or since the command started.
    secs: f64,
}

pub struct Session {
    engine: Arc<Engine>,
    bridge: Arc<AgentBridge>,
    term: Arc<Term>,
    show: Arc<Show>,
    names: Arc<Names>,
    completions: Completions,
    interrupt: Arc<Interrupt>,
    current: Option<Current>,
    /// The host's latest choices, numbered from 1 for `/go`.
    offers: Vec<Offer>,
    calls: Vec<Timed>,
    /// The highest meter row already counted; rows before the session are not ours.
    metered: i64,
    /// Text for the next input line to start with (`/resend`).
    prefill: Option<String>,
    /// The memories of the last `/memories` list, which `/forget`, `/restore` and `/edit`
    /// number into. Dropped when one of them changes the list.
    listed: Vec<FactView>,
    /// The channels of the last `/chats` list, which `/chat` numbers into.
    chats: Vec<ChannelSummary>,
    /// What the next line answers (y/n), when a command asked. The question is the input
    /// prompt while it waits, so it is never drawn over by a fresh prompt.
    pending: Option<Asking>,
    /// Asks for a password with nothing echoed; `None` is no answer. Replaced in tests.
    secret: Secret,
    /// No save is open (first run, the last one is gone, or the user logged out): every
    /// line answers the save list until one opens.
    picking: bool,
    /// The saves of the list on screen, numbered from 1.
    pickable: Vec<String>,
    /// Library mode: the corpus without a language model. On when no chat endpoint or key
    /// is set up, or when the user asked for it; nothing in it calls a model.
    library: bool,
    /// No model can be reached at all (no endpoint or no key), so `/library off` has
    /// nowhere to go.
    no_model: bool,
    /// The passages of the last `/search`, which `/read` numbers into.
    found: Vec<Passage>,
}

/// Asks for a password and returns what was typed.
type Secret = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What `/forget`, `/restore` and `/edit` do to a listed memory.
enum Change {
    Forget,
    Restore,
    Edit(String),
}

/// A confirmation on screen: the step, its question, and whether it was asked twice.
struct Asking {
    what: Pending,
    question: String,
    repeated: bool,
}

/// A destructive step waiting for a yes.
enum Pending {
    DeleteSave(String),
    DeleteGroup(ChannelId),
    Overwrite { name: String, path: String },
    PasswordOn,
    PasswordOff,
    ClearStorage(Vec<StorageKind>),
}

/// How many times a password is asked for before giving up.
const TRIES: usize = 3;

/// Kilobytes as a short human size.
fn size(kb: u32) -> String {
    match kb {
        0..1024 => format!("{kb} KB"),
        1024..1_048_576 => format!("{:.1} MB", f64::from(kb) / 1024.0),
        _ => format!("{:.1} GB", f64::from(kb) / 1_048_576.0),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A path as typed in a shell: a leading `~/` is the home directory.
fn expand(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::Path::new(&home).join(rest).display().to_string(),
        _ => path.to_owned(),
    }
}

/// A passage's heading: its header, else its page.
fn unit_label(p: &Passage) -> &str {
    if p.header.is_empty() {
        &p.citation.page
    } else {
        &p.header
    }
}

fn excerpt(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

impl Session {
    pub async fn new(
        engine: Arc<Engine>,
        bridge: Arc<AgentBridge>,
        term: Arc<Term>,
        show: Arc<Show>,
        names: Arc<Names>,
        completions: Completions,
        interrupt: Arc<Interrupt>,
    ) -> Self {
        interrupt.attach(bridge.clone());
        let metered = bridge
            .calls_after(0)
            .await
            .ok()
            .and_then(|c| c.last().map(|c| c.id))
            .unwrap_or(0);
        let mut session = Self {
            engine,
            bridge,
            term,
            show,
            names,
            completions,
            interrupt,
            current: None,
            offers: Vec::new(),
            calls: Vec::new(),
            metered,
            prefill: None,
            listed: Vec::new(),
            chats: Vec::new(),
            pending: None,
            secret: Box::new(super::secret::read),
            picking: false,
            pickable: Vec::new(),
            library: false,
            no_model: false,
            found: Vec::new(),
        };

        session.picking = session
            .engine
            .app_status()
            .await
            .is_ok_and(|s| s.save.is_none());
        session.refresh_completions().await;
        session
    }

    /// The input prompt: the channel in dim type, then the user's own name, since the
    /// line being typed is theirs and not the channel's.
    pub fn prompt(&self) -> String {
        let style = self.term.style;
        if let Some(asking) = &self.pending {
            return format!("{} ", asking.question);
        }
        if self.picking {
            return format!("{} ", self.term.style.bold(&self.msg("cli.prompt.save")));
        }
        let idle = if self.library {
            self.msg("cli.prompt.library")
        } else {
            "-".to_owned()
        };
        let channel = self
            .current
            .as_ref()
            .map_or(idle.as_str(), |c| c.label.as_str());
        format!(
            "{} {}> ",
            style.dim(&format!("[{channel}]")),
            style.bold(&self.names.user())
        )
    }

    /// One of the terminal's own sentences.
    fn msg(&self, key: &str) -> String {
        self.names.text(key)
    }

    /// A failed command, in words.
    fn fail(&self, e: &ApiError) {
        let text = self.names.api_error(e);
        self.term
            .error(&self.names.say("cli.error", &[("text", &text)]));
    }

    /// Wait for a y/n answer to `question`, which becomes the input prompt.
    fn confirm(&mut self, question: String, what: Pending) {
        self.pending = Some(Asking {
            what,
            question,
            repeated: false,
        });
    }

    /// The confirmation waiting for an answer, which the line editor must not record.
    pub fn question(&self) -> Option<&str> {
        self.pending.as_ref().map(|a| a.question.as_str())
    }

    /// Whether a typed answer is a no.
    fn is_no(&self, line: &str) -> bool {
        let line = line.trim().to_lowercase();
        self.msg("cli.answer.no").split('|').any(|n| n == line)
    }

    /// Whether a typed answer is a yes.
    fn is_yes(&self, line: &str) -> bool {
        let line = line.trim().to_lowercase();
        self.msg("cli.answer.yes").split('|').any(|y| y == line)
    }

    /// Where passwords come from (tests supply them).
    #[cfg(test)]
    pub fn set_secret(&mut self, ask: impl Fn(&str) -> Option<String> + Send + Sync + 'static) {
        self.secret = Box::new(ask);
    }

    /// The text `/resend` put back, for the editor to start the next line with.
    pub fn take_prefill(&mut self) -> Option<String> {
        self.prefill.take()
    }

    /// Set the user's name; a changed name rolls the channels over, as in the app.
    pub async fn set_user(&mut self, name: &str) -> bool {
        let result: ApiResult<()> = async {
            let settings = Settings {
                user_name: Some(name.to_owned()),
                ..self.engine.settings().await?
            };
            self.engine.update_settings(settings).await?;
            self.names.refresh_user(&self.engine).await
        }
        .await;
        if let Err(e) = result {
            self.fail(&e);
            return false;
        }
        // The rollover may ask the model for a summary.
        if !self.library {
            self.interruptible(self.bridge.idle()).await;
        }
        self.report_failures();
        true
    }

    /// One line of input: the answer to a question just asked, else a command or message.
    pub async fn input(&mut self, line: &str) -> Flow {
        if self.save_changed().await {
            return Flow::Continue;
        }
        if self.picking {
            return self.pick(line).await;
        }
        let Some(mut asking) = self.pending.take() else {
            return self.handle(parse(line)).await;
        };
        if self.is_yes(line) {
            self.confirmed(asking.what).await;
            self.refresh_completions().await;
        } else if self.is_no(line) || line.trim().is_empty() || asking.repeated {
            self.term.note(&self.msg("cli.cancelled"));
        } else {
            // Neither yes nor no: ask once more, then give up.
            asking.repeated = true;
            self.term.note(&self.msg("cli.answer.again"));
            self.pending = Some(asking);
        }
        Flow::Continue
    }

    pub async fn handle(&mut self, command: Command) -> Flow {
        match command {
            Command::Noop => {}
            Command::Quit => return Flow::Quit,
            Command::Help => self.term.print(&format!("{}\n", self.help())),
            Command::Invalid(key) => self.term.line(&self.msg(key)),
            Command::Unknown(name) => self
                .term
                .line(&self.names.say("cli.unknown_command", &[("name", &name)])),
            Command::List(filter) => self.list(filter.as_deref()).await,
            Command::Talk(who) => {
                // Talking to someone leaves the library when a model can answer.
                if self.library && !self.no_model {
                    self.set_library(false);
                }
                self.talk(&who).await
            }
            Command::Group(who) => self.group(&who).await,
            Command::Host => self.host().await,
            Command::Go(_) if self.library => self.needs_model(),
            Command::Go(n) => self.go(n).await,
            Command::Chats(archived) => self.list_chats(archived).await,
            Command::Chat(n) => self.chat(n).await,
            Command::Who(who) => self.who(&who).await,
            Command::Mode(who, mode) => self.mode(&who, &mode).await,
            Command::Archive(on) => self.archive(on).await,
            Command::Rename(name) => self.rename(&name).await,
            Command::Members => self.members(),
            Command::Invite(who) => self.invite(&who).await,
            Command::Remove(who) => self.remove(&who).await,
            Command::Leave => self.leave(),
            Command::Read(all) => self.read(all).await,
            Command::ReadUnit(unit) => self.read_unit(&unit).await,
            Command::Library(on) => self.set_library(on),
            Command::Ask(..) | Command::Say(_) | Command::Wrapup if self.library => {
                self.needs_model()
            }
            // Without any endpoint a rollover has no model to ask; with one it would.
            Command::Name(_) if self.library && !self.no_model => self.needs_model(),
            Command::Ask(who, question) => self.ask(&who, question).await,
            Command::Requests => self.requests().await,
            Command::Clock => self.clock().await,
            Command::Find(topic) => self.find(&topic).await,
            Command::Waiting => self.waiting().await,
            Command::Birthday(day) => self.birthday(day.as_deref()).await,
            Command::Name(name) => {
                if self.set_user(&name).await {
                    self.term
                        .note(&self.names.say("cli.name.set", &[("name", &name)]));
                }
            }
            Command::Bio(text) => self.bio(text).await,
            Command::Password(act) => self.password(act).await,
            Command::Say(text) => self.say(&text).await,
            Command::Wrapup => self.wrapup().await,
            Command::Memories(who) => self.memories(who.as_deref()).await,
            Command::Forget(n) => self.change_memory(n, Change::Forget).await,
            Command::Restore(n) => self.change_memory(n, Change::Restore).await,
            Command::Edit(n, text) => self.change_memory(n, Change::Edit(text)).await,
            Command::Lang(arg) => self.lang(arg.as_deref()).await,
            Command::Tier(level) => self.tier(level.as_deref()).await,
            Command::Quiet(arg) => self.quiet(arg.as_deref()).await,
            Command::Events(arg) => self.events(arg.as_deref()).await,
            Command::Save(cmd) => {
                self.save(cmd).await;
                self.refresh_completions().await;
            }
            Command::History(n) => self.history(n).await,
            Command::Resend(n) => self.resend(n).await,
            Command::Sources(n, entry) => self.sources(n, entry).await,
            Command::Search(query) => self.search(&query).await,
            Command::Stats => self.stats(),
            Command::Storage(act) => self.storage(act).await,
        }
        self.mark_seen().await;
        Flow::Continue
    }

    /// What was printed counts as read, here and in the app: the cursor moves up to the
    /// newest message shown in each channel.
    async fn mark_seen(&self) {
        for (channel, up_to) in self.show.take_shown() {
            if let Err(e) = self.engine.mark_read(channel, up_to).await {
                self.fail(&e);
            }
        }
    }

    /// Print what arrived in `channel` since it was last read, then it is read.
    async fn catch_up(&self, channel: &ChannelId) {
        let Ok(list) = self.engine.channels().await else {
            return;
        };
        let shown = list.iter().find(|c| &c.id == channel);
        // An archived conversation stays archived when opened; say so, and how to undo it.
        if shown.is_some_and(|c| c.archived) {
            self.term.note(&self.msg("cli.archived.open"));
        }
        let unread = shown.map_or(0, |c| c.unread as usize);
        if unread == 0 {
            return;
        }
        let Ok(messages) = self.engine.history(channel.clone(), None, 200).await else {
            return;
        };
        let from = messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, m)| m.author != Author::User)
            .nth(unread - 1)
            .map_or(0, |(i, _)| i);
        self.term
            .note(&self.names.say("cli.unread.header", &[("n", &unread)]));
        for m in &messages[from..] {
            self.term.print(&self.show.whole(m));
        }
    }

    // ---- who ----

    /// Everyone in the roster, with the other names the corpus records for them.
    async fn roster(&self) -> ApiResult<Vec<Entry>> {
        let roster = self.engine.roster().await?;
        Ok(roster
            .groups
            .into_iter()
            .flat_map(|g| g.rows)
            .map(|r: RosterRow| Entry {
                forms: self
                    .names
                    .corpus()
                    .and_then(|c| c.with_folio(|f| f.person(&r.person.id.0)).ok().flatten())
                    .map(|p| p.forms.into_iter().map(|f| f.page).collect())
                    .unwrap_or_default(),
                persona: r.has_persona,
                id: r.person.id.0,
                display: r.person.name,
            })
            .collect())
    }

    /// What the editor's Tab completes to: the names persons answer to and the save names.
    /// Read again after a `/save` command, which can change either. When either cannot be
    /// read the previous lists stay, and the error is shown.
    async fn refresh_completions(&self) {
        let roster = match self.roster().await {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        let talkable: Vec<&Entry> = roster.iter().filter(|e| e.persona).collect();
        let mut names: Vec<String> = talkable
            .iter()
            .flat_map(|e| std::iter::once(&e.display).chain(&e.forms).cloned())
            .collect();
        // Aliases are the corpus's own dictionary: keep those that name a talkable person.
        if let Some(corpus) = self.names.corpus() {
            let pairs = corpus
                .with_folio(|f| f.aliases_for_persons())
                .unwrap_or_default();
            names.extend(
                pairs
                    .into_iter()
                    .filter(|(_, id)| talkable.iter().any(|e| &e.id == id))
                    .map(|(alias, _)| alias),
            );
        }
        let ids = talkable.iter().map(|e| e.id.clone()).collect();
        let saves = match self.engine.saves().await {
            Ok(s) => s,
            Err(e) => return self.fail(&e),
        };
        self.completions.set(Snapshot::new(
            names,
            ids,
            saves.into_iter().map(|s| s.name).collect(),
        ));
    }

    async fn list(&self, filter: Option<&str>) {
        let roster = match self.roster().await {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        let needle = filter.map(str::to_lowercase);
        let host = self.msg(if self.names.has_host() && !self.library {
            "cli.list.host.available"
        } else {
            "cli.list.host.unavailable"
        });
        self.term.line(&self.names.say(
            "cli.list.host",
            &[("host", &self.names.host()), ("state", &host)],
        ));
        let mut shown = 0;
        for e in roster.iter().filter(|e| e.persona || self.library) {
            let hit = needle.as_deref().is_none_or(|n| {
                e.id.to_lowercase().contains(n)
                    || e.display.to_lowercase().contains(n)
                    || e.forms.iter().any(|f| f.to_lowercase().contains(n))
            });
            if hit {
                self.term
                    .print(&format!("{}  {}\n", e.display, self.term.style.dim(&e.id)));
                shown += 1;
            }
        }
        let key = if self.library {
            "cli.list.count"
        } else {
            "cli.list.count.persona"
        };
        self.term.note(&self.names.say(key, &[("n", &shown)]));
    }

    /// Resolve what the user typed to one person with a persona: id, display name, another
    /// form's name, an alias the corpus records, then a unique fragment.
    async fn resolve(&self, query: &str) -> Result<PersonId, String> {
        self.resolve_as(query, true).await
    }

    /// Like [`Self::resolve`]; with `talkable` false a person without a persona is found
    /// too (for a profile or a mode).
    async fn resolve_as(&self, query: &str, talkable: bool) -> Result<PersonId, String> {
        let roster = self.roster().await.map_err(|e| self.names.api_error(&e))?;
        let q = query.trim();
        let lower = q.to_lowercase();
        let exact: Vec<&Entry> = roster
            .iter()
            .filter(|e| {
                e.id == q
                    || e.display.to_lowercase() == lower
                    || e.forms.iter().any(|f| f.to_lowercase() == lower)
            })
            .collect();
        let found: Vec<&Entry> = if !exact.is_empty() {
            exact
        } else {
            let aliased = self
                .names
                .corpus()
                .and_then(|c| c.with_folio(|f| f.persons_for_alias(q)).ok())
                .unwrap_or_default();
            let by_alias: Vec<&Entry> = roster.iter().filter(|e| aliased.contains(&e.id)).collect();
            if !by_alias.is_empty() {
                by_alias
            } else {
                roster
                    .iter()
                    .filter(|e| {
                        e.id.to_lowercase().contains(&lower)
                            || e.display.to_lowercase().contains(&lower)
                    })
                    .collect()
            }
        };
        let with_persona: Vec<&Entry> = found
            .iter()
            .copied()
            .filter(|e| e.persona || !talkable)
            .collect();
        match (found.len(), with_persona.as_slice()) {
            (0, _) => Err(self.names.say("cli.resolve.none", &[("q", &q)])),
            (_, []) => Err(self
                .names
                .say("cli.resolve.no_persona", &[("name", &found[0].display)])),
            (_, [one]) => Ok(PersonId(one.id.clone())),
            (_, many) => Err(self.names.say(
                "cli.resolve.ambiguous",
                &[
                    ("q", &q),
                    (
                        "names",
                        &many
                            .iter()
                            .take(8)
                            .map(|e| format!("{} ({})", e.display, e.id))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ],
            )),
        }
    }

    async fn talk(&mut self, who: &str) {
        match self.resolve(who).await {
            Ok(person) => self.open_person(person).await,
            Err(e) => self.term.line(&e),
        }
    }

    async fn open_person(&mut self, person: PersonId) {
        let opened = self.engine.open_direct(person.clone()).await;
        self.enter_person(person, opened).await;
    }

    async fn enter_person(&mut self, person: PersonId, opened: ApiResult<ChannelId>) {
        match opened {
            Ok(channel) => {
                let name = self.names.person(&person.0);
                self.term.note(
                    &self
                        .names
                        .say("cli.talk.person", &[("name", &name), ("id", &person.0)]),
                );
                self.catch_up(&channel).await;
                self.current = Some(Current {
                    channel,
                    kind: Kind::Person(person),
                    label: name,
                });
            }
            Err(e) => self.fail(&e),
        }
    }

    async fn group(&mut self, who: &[String]) {
        let mut members: Vec<PersonId> = Vec::new();
        for w in who {
            match self.resolve(w).await {
                Ok(p) if !members.contains(&p) => members.push(p),
                Ok(_) => {}
                Err(e) => return self.term.line(&e),
            }
        }
        if members.len() < 2 {
            return self.term.line(&self.msg("cli.group.too_few"));
        }
        match self.engine.create_group(members.clone(), None).await {
            Ok(channel) => self.enter_group(channel, members).await,
            Err(e) => self.fail(&e),
        }
    }

    async fn enter_group(&mut self, channel: ChannelId, members: Vec<PersonId>) {
        let names: Vec<String> = members.iter().map(|p| self.names.person(&p.0)).collect();
        let label = self.names.group_label(&names);
        self.term
            .note(&self.names.say("cli.talk.group", &[("label", &label)]));
        self.catch_up(&channel).await;
        self.current = Some(Current {
            channel,
            kind: Kind::Group(members),
            label,
        });
    }

    /// A group changed its roster behind the session's back (an invitation was taken):
    /// read its members again, and go into it unless it is already the open one.
    async fn joined_group(&mut self, channel: ChannelId) {
        let members = match self.engine.channels().await {
            Ok(list) => list
                .into_iter()
                .find(|c| c.id == channel)
                .map(|c| c.participants.into_iter().map(|p| p.id).collect::<Vec<_>>()),
            Err(e) => return self.fail(&e),
        };
        let Some(members) = members else {
            return;
        };
        if self.current.as_ref().is_some_and(|c| c.channel == channel) {
            self.set_members(members);
        } else {
            self.enter_group(channel, members).await;
        }
    }

    /// Switch to choice `n` of the host's latest offer.
    async fn go(&mut self, n: usize) {
        let Some(offer) = self.offers.get(n - 1).cloned() else {
            return self.term.line(&self.msg(if self.offers.is_empty() {
                "cli.go.nothing"
            } else {
                "cli.go.no_such"
            }));
        };
        match offer {
            // Taking a found person writes why they were found, once.
            Offer::Person {
                id,
                line: Some(line),
                ..
            } => {
                let person = PersonId(id);
                let opened = self
                    .engine
                    .open_found(line.message, line.index, person.clone())
                    .await;
                self.enter_person(person, opened).await;
            }
            Offer::Person { id, .. } => self.open_person(PersonId(id)).await,
            // Taking an invitation adds the person as `/invite` does; the group stays open.
            Offer::Invite { id, line, .. } => {
                let person = PersonId(id);
                match self
                    .engine
                    .open_invite(line.message, line.index, person.clone())
                    .await
                {
                    Ok(channel) => self.joined_group(channel).await,
                    Err(e) => self.fail(&e),
                }
            }
            group @ Offer::Group { .. } => {
                let Offer::Group { channel, .. } = &group else {
                    return;
                };
                self.enter_group(channel.clone(), group.members()).await;
            }
            // Accepting is what creates it; taking it again returns the same group.
            group @ Offer::NewGroup { .. } => {
                let members = group.members();
                let Offer::NewGroup { line, .. } = &group else {
                    return;
                };
                match self
                    .engine
                    .open_offer(line.message.clone(), line.index)
                    .await
                {
                    Ok(channel) => self.enter_group(channel, members).await,
                    Err(e) => self.fail(&e),
                }
            }
        }
    }

    /// The current group and its members, or the reason there is none, said to the user.
    fn current_group(&self) -> Option<(ChannelId, Vec<PersonId>)> {
        match self.current.as_ref() {
            Some(Current {
                channel,
                kind: Kind::Group(members),
                ..
            }) => Some((channel.clone(), members.clone())),
            Some(_) => {
                self.term.line(&self.msg("cli.members.not_group"));
                None
            }
            None => {
                self.term.line(&self.msg("cli.no_channel"));
                None
            }
        }
    }

    /// The current group has these members now.
    fn set_members(&mut self, members: Vec<PersonId>) {
        let names: Vec<String> = members.iter().map(|p| self.names.person(&p.0)).collect();
        let label = self.names.group_label(&names);
        if let Some(c) = self.current.as_mut() {
            c.kind = Kind::Group(members);
            c.label = label;
        }
    }

    /// Bring people into the current group; the group says who joined.
    async fn invite(&mut self, who: &[String]) {
        let Some((channel, members)) = self.current_group() else {
            return;
        };
        let mut added: Vec<PersonId> = Vec::new();
        for w in who {
            match self.resolve(w).await {
                Ok(p) if !members.contains(&p) && !added.contains(&p) => added.push(p),
                Ok(_) => {}
                Err(e) => return self.term.line(&e),
            }
        }
        if added.is_empty() {
            return self.term.line(&self.msg("cli.invite.already"));
        }
        match self.engine.add_members(channel, added.clone()).await {
            Ok(()) => {
                let mut members = members;
                members.extend(added);
                self.set_members(members);
            }
            Err(e) => self.fail(&e),
        }
    }

    /// Take a person out of the current group; what they said stays.
    async fn remove(&mut self, who: &str) {
        let Some((channel, mut members)) = self.current_group() else {
            return;
        };
        let person = match self.resolve_as(who, false).await {
            Ok(p) => p,
            Err(e) => return self.term.line(&e),
        };
        if !members.contains(&person) {
            let name = self.names.person(&person.0);
            return self
                .term
                .line(&self.names.say("cli.remove.not_member", &[("name", &name)]));
        }
        match self.engine.remove_member(channel, person.clone()).await {
            Ok(()) => {
                members.retain(|p| p != &person);
                self.set_members(members);
            }
            Err(ApiError::Invalid { .. }) => self.term.line(&self.msg("cli.remove.last")),
            Err(e) => self.fail(&e),
        }
    }

    #[cfg(test)]
    pub fn offers_for_test(&mut self) {
        self.offer();
    }

    /// Print what the host just offered and keep it for `/go`.
    fn offer(&mut self) {
        let offers = self.show.take_offers();
        if offers.is_empty() {
            return;
        }
        for (i, o) in offers.iter().enumerate() {
            self.term
                .note(&format!("  {} {}", i + 1, o.text(&self.names)));
        }
        self.term.note(&self.msg("cli.go.hint"));
        self.offers = offers;
    }

    async fn host(&mut self) {
        if !self.names.has_host() {
            return self.term.line(&self.msg("cli.host.none"));
        }
        match self.engine.host_channel().await {
            Ok(channel) => {
                let name = self.names.host();
                self.term
                    .note(&self.names.say("cli.talk.host", &[("name", &name)]));
                self.catch_up(&channel).await;
                self.current = Some(Current {
                    channel,
                    kind: Kind::Host,
                    label: name,
                });
            }
            Err(e) => self.fail(&e),
        }
    }

    // ---- conversations, people and requests ----

    fn stamp(at: &Timestamp) -> String {
        at.0.chars().take(16).collect::<String>().replace('T', " ")
    }

    /// The title a channel has in the list.
    fn title_of(&self, c: &ChannelSummary) -> String {
        if let Some(name) = c.name.as_deref().filter(|n| !n.trim().is_empty()) {
            return name.to_owned();
        }
        match c.kind {
            ChannelKind::Host => self.names.host(),
            _ => {
                let names: Vec<String> = c.participants.iter().map(|p| p.name.clone()).collect();
                self.names.group_label(&names)
            }
        }
    }

    async fn list_chats(&mut self, archived: bool) {
        let mut list: Vec<ChannelSummary> = match self.engine.channels().await {
            Ok(l) => l.into_iter().filter(|c| c.archived == archived).collect(),
            Err(e) => return self.fail(&e),
        };
        // Newest first; the host's channel leads when nothing was said anywhere.
        list.sort_by(|a, b| b.last_at.cmp(&a.last_at));
        if list.is_empty() {
            return self.term.note(&self.msg(if archived {
                "cli.chats.none_archived"
            } else {
                "cli.chats.none"
            }));
        }
        for (i, c) in list.iter().enumerate() {
            let kind = self.msg(match c.kind {
                ChannelKind::Host => "cli.chats.kind.host",
                ChannelKind::Direct => "cli.chats.kind.direct",
                ChannelKind::Group => "cli.chats.kind.group",
            });
            let when = c.last_at.as_ref().map_or(String::new(), Self::stamp);
            let unread = if c.unread > 0 {
                format!(
                    "  {}",
                    self.names.say("cli.chats.unread", &[("n", &c.unread)])
                )
            } else {
                String::new()
            };
            self.term.print(&format!(
                "{:>3}  {}  {}{}\n",
                i + 1,
                self.title_of(c),
                self.term.style.dim(&format!("{kind} {when}")),
                unread
            ));
        }
        self.chats = list;
        self.term.note(&self.msg("cli.chats.hint"));
    }

    async fn chat(&mut self, n: usize) {
        let Some(c) = self.chats.get(n - 1).cloned() else {
            return self.term.line(&self.msg(if self.chats.is_empty() {
                "cli.chat.no_list"
            } else {
                "cli.chat.no_such"
            }));
        };
        match (c.kind, c.participants.first()) {
            (ChannelKind::Host, _) => self.host().await,
            (ChannelKind::Direct, Some(p)) => self.open_person(p.id.clone()).await,
            _ => {
                let members = c.participants.iter().map(|p| p.id.clone()).collect();
                self.enter_group(c.id, members).await;
            }
        }
    }

    async fn who(&self, who: &str) {
        let person = match self.resolve_as(who, false).await {
            Ok(p) => p,
            Err(e) => return self.term.line(&e),
        };
        let page = match self.engine.person_page(person).await {
            Ok(p) => p,
            Err(e) => return self.fail(&e),
        };
        self.term.print(&format!(
            "{}  {}\n",
            self.term.style.bold(&page.person.name),
            self.term.style.dim(&page.person.id.0)
        ));
        if let Some(s) = &page.summary {
            self.term.print(&format!("{s}\n"));
        }
        self.term.line(&self.names.say(
            "cli.who.mode",
            &[
                ("mode", &self.names.mode_word(page.mode)),
                ("trust", &page.trust),
            ],
        ));
        if !page.has_persona {
            self.term.line(&self.msg("cli.who.no_persona"));
        }
        let seen = page
            .last_seen
            .as_ref()
            .map_or_else(|| self.msg("cli.who.never"), Self::stamp);
        self.term.line(&self.names.say(
            "cli.who.seen",
            &[("seen", &seen), ("done", &page.tasks_done)],
        ));
        if !page.companions.is_empty() {
            let names: Vec<&str> = page.companions.iter().map(|p| p.name.as_str()).collect();
            self.term.line(
                &self
                    .names
                    .say("cli.who.close", &[("names", &names.join(", "))]),
            );
        }
    }

    async fn mode(&self, who: &str, mode: &str) {
        let mode = match mode {
            "enabled" => Mode::Enabled,
            "frozen" => Mode::Frozen,
            _ => Mode::Disabled,
        };
        // Named, or the conversation we are in: a group has a mode of its own.
        let (target, name) = if who.is_empty() {
            match self.current.as_ref() {
                Some(Current {
                    kind: Kind::Person(p),
                    ..
                }) => (Target::Person { id: p.clone() }, self.names.person(&p.0)),
                Some(Current {
                    channel,
                    kind: Kind::Group(_),
                    label,
                }) => (
                    Target::Channel {
                        id: channel.clone(),
                    },
                    label.clone(),
                ),
                Some(_) => return self.term.line(&self.msg("cli.mode.host")),
                None => return self.term.line(&self.msg("cli.no_channel")),
            }
        } else {
            match self.resolve_as(who, false).await {
                Ok(p) => {
                    let name = self.names.person(&p.0);
                    (Target::Person { id: p }, name)
                }
                Err(e) => return self.term.line(&e),
            }
        };
        match self.engine.set_mode(target, mode).await {
            Ok(()) => self.term.note(&self.names.say(
                "cli.mode.set",
                &[("name", &name), ("mode", &self.names.mode_word(mode))],
            )),
            Err(e) => self.fail(&e),
        }
    }

    async fn archive(&self, archived: bool) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        match self.engine.archive_channel(channel, archived).await {
            Ok(()) => self.term.note(&self.msg(if archived {
                "cli.archive.on"
            } else {
                "cli.archive.off"
            })),
            Err(api::ApiError::Invalid { .. }) => self.term.line(&self.msg("cli.archive.host")),
            Err(e) => self.fail(&e),
        }
    }

    async fn rename(&mut self, name: &str) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        let title = (!name.is_empty()).then(|| name.to_owned());
        match self.engine.rename_channel(channel, title).await {
            Ok(()) => self.term.note(&self.msg(if name.is_empty() {
                "cli.rename.cleared"
            } else {
                "cli.rename.done"
            })),
            Err(e) => self.fail(&e),
        }
    }

    fn members(&self) {
        match self.current.as_ref().map(|c| &c.kind) {
            Some(Kind::Group(members)) => {
                let names: Vec<String> = members.iter().map(|p| self.names.person(&p.0)).collect();
                self.term.print(&format!("{}\n", names.join(", ")));
            }
            Some(_) => self.term.line(&self.msg("cli.members.not_group")),
            None => self.term.line(&self.msg("cli.no_channel")),
        }
    }

    fn leave(&mut self) {
        match self.current.as_ref() {
            Some(Current {
                channel,
                kind: Kind::Group(_),
                ..
            }) => {
                self.confirm(
                    self.msg("cli.leave.ask"),
                    Pending::DeleteGroup(channel.clone()),
                );
            }
            Some(_) => self.term.line(&self.msg("cli.leave.only_group")),
            None => self.term.line(&self.msg("cli.no_channel")),
        }
    }

    /// Mark the current channel, or every one, read up to its newest message.
    async fn read(&self, all: bool) {
        let channels: Vec<ChannelId> = if all {
            match self.engine.channels().await {
                Ok(l) => l
                    .into_iter()
                    .filter(|c| c.unread > 0)
                    .map(|c| c.id)
                    .collect(),
                Err(e) => return self.fail(&e),
            }
        } else {
            match self.current_channel().await {
                Some(c) => vec![c],
                None => return,
            }
        };
        for channel in &channels {
            let last = match self.engine.history(channel.clone(), None, 1).await {
                Ok(mut m) => m.pop(),
                Err(e) => return self.fail(&e),
            };
            if let Some(m) = last
                && let Err(e) = self.engine.mark_read(channel.clone(), m.id).await
            {
                return self.fail(&e);
            }
        }
        self.term.note(&self.names.say(
            if channels.len() == 1 {
                "cli.read.marked.one"
            } else {
                "cli.read.marked"
            },
            &[("n", &channels.len())],
        ));
    }

    async fn ask(&mut self, who: &str, question: String) {
        let person = match self.resolve(who).await {
            Ok(p) => p,
            Err(e) => return self.term.line(&e),
        };
        let name = self.names.person(&person.0);
        let (started, at) = (Instant::now(), now_ms());
        let (engine, bridge) = (self.engine.clone(), self.bridge.clone());
        let result = self
            .interruptible(async move {
                engine.ask(person, question, None, None).await?;
                bridge.idle().await;
                Ok::<(), api::ApiError>(())
            })
            .await;
        match result {
            Some(Err(e)) => self.fail(&e),
            Some(Ok(())) => self
                .term
                .note(&self.names.say("cli.ask.sent", &[("name", &name)])),
            None => {}
        }
        self.report_failures();
        self.footer(at, started).await;
    }

    async fn requests(&self) {
        let tasks = match self.engine.tasks().await {
            Ok(t) => t,
            Err(e) => return self.fail(&e),
        };
        if tasks.is_empty() {
            return self.term.note(&self.msg("cli.requests.none"));
        }
        for t in &tasks {
            let state = self.names.word_with(
                match t.status {
                    TaskStatus::Active => "cases.active",
                    TaskStatus::Done => "cases.done",
                },
                match t.status {
                    TaskStatus::Active => "active",
                    TaskStatus::Done => "done",
                },
                &[],
            );
            let who = t
                .assignee
                .as_ref()
                .map_or_else(|| self.msg("cli.requests.nobody"), |p| p.name.clone());
            let pin = if t.pinned {
                format!(" {}", self.msg("cli.requests.pinned"))
            } else {
                String::new()
            };
            self.term.line(&format!(
                "{:<7} {}  {}: {}{}",
                state,
                Self::stamp(&t.created_at),
                who,
                excerpt(&t.question, 60),
                pin
            ));
        }
    }

    async fn clock(&self) {
        match self.engine.clock().await {
            Ok(c) => self.term.print(&format!(
                "{:04}-{:02}-{:02} {}\n",
                c.in_world.year, c.in_world.month, c.in_world.day, c.local_time
            )),
            Err(e) => self.fail(&e),
        }
    }

    async fn find(&self, topic: &str) {
        match self.engine.find_people(topic.to_owned()).await {
            Ok(found) if found.is_empty() => self.term.note(&self.msg("cli.find.none")),
            Ok(found) => {
                for c in found {
                    self.term.print(&format!(
                        "{}  {}\n    {}\n",
                        c.person.name,
                        self.term.style.dim(&c.person.id.0),
                        excerpt(&c.reason, 100)
                    ));
                }
            }
            Err(e) => self.fail(&e),
        }
    }

    async fn waiting(&self) {
        use api::views::Activity;
        let digest = match self.engine.digest().await {
            Ok(d) => d,
            Err(e) => return self.fail(&e),
        };
        if digest.items.is_empty() {
            return self.term.note(&self.msg("cli.waiting.none"));
        }
        for w in &digest.items {
            let word = |key: &str, default: &str, vars: &[(&str, &dyn std::fmt::Display)]| {
                self.names.word_with(key, default, vars)
            };
            let what = match &w.activity {
                Activity::Messaged { .. } => word("activity.messaged", "wrote to you", &[]),
                Activity::Replied { question, .. } => word(
                    "activity.replied",
                    "answered: {question}",
                    &[("question", &excerpt(question, 50))],
                ),
                Activity::CommitmentDue { text, .. } => word(
                    "activity.commitment_due",
                    "due: {text}",
                    &[("text", &excerpt(text, 50))],
                ),
                Activity::CameBy { .. } => word("activity.came_by", "came by", &[]),
                Activity::Birthday => word("activity.birthday", "has a birthday today", &[]),
            };
            self.term.line(&format!("{}  {what}", w.person.name));
        }
    }

    async fn birthday(&self, day: Option<&str>) {
        let settings = match self.engine.settings().await {
            Ok(s) => s,
            Err(e) => return self.fail(&e),
        };
        let Some(day) = day else {
            return self.term.note(&match settings.birthday {
                Some(d) => self.names.say(
                    "cli.birthday.show",
                    &[("date", &format!("{:02}-{:02}", d.month, d.day))],
                ),
                None => self.msg("cli.birthday.none"),
            });
        };
        let parsed = if day == "clear" {
            Some(None)
        } else {
            day.split_once('-')
                .and_then(|(m, d)| Some((m.parse::<u8>().ok()?, d.parse::<u8>().ok()?)))
                .map(|(month, day)| Some(api::views::MonthDay { month, day }))
        };
        let Some(birthday) = parsed else {
            return self.term.line(&self.msg("cli.usage.birthday"));
        };
        match self
            .engine
            .update_settings(Settings {
                birthday,
                ..settings
            })
            .await
        {
            Ok(()) => self.term.note(&self.msg(if birthday.is_some() {
                "cli.birthday.saved"
            } else {
                "cli.birthday.cleared"
            })),
            Err(api::ApiError::Invalid { .. }) => self.term.line(&self.msg("cli.birthday.invalid")),
            Err(e) => self.fail(&e),
        }
    }

    // ---- talking ----

    /// Run a model-facing future; Ctrl-C abandons the background work instead of ending
    /// the session. What was already stored stays stored, so the cancelled turn may leave
    /// the user's line without a reply.
    async fn interruptible<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        self.interrupt.clear();
        tokio::select! {
            r = work => Some(r),
            _ = self.interrupt.wait() => {
                self.bridge.cancel();
                self.show.abort();
                self.term.note(&self.msg("cli.interrupted"));
                None
            }
        }
    }

    /// What background work reported as failed: the endpoint's own text for a turn that
    /// ended without a reply.
    fn report_failures(&self) {
        for f in self.bridge.take_failures() {
            self.term
                .error(&self.names.say("cli.error", &[("text", &f)]));
        }
    }

    async fn current_channel(&self) -> Option<ChannelId> {
        let channel = self.current.as_ref().map(|c| c.channel.clone());
        if channel.is_none() {
            self.term.line(&self.msg("cli.no_channel"));
        }
        channel
    }

    async fn say(&mut self, text: &str) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        let (started, at) = (Instant::now(), now_ms());
        // The typing line comes from the turn's own activity events, naming who writes.
        let (engine, bridge) = (self.engine.clone(), self.bridge.clone());
        let result = self
            .interruptible(async move {
                engine.send_message(channel, text.to_owned(), None).await?;
                bridge.idle().await;
                Ok::<(), api::ApiError>(())
            })
            .await;
        if let Some(Err(e)) = result {
            self.fail(&e);
        }
        self.report_failures();
        self.offer();
        self.footer(at, started).await;
        self.arrivals(false).await;
    }

    /// Count the calls made since `at` and say how many and how long.
    async fn footer(&mut self, at: i64, started: Instant) {
        let n = self.collect(at).await;
        if n > 0 {
            self.term.note(&self.names.say(
                if n == 1 {
                    "cli.footer.one"
                } else {
                    "cli.footer"
                },
                &[
                    ("n", &n),
                    ("secs", &format!("{:.1}", started.elapsed().as_secs_f64())),
                ],
            ));
        }
    }

    /// Read the meter rows added since the last look into `calls`. A call's time is the
    /// gap from the previous call's end (or `at`), which is when it began.
    async fn collect(&mut self, at: i64) -> usize {
        let rows = self
            .bridge
            .calls_after(self.metered)
            .await
            .unwrap_or_default();
        let mut prev = at;
        for call in &rows {
            self.calls.push(Timed {
                secs: (call.at - prev).max(0) as f64 / 1000.0,
                call: call.clone(),
            });
            prev = call.at;
            self.metered = call.id;
        }
        rows.len()
    }

    async fn wrapup(&mut self) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        if matches!(self.current.as_ref().map(|c| &c.kind), Some(Kind::Host)) {
            return self.term.line(&self.msg("cli.wrapup.host"));
        }
        let before = self.fact_ids().await;
        let (started, at) = (Instant::now(), now_ms());
        let who = self.current.as_ref().map_or("", |c| c.label.as_str());
        self.term.note(&self.names.working(who, Working::Typing));
        let (engine, bridge) = (self.engine.clone(), self.bridge.clone());
        let result = self
            .interruptible(async move {
                engine.wrapup(channel).await?;
                bridge.idle().await;
                Ok::<(), api::ApiError>(())
            })
            .await;
        self.report_failures();
        match result {
            Some(Err(e)) => self.fail(&e),
            Some(Ok(())) => {
                let written: Vec<FactView> = self
                    .facts(None, FactScope::All)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| !before.contains(&f.id))
                    .collect();
                if written.is_empty() {
                    self.term.note(&self.msg("cli.wrapup.empty"));
                } else {
                    self.term.note(
                        &self
                            .names
                            .say("cli.wrapup.written", &[("n", &written.len())]),
                    );
                    self.print_memories(&written);
                    // So `/forget 1` right after a wrap-up means the first of these.
                    self.listed = written;
                }
            }
            None => {}
        }
        self.footer(at, started).await;
    }

    // ---- memories ----

    async fn facts(&self, person: Option<PersonId>, scope: FactScope) -> ApiResult<Vec<FactView>> {
        let filter = FactFilter {
            scope,
            person,
            task: None,
        };
        Ok(self.engine.facts(filter).await?.items)
    }

    async fn fact_ids(&self) -> Vec<FactId> {
        self.facts(None, FactScope::All)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|f| f.id)
            .collect()
    }

    /// Who may see a memory, in words.
    fn audience(&self, f: &FactView) -> String {
        match &f.audience {
            Audience::World => self.msg("cli.audience.world"),
            Audience::Own { person } => self
                .names
                .say("cli.audience.own", &[("name", &person.name)]),
            Audience::Participants { channel, persons } => {
                let names: Vec<&str> = persons.iter().map(|p| p.name.as_str()).collect();
                self.names.say(
                    "cli.audience.channel",
                    &[("channel", &channel.0), ("names", &names.join(", "))],
                )
            }
        }
    }

    /// A memory's kind in words.
    fn kind_word(&self, m: &FactView) -> String {
        let kind = format!("{:?}", m.kind).to_lowercase();
        self.names.word(&format!("world.kind.{kind}"), &kind)
    }

    /// The line saying who may recall a memory.
    fn seen_by(&self, m: &FactView) -> String {
        self.names
            .say("cli.memories.seen_by", &[("who", &self.audience(m))])
    }

    fn print_memories(&self, memories: &[FactView]) {
        for (i, m) in memories.iter().enumerate() {
            let kind = self.kind_word(m);
            self.term.line(&format!("{}. [{kind}] {}", i + 1, m.text));
            self.term.note(&self.seen_by(m));
        }
    }

    async fn memories(&mut self, who: Option<&str>) {
        let person = match who {
            Some(who) => match self.resolve(who).await {
                Ok(p) => Some(p),
                Err(e) => return self.term.line(&e),
            },
            None => None,
        };
        let mut all = match self.facts(person.clone(), FactScope::All).await {
            Ok(m) => m,
            Err(e) => return self.fail(&e),
        };
        let live = all.len();
        // Deleted ones follow in the same numbering, so `/restore` has something to name.
        all.extend(
            self.facts(person, FactScope::Deleted)
                .await
                .unwrap_or_default(),
        );
        if all.is_empty() {
            return self.term.note(&self.msg("cli.memories.none"));
        }
        for (i, m) in all.iter().enumerate() {
            let kind = self.kind_word(m);
            let line = format!("{}. [{kind}] {}", i + 1, m.text);
            if i < live {
                self.term.line(&line);
                self.term.note(&self.seen_by(m));
            } else {
                self.term.note(&format!(
                    "{line}  {}",
                    self.msg("cli.memories.deleted_mark")
                ));
            }
        }
        self.term.note(&self.names.say(
            "cli.memories.summary",
            &[("live", &live), ("deleted", &(all.len() - live))],
        ));
        self.listed = all;
    }

    async fn change_memory(&mut self, n: usize, change: Change) {
        let Some(m) = self.listed.get(n - 1).cloned() else {
            return self.term.note(&self.msg(if self.listed.is_empty() {
                "cli.memory.no_list"
            } else {
                "cli.memory.no_such"
            }));
        };
        let done = match (&change, m.retracted) {
            (Change::Forget, true) | (Change::Edit(_), true) => {
                return self.term.note(&self.msg("cli.memory.is_deleted"));
            }
            (Change::Restore, false) => {
                return self.term.note(&self.msg("cli.memory.not_deleted"));
            }
            (Change::Forget, false) => self
                .engine
                .delete_fact(m.id)
                .await
                .map(|()| "cli.memory.deleted"),
            (Change::Restore, true) => self
                .engine
                .restore_fact(m.id)
                .await
                .map(|()| "cli.memory.restored"),
            (Change::Edit(text), false) => self
                .engine
                .edit_fact(m.id, text.clone())
                .await
                .map(|_| "cli.memory.rewritten"),
        };
        match done {
            Ok(key) => {
                // Numbers would now point at different memories.
                self.listed.clear();
                self.term.note(&self.msg(key));
            }
            Err(e) => self.fail(&e),
        }
    }

    // ---- reading back ----

    async fn history(&self, n: usize) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        match self.engine.history(channel, None, n as u32).await {
            Ok(messages) if messages.is_empty() => self.term.note(&self.msg("cli.history.none")),
            Ok(messages) => {
                for m in &messages {
                    self.term.print(&self.show.whole(m));
                }
            }
            Err(e) => self.fail(&e),
        }
    }

    /// Messages to look back through for `/resend` and `/sources`.
    const LOOKBACK: u32 = 500;

    /// Take back the user's n-th last message here and hand its text to the input line.
    async fn resend(&mut self, n: usize) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        let messages = match self
            .engine
            .history(channel.clone(), None, Self::LOOKBACK)
            .await
        {
            Ok(m) => m,
            Err(e) => return self.fail(&e),
        };
        let Some(at) = messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, m)| m.author == Author::User)
            .nth(n - 1)
            .map(|(i, _)| i)
        else {
            return self.term.note(&self.msg("cli.resend.none"));
        };
        let removed = messages.len() - at;
        match self.engine.rewind(channel, messages[at].id.clone()).await {
            Ok(text) => {
                self.term.note(&self.names.say(
                    if removed == 1 {
                        "cli.resend.done.one"
                    } else {
                        "cli.resend.done"
                    },
                    &[("n", &removed)],
                ));
                self.prefill = Some(text);
            }
            Err(api::ApiError::Invalid { field }) if field == "busy" => {
                self.term.note(&self.msg("cli.resend.busy"));
            }
            Err(e) => self.fail(&e),
        }
    }

    /// The sources of the n-th last reply that cites any, as a numbered list; with `entry`,
    /// the text of that entry and where it comes from.
    async fn sources(&self, n: usize, entry: Option<usize>) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        let messages = match self.engine.history(channel, None, Self::LOOKBACK).await {
            Ok(m) => m,
            Err(e) => return self.fail(&e),
        };
        let Some(reply) = messages
            .iter()
            .rev()
            .filter(|m| m.author != Author::User && !m.cites.is_empty())
            .nth(n - 1)
        else {
            return self.term.note(&self.msg("cli.sources.none"));
        };
        let mut cites: Vec<&api::types::Citation> = Vec::new();
        for c in &reply.cites {
            if !cites.iter().any(|d| d.chunk_id == c.chunk_id) {
                cites.push(c);
            }
        }
        let Some(k) = entry else {
            let mut out = format!(
                "{}\n",
                self.names.say("cli.sources.header", &[("n", &cites.len())])
            );
            for (i, c) in cites.iter().enumerate() {
                let label = match self.engine.resolve_citation((*c).clone()).await {
                    Ok(v) => v.passage.map(|p| unit_label(&p).to_owned()),
                    Err(_) => None,
                };
                out.push_str(&format!(
                    "  {}. {}\n",
                    i + 1,
                    label.unwrap_or_else(|| c.page.clone())
                ));
            }
            out.push_str(&format!("{}\n", self.msg("cli.sources.hint")));
            return self.term.print(&out);
        };
        let Some(cite) = cites.get(k - 1) else {
            return self.term.note(
                &self
                    .names
                    .say("cli.sources.no_entry", &[("k", &k), ("n", &cites.len())]),
            );
        };
        match self.engine.resolve_citation((*cite).clone()).await {
            Ok(v) => {
                let Some(p) = v.passage else {
                    return self.term.note(&self.msg("cli.sources.gone"));
                };
                self.term.print(&format!(
                    "{}\n{}\n",
                    self.term.style.bold(unit_label(&p)),
                    p.text
                ));
                if let Some(url) = v.source_url.or(v.revision_url) {
                    self.term
                        .line(&self.names.say("cli.source.link", &[("url", &url)]));
                }
            }
            Err(e) => self.fail(&e),
        }
    }

    /// Whose retrieval `/search` runs as: the person of a direct channel, the last person
    /// who spoke in a group (else its first member), nobody for the host.
    async fn speaker(&self, c: &Current) -> Option<PersonId> {
        match &c.kind {
            Kind::Person(p) => Some(p.clone()),
            Kind::Host => None,
            Kind::Group(members) => {
                let last = self
                    .engine
                    .history(c.channel.clone(), None, 50)
                    .await
                    .ok()
                    .and_then(|h| {
                        h.iter().rev().find_map(|m| match &m.author {
                            Author::Person { id } => Some(id.clone()),
                            _ => None,
                        })
                    });
                last.or_else(|| members.first().cloned())
            }
        }
    }

    async fn search(&mut self, query: &str) {
        // Without a channel (the library form) the whole corpus is searched.
        let as_person = match &self.current {
            Some(c) => self.speaker(c).await,
            None => None,
        };
        let found = self
            .engine
            .search_corpus(
                query.to_owned(),
                as_person.iter().cloned().collect(),
                Vec::new(),
            )
            .await;
        let found = match found {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        match &as_person {
            Some(p) => self.term.note(
                &self
                    .names
                    .say("cli.search.as", &[("name", &self.names.person(&p.0))]),
            ),
            None => self.term.note(&self.msg("cli.search.as_host")),
        }
        let confidence = format!("{:?}", found.confidence).to_lowercase();
        self.term.note(
            &self
                .names
                .say("cli.search.confidence", &[("level", &confidence)]),
        );
        for (i, hit) in found.hits.iter().enumerate() {
            self.term.print(&format!(
                "{}. {} {}\n",
                i + 1,
                unit_label(hit),
                self.term.style.dim(&format!("({})", hit.citation.chunk_id)),
            ));
            self.term
                .print(&format!("   {}\n", excerpt(&hit.text, 100)));
        }
        if found.hits.is_empty() {
            self.term.note(&self.msg("cli.search.none"));
        } else {
            self.term.note(&self.msg("cli.search.hint"));
        }
        self.found = found.hits;
    }

    /// Show a unit of the corpus with the units next to it and the link to its source:
    /// `what` is a number from the last `/search`, or a unit id.
    async fn read_unit(&self, what: &str) {
        let id = match what.parse::<usize>() {
            Ok(n) => match self.found.get(n.wrapping_sub(1)) {
                Some(p) => p.citation.chunk_id.clone(),
                None => {
                    return self.term.note(&self.msg(if self.found.is_empty() {
                        "cli.read.no_results"
                    } else {
                        "cli.read.no_such"
                    }));
                }
            },
            Err(_) => what.to_owned(),
        };
        let view = match self.engine.read_unit(id, 2).await {
            Ok(v) => v,
            Err(api::ApiError::NotFound) => {
                return self
                    .term
                    .note(&self.names.say("cli.read.no_unit", &[("what", &what)]));
            }
            Err(e) => return self.fail(&e),
        };
        let style = self.term.style;
        for p in &view.before {
            self.term
                .print(&format!("{}\n", style.dim(&excerpt(&p.text, 160))));
        }
        self.term.print(&format!(
            "{} {}\n{}\n",
            style.bold(unit_label(&view.unit)),
            style.dim(&format!("({})", view.unit.citation.chunk_id)),
            view.unit.text
        ));
        for p in &view.after {
            self.term
                .print(&format!("{}\n", style.dim(&excerpt(&p.text, 160))));
        }
        if let Some(url) = &view.unit.source_url {
            self.term
                .line(&self.names.say("cli.source.link", &[("url", url)]));
        }
    }

    /// Say plainly that a command needs a language model.
    fn needs_model(&self) {
        self.term.print(&format!(
            "{}\n",
            self.names.text(if self.no_model {
                "cli.library.needs_model"
            } else {
                "cli.library.needs_model_off"
            })
        ));
    }

    /// `/library`: the corpus without a language model; `/library off` goes back to the
    /// conversations when a model is set up.
    fn set_library(&mut self, on: bool) {
        if on {
            self.library = true;
            return self.term.note(&self.names.text("cli.library.on"));
        }
        if self.no_model {
            return self.term.note(&self.names.text("cli.library.stays"));
        }
        self.library = false;
        self.term.note(&self.names.text("cli.library.off"));
    }

    /// The one line said at start when it opens in library mode.
    pub fn library_notice(&self, chosen: bool) -> Option<String> {
        self.library.then(|| {
            self.names.text(if chosen {
                "cli.library.on"
            } else {
                "cli.library.start"
            })
        })
    }

    /// Say whether a chat endpoint with a key is missing; if so the session is a library.
    pub fn set_no_model(&mut self, missing: bool) {
        self.no_model = missing;
        self.library = missing;
    }

    /// Start in library mode on purpose (`--library`).
    pub fn force_library(&mut self) {
        self.library = true;
    }

    // ---- encryption ----

    /// The new password, typed twice.
    fn new_password(&self) -> Option<String> {
        let first = (self.secret)(&self.msg("cli.password.new"));
        let again = first
            .as_ref()
            .and_then(|_| (self.secret)(&self.msg("cli.password.again")));
        match (first, again) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(_), Some(_)) => {
                self.term.line(&self.msg("cli.password.mismatch"));
                None
            }
            _ => {
                self.term.note(&self.msg("cli.cancelled"));
                None
            }
        }
    }

    async fn set_password(&self, old: Option<String>, new: Option<String>, done: &str) {
        match self.engine.set_password(old, new).await {
            Ok(()) => self.term.note(&self.msg(done)),
            Err(e) => self.term.line(&self.why(&e)),
        }
    }

    /// `/password`: whether the open save is encrypted, or turn it on, change it, off.
    async fn password(&mut self, act: Option<PasswordAct>) {
        let encrypted = self
            .engine
            .saves()
            .await
            .unwrap_or_default()
            .iter()
            .any(|s| s.current && s.encrypted);
        match (act, encrypted) {
            (None, true) => self.term.note(&self.msg("cli.password.is_on")),
            (None, false) => self.term.note(&self.msg("cli.password.is_off")),
            (Some(PasswordAct::On), false) => {
                self.term.line(&self.msg("cli.password.warning"));
                self.confirm(self.msg("cli.password.ask_on"), Pending::PasswordOn);
            }
            (Some(PasswordAct::Off), true) => {
                self.confirm(self.msg("cli.password.ask_off"), Pending::PasswordOff);
            }
            (Some(PasswordAct::Change), true) => {
                let Some(old) = (self.secret)(&self.msg("cli.password.current")) else {
                    return self.term.note(&self.msg("cli.cancelled"));
                };
                let Some(new) = self.new_password() else {
                    return;
                };
                self.set_password(Some(old), Some(new), "cli.password.changed")
                    .await;
            }
            (Some(PasswordAct::On), true)
            | (Some(PasswordAct::Change | PasswordAct::Off), false) => {
                self.term.line(&self.msg(if encrypted {
                    "cli.password.already_on"
                } else {
                    "cli.password.not_on"
                }))
            }
        }
    }

    /// `/bio`: what the people are told about you.
    async fn bio(&self, text: Option<String>) {
        let settings = match self.engine.settings().await {
            Ok(s) => s,
            Err(e) => return self.fail(&e),
        };
        let Some(text) = text else {
            return self.term.note(&match settings.bio {
                Some(b) => self.names.say("cli.bio.show", &[("bio", &b)]),
                None => self.msg("cli.bio.none"),
            });
        };
        let next = Settings {
            bio: Some(text).filter(|t| !t.is_empty()),
            ..settings
        };
        match self.engine.update_settings(next.clone()).await {
            // A change rolls the channels over, which may ask the model for a summary.
            Ok(()) => {
                self.interruptible(self.bridge.idle()).await;
                self.term.note(&self.msg(if next.bio.is_some() {
                    "cli.bio.saved"
                } else {
                    "cli.bio.cleared"
                }))
            }
            Err(api::ApiError::Invalid { .. }) => self.term.line(
                &self
                    .names
                    .say("cli.bio.too_long", &[("max", &world::settings::BIO_MAX)]),
            ),
            Err(e) => self.fail(&e),
        }
    }

    // ---- help and settings ----

    /// `/help`: the groups in the corpus's wording, then how to send.
    fn help(&self) -> String {
        let mut groups: Vec<String> = [
            "conversation",
            "channels",
            "people",
            "memories",
            "saves",
            "settings",
            "other",
        ]
        .iter()
        .map(|g| self.names.text(&format!("cli.help.{g}")))
        .collect();
        groups.push(self.names.text("cli.typing"));
        groups.join("\n\n")
    }

    /// `/lang`: the words of the interface, the corpus's language or English. Applies at
    /// once; the home keeps it for the app too.
    async fn lang(&self, arg: Option<&str>) {
        let chosen = match arg.map(|a| a.trim().to_lowercase()).as_deref() {
            None => None,
            Some("en" | "english") => Some(InterfaceLanguage::English),
            Some("corpus") => Some(InterfaceLanguage::Corpus),
            Some(_) => return self.term.line(&self.msg("cli.usage.lang")),
        };
        if let Some(language) = chosen {
            let done = async {
                self.engine.set_interface_language(language).await?;
                self.names.reload(&self.engine).await
            }
            .await;
            if let Err(e) = done {
                return self.fail(&e);
            }
        }
        let current = self.engine.interface_language().await.unwrap_or_default();
        let name = match current {
            InterfaceLanguage::Corpus => self.names.word("lang.corpus", "corpus language"),
            InterfaceLanguage::English => "English".to_owned(),
        };
        self.term.note(&self.names.say(
            if chosen.is_some() {
                "cli.lang.set"
            } else {
                "cli.lang.current"
            },
            &[("language", &name)],
        ));
    }

    async fn tier(&self, level: Option<&str>) {
        const LEVELS: [(&str, Tier); 6] = [
            ("low", Tier::Low),
            ("middle", Tier::Middle),
            ("high", Tier::High),
            ("extra_high", Tier::ExtraHigh),
            ("max", Tier::Max),
            ("ultra", Tier::Ultra),
        ];
        let name = |t: Tier| LEVELS.iter().find(|(_, l)| *l == t).map_or("", |(n, _)| *n);
        if let Some(level) = level {
            let want = level.trim().to_lowercase().replace(['-', ' '], "_");
            let Some((_, tier)) = LEVELS.iter().find(|(n, _)| *n == want) else {
                let all: Vec<&str> = LEVELS.iter().map(|(n, _)| *n).collect();
                return self.term.line(
                    &self
                        .names
                        .say("cli.tier.levels", &[("levels", &all.join(", "))]),
                );
            };
            if let Err(e) = self.engine.set_tier(*tier).await {
                return self.fail(&e);
            }
        }
        match self.engine.quota().await {
            Ok(q) => self.term.note(&self.names.say(
                "cli.tier.status",
                &[
                    ("tier", &name(q.tier)),
                    ("band", &format!("{:?}", q.band).to_lowercase()),
                ],
            )),
            Err(e) => self.fail(&e),
        }
    }

    async fn quiet(&self, arg: Option<&str>) {
        let settings = match self.engine.settings().await {
            Ok(s) => s,
            Err(e) => return self.fail(&e),
        };
        let quiet = match arg.map(|a| a.trim().to_lowercase()).as_deref() {
            None => settings.quiet_hours.clone(),
            Some("off") => None,
            Some("on") => settings.quiet_hours.clone().or_else(|| {
                // Back to the register's hours, which the engine would show on a fresh world.
                let d = &self.engine.params().world.seed.quiet_hours;
                Some(QuietHours {
                    from: d.from.clone(),
                    to: d.to.clone(),
                })
            }),
            Some(range) => match range.split_once('-') {
                Some((from, to)) => Some(QuietHours {
                    from: from.trim().into(),
                    to: to.trim().into(),
                }),
                None => return self.term.line(&self.msg("cli.usage.quiet")),
            },
        };
        if arg.is_some() {
            let next = Settings {
                quiet_hours: quiet.clone(),
                ..settings
            };
            if let Err(e) = self.engine.update_settings(next).await {
                return match e {
                    api::ApiError::Invalid { .. } => self.term.line(&self.msg("cli.quiet.invalid")),
                    e => self.fail(&e),
                };
            }
        }
        self.term.note(&match quiet {
            Some(q) => self
                .names
                .say("cli.quiet.on", &[("from", &q.from), ("to", &q.to)]),
            None => self.msg("cli.quiet.off"),
        });
    }

    /// The daily events: how many a day, and the last few. A number or `on`/`off` sets it.
    async fn events(&self, arg: Option<&str>) {
        let settings = match self.engine.settings().await {
            Ok(s) => s,
            Err(e) => return self.fail(&e),
        };
        let params = &self.engine.params().world.events;
        let n = match arg.map(|a| a.trim().to_lowercase()).as_deref() {
            None => settings.events_per_day,
            Some("off") => 0,
            Some("on") if settings.events_per_day > 0 => settings.events_per_day,
            Some("on") => params.per_day.max(1),
            Some(n) => match n.parse::<u32>() {
                Ok(n) if n <= params.max_per_day => n,
                _ => return self.term.line(&self.msg("cli.usage.events")),
            },
        };
        if arg.is_some() && n != settings.events_per_day {
            let next = Settings {
                events_per_day: n,
                ..settings
            };
            if let Err(e) = self.engine.update_settings(next).await {
                return self.fail(&e);
            }
        }
        self.term.note(&match n {
            0 => self.msg("cli.events.off"),
            n => self.names.say("cli.events.on", &[("n", &n.to_string())]),
        });
        if arg.is_some() {
            return;
        }
        match self.engine.daily_events().await {
            Ok(events) if events.is_empty() => self.term.note(&self.msg("cli.events.none")),
            Ok(events) => {
                for e in events {
                    let who: Vec<_> = e.persons.iter().map(|p| p.name.as_str()).collect();
                    self.term
                        .line(&format!("{}  {} ({})", e.day, e.title, who.join(", ")));
                    self.term.line(&format!("    {}", e.text));
                }
            }
            Err(e) => self.fail(&e),
        }
    }

    // ---- saves ----

    /// What a save command's failure means in a terminal.
    fn why(&self, e: &api::ApiError) -> String {
        let key = match e {
            api::ApiError::Busy => "cli.save.busy",
            api::ApiError::OpenElsewhere => "cli.save.open_elsewhere",
            api::ApiError::Exists => "cli.save.exists",
            api::ApiError::Locked => "cli.save.locked",
            api::ApiError::WrongPassword => "cli.save.wrong_password",
            api::ApiError::Invalid { field } if field == "new" => "cli.save.bad_password",
            api::ApiError::NotFound => "cli.save.not_found",
            api::ApiError::Invalid { field } if field == "name" => "cli.save.bad_name",
            api::ApiError::Invalid { field } if field == "file" => "cli.save.bad_file",
            other => return self.names.api_error(other),
        };
        self.msg(key)
    }

    async fn save(&mut self, cmd: Save) {
        let done = match cmd {
            Save::List => return self.list_saves().await,
            Save::New(n) => self
                .engine
                .new_save(n.clone())
                .await
                .map(|()| self.names.say("cli.save.created", &[("n", &n)])),
            Save::Switch(n) => return self.switch_save(&n).await,
            Save::Logout => return self.log_out().await,
            Save::Copy(a, b) => self
                .engine
                .copy_save(a.clone(), b.clone())
                .await
                .map(|()| self.names.say("cli.save.copied", &[("a", &a), ("b", &b)])),
            Save::Rename(a, b) => self
                .engine
                .rename_save(a.clone(), b.clone())
                .await
                .map(|()| self.names.say("cli.save.renamed", &[("a", &a), ("b", &b)])),
            Save::Delete(n) => return self.ask_delete(n).await,
            Save::Export(n, path) => return self.export_save(n, path).await,
            Save::Import(path, name) => self
                .import(expand(&path), name)
                .await
                .map(|n| self.names.say("cli.save.imported", &[("n", &n)])),
        };
        match done {
            Ok(text) => self.term.note(&text),
            Err(e) => self.term.line(&self.why(&e)),
        }
    }

    async fn list_saves(&self) {
        let saves = match self.engine.saves().await {
            Ok(s) => s,
            Err(e) => return self.fail(&e),
        };
        for s in &saves {
            let played = s.played_at.as_ref().map_or_else(
                || "-".to_owned(),
                |t| t.0.chars().take(16).collect::<String>().replace('T', " "),
            );
            self.term.line(&format!(
                "{} {:<24} {:>8} KB  {played}{}",
                if s.current { "*" } else { " " },
                s.name,
                s.size_kb,
                self.encrypted_mark(s.encrypted)
            ));
        }
    }

    /// The mark after an encrypted save in a list; nothing otherwise.
    fn encrypted_mark(&self, encrypted: bool) -> String {
        if encrypted {
            format!("  {}", self.msg("cli.save.encrypted"))
        } else {
            String::new()
        }
    }

    /// Close this world and open another: what the session held about the old one (the
    /// channel, the choices the host offered, the numbered memories, the call counts) goes.
    async fn switch_save(&mut self, name: &str) {
        let mut switched = self.engine.switch_save(name.to_owned()).await;
        if switched == Err(api::ApiError::Locked) {
            if !self.unlock(name).await {
                return;
            }
            switched = self.engine.switch_save(name.to_owned()).await;
        }
        if let Err(e) = switched {
            return self.term.line(&self.why(&e));
        }
        self.opened(name).await;
    }

    /// A save is open now: nothing of the last one carries over.
    async fn opened(&mut self, name: &str) {
        self.current = None;
        self.offers.clear();
        self.listed.clear();
        self.calls.clear();
        self.metered = self
            .bridge
            .calls_after(0)
            .await
            .ok()
            .and_then(|c| c.last().map(|c| c.id))
            .unwrap_or(0);
        if let Err(e) = self.names.refresh_user(&self.engine).await {
            self.fail(&e);
        }
        self.term
            .note(&self.names.say("cli.save.playing", &[("name", &name)]));
    }

    /// Whether another process replaced, renamed, deleted or encrypted the open save's
    /// file under this one. Says so, then reopens it (asking for the password when it is
    /// encrypted now) or, when it cannot be, goes back to the list. True when it did
    /// either, so the line typed is dropped rather than sent to a different save.
    async fn save_changed(&mut self) -> bool {
        if self.picking || !self.engine.check_open_file() {
            return false;
        }
        self.pending = None;
        let name = match self.engine.app_status().await {
            Ok(status) => status.save.unwrap_or_default(),
            Err(_) => String::new(),
        };
        self.term.line(&self.msg("cli.save.changed"));
        let mut reopened = match self.engine.switch_save(name.clone()).await {
            Ok(()) => true,
            Err(e) => {
                self.term.line(&self.why(&e));
                false
            }
        };
        let locked = reopened
            && self
                .engine
                .app_status()
                .await
                .is_ok_and(|status| status.locked);
        if locked {
            reopened = self.unlock(&name).await;
        }
        if reopened {
            self.opened(&name).await;
        } else {
            self.log_out().await;
        }
        true
    }

    /// Whether no save is open and the lines typed choose one.
    pub fn picking(&self) -> bool {
        self.picking
    }

    /// `/save logout`: close the open save, forget where it was, and show the save list.
    async fn log_out(&mut self) {
        if let Err(e) = self.engine.log_out().await {
            return self.term.line(&self.why(&e));
        }
        self.current = None;
        self.offers.clear();
        self.listed.clear();
        self.calls.clear();
        self.picking = true;
        self.term.note(&self.msg("cli.save.logged_out"));
        self.show_picker().await;
    }

    /// The numbered list of saves and what can be typed. Also the first thing a start
    /// without a usable last save shows, with the reason.
    pub async fn show_picker(&mut self) {
        let status = self.engine.app_status().await.ok();
        if let Some(gone) = status.as_ref().and_then(|s| s.missing_save.as_ref()) {
            self.term
                .line(&self.names.say("cli.picker.gone", &[("gone", gone)]));
        }
        let saves = self.engine.saves().await.unwrap_or_default();
        let dir = status.map_or_else(String::new, |s| s.saves_dir);
        self.term
            .line(&self.names.say("cli.picker.header", &[("dir", &dir)]));
        if saves.is_empty() {
            self.term.line(&self.msg("cli.picker.none"));
        }
        for (i, s) in saves.iter().enumerate() {
            let played = s.played_at.as_ref().map_or_else(
                || "-".to_owned(),
                |t| t.0.chars().take(16).collect::<String>().replace('T', " "),
            );
            self.term.line(&format!(
                "  {:>2}  {:<24} {:>8} KB  {played}{}",
                i + 1,
                s.name,
                s.size_kb,
                self.encrypted_mark(s.encrypted)
            ));
        }
        self.pickable = saves.into_iter().map(|s| s.name).collect();
        self.term.line(&self.msg("cli.picker.help"));
    }

    /// One line typed while no save is open.
    async fn pick(&mut self, line: &str) -> Flow {
        let words: Vec<String> = super::command::words(line.trim().trim_start_matches('/'));
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let chosen = match words.as_slice() {
            [] => {
                self.show_picker().await;
                return Flow::Continue;
            }
            ["quit" | "exit" | "q"] => return Flow::Quit,
            ["new", name] => match self.engine.new_save((*name).into()).await {
                Ok(()) => (*name).to_owned(),
                Err(e) => return self.picker_error(&e),
            },
            ["import", path] | ["import", path, _] => {
                let name = words.get(2).map(|n| (*n).to_owned());
                match self.import(expand(path), name).await {
                    Ok(name) => name,
                    Err(e) => return self.picker_error(&e),
                }
            }
            [n] if n.parse::<usize>().is_ok() => match n
                .parse::<usize>()
                .ok()
                .and_then(|n| self.pickable.get(n.checked_sub(1)?))
            {
                Some(name) => name.clone(),
                None => {
                    self.term.line(&self.msg("cli.picker.no_number"));
                    return Flow::Continue;
                }
            },
            [name] if self.pickable.iter().any(|s| s == name) => (*name).to_owned(),
            _ => {
                self.term.line(&self.msg("cli.picker.reminder"));
                return Flow::Continue;
            }
        };
        self.switch_save(&chosen).await;
        let opened = self
            .engine
            .app_status()
            .await
            .is_ok_and(|s| s.save.is_some());
        if opened {
            self.picking = false;
            self.refresh_completions().await;
            self.arrivals(true).await;
        } else {
            self.show_picker().await;
        }
        Flow::Continue
    }

    fn picker_error(&self, e: &api::ApiError) -> Flow {
        self.term.line(&self.why(e));
        Flow::Continue
    }

    /// Ask for the password of save `name` until it opens it or the tries run out.
    async fn unlock(&self, name: &str) -> bool {
        for _ in 0..TRIES {
            let Some(password) =
                (self.secret)(&self.names.say("cli.password.for", &[("name", &name)]))
            else {
                self.term.note(&self.msg("cli.cancelled"));
                return false;
            };
            match self.engine.unlock(name.to_owned(), password).await {
                Ok(()) => return true,
                Err(api::ApiError::WrongPassword) => {
                    self.term.line(&self.msg("cli.save.wrong_password"))
                }
                Err(e) => {
                    self.term.line(&self.why(&e));
                    return false;
                }
            }
        }
        false
    }

    /// Import a file, asking for its password when it is encrypted.
    async fn import(&self, path: String, name: Option<String>) -> api::error::ApiResult<String> {
        let mut result = self
            .engine
            .import_save(path.clone(), name.clone(), None)
            .await;
        for _ in 0..TRIES {
            if result != Err(api::ApiError::Locked) && result != Err(api::ApiError::WrongPassword) {
                break;
            }
            if result == Err(api::ApiError::WrongPassword) {
                self.term.line(&self.msg("cli.save.wrong_password"));
            }
            let Some(password) = (self.secret)(&self.msg("cli.password.file")) else {
                break;
            };
            result = self
                .engine
                .import_save(path.clone(), name.clone(), Some(password))
                .await;
        }
        result
    }

    async fn ask_delete(&mut self, name: String) {
        let in_use = self
            .engine
            .saves()
            .await
            .unwrap_or_default()
            .iter()
            .any(|s| s.current && s.name == name);
        if in_use {
            return self.term.line(&self.msg("cli.delete.in_use"));
        }
        self.confirm(
            self.names.say("cli.delete.ask", &[("name", &name)]),
            Pending::DeleteSave(name),
        );
    }

    async fn export_save(&mut self, name: String, path: String) {
        let path = expand(&path);
        if std::path::Path::new(&path).exists() {
            self.confirm(
                self.names.say("cli.export.replace", &[("path", &path)]),
                Pending::Overwrite { name, path },
            );
            return;
        }
        self.export_now(&name, path).await;
    }

    async fn export_now(&self, name: &str, path: String) {
        match self.engine.export_save(name.to_owned(), path.clone()).await {
            Ok(()) => self
                .term
                .note(&self.names.say("cli.export.wrote", &[("path", &path)])),
            Err(e) => self.term.line(&self.why(&e)),
        }
    }

    async fn confirmed(&mut self, pending: Pending) {
        match pending {
            Pending::DeleteSave(name) => match self.engine.delete_save(name.clone()).await {
                Ok(()) => self
                    .term
                    .note(&self.names.say("cli.delete.done", &[("name", &name)])),
                Err(e) => self.term.line(&self.why(&e)),
            },
            Pending::DeleteGroup(channel) => {
                match self.engine.delete_group(channel.clone()).await {
                    Ok(()) => {
                        if self.current.as_ref().is_some_and(|c| c.channel == channel) {
                            self.current = None;
                        }
                        self.term.note(&self.msg("cli.leave.done"));
                    }
                    Err(e) => self.fail(&e),
                }
            }
            Pending::Overwrite { name, path } => self.export_now(&name, path).await,
            Pending::ClearStorage(kinds) => match self.engine.clear_storage(kinds).await {
                Ok(usage) => {
                    self.term.note(&self.msg("cli.storage.cleared"));
                    self.print_storage(&usage);
                }
                Err(e) => self.term.line(&self.why(&e)),
            },
            Pending::PasswordOn => {
                let Some(new) = self.new_password() else {
                    return;
                };
                self.set_password(None, Some(new), "cli.password.now_on")
                    .await;
            }
            Pending::PasswordOff => {
                let Some(old) = (self.secret)(&self.msg("cli.password.plain")) else {
                    return self.term.note(&self.msg("cli.cancelled"));
                };
                self.set_password(Some(old), None, "cli.password.now_off")
                    .await;
            }
        }
    }

    // ---- people coming to you ----

    /// What the app does on a timer, here at the two moments the terminal is awake: when
    /// it opens (`opened`: presence, which may bring the host's line and one person's
    /// opening) and after each message. Idle conversations are wrapped up and due
    /// openings written; anything that arrives is printed by the event sink.
    pub async fn arrivals(&mut self, opened: bool) {
        // Presence and ticks write with a model; the library form never calls one.
        if self.library {
            return;
        }
        let (started, at) = (Instant::now(), now_ms());
        let (engine, bridge) = (self.engine.clone(), self.bridge.clone());
        let ran = self
            .interruptible(async move {
                if opened {
                    engine.presence().await?;
                    bridge.idle().await;
                }
                bridge.tick(true).await;
                bridge.idle().await;
                Ok::<(), api::ApiError>(())
            })
            .await;
        if let Some(Err(e)) = ran {
            self.fail(&e);
        }
        self.report_failures();
        self.offer();
        self.footer(at, started).await;
    }

    async fn storage(&mut self, act: StorageAct) {
        match act {
            StorageAct::Show => match self.engine.storage_usage().await {
                Ok(usage) => self.print_storage(&usage),
                Err(e) => self.term.line(&self.why(&e)),
            },
            StorageAct::Clear(kinds) => {
                let usage = match self.engine.storage_usage().await {
                    Ok(u) => u,
                    Err(e) => return self.term.line(&self.why(&e)),
                };
                let kb: u32 = usage
                    .categories
                    .iter()
                    .filter(|c| kinds.contains(&c.kind))
                    .map(|c| c.kb)
                    .sum();
                self.confirm(
                    self.names.say("cli.storage.ask", &[("size", &size(kb))]),
                    Pending::ClearStorage(kinds),
                );
            }
        }
    }

    fn print_storage(&self, usage: &StorageUsage) {
        let label = |k: StorageKind| match k {
            StorageKind::Saves => self.names.word("storage.kind.saves", "saves"),
            StorageKind::Avatars => self.names.word("storage.kind.avatars", "avatars"),
            StorageKind::Audio => self.names.word("storage.kind.audio", "audio"),
            StorageKind::Uploads => self.names.word("storage.kind.uploads", "uploads"),
            StorageKind::Corpus => self.names.word("storage.kind.corpus", "corpus"),
        };
        for c in &usage.categories {
            let note = if c.clearable {
                format!("  {}", self.msg("cli.storage.clearable"))
            } else {
                String::new()
            };
            self.term.line(&format!(
                "{:<8} {:>10}  {}{note}",
                label(c.kind),
                size(c.kb),
                self.names.say("cli.storage.files", &[("n", &c.files)])
            ));
        }
        for s in &usage.saves {
            let mark = if s.current { " *" } else { "" };
            self.term.line(&self.names.say(
                "cli.storage.save",
                &[
                    ("name", &format!("{}{mark}", s.name)),
                    ("size", &size(s.kb)),
                ],
            ));
        }
        self.term.line(&self.names.say(
            "cli.storage.pictures",
            &[("n", &usage.pictures), ("size", &size(usage.picture_kb))],
        ));
        self.term.line(&self.names.say(
            "cli.storage.total",
            &[("size", &format!("{:>10}", size(usage.total_kb)))],
        ));
    }

    fn stats(&self) {
        if self.calls.is_empty() {
            return self.term.note(&self.msg("cli.stats.none"));
        }
        let head = |k: &str| self.msg(k);
        self.term.line(&format!(
            "{:>3}  {:<10} {:<28} {:>9} {:>9} {:>7} {:>6}",
            "#",
            head("cli.stats.shape"),
            head("cli.stats.model"),
            head("cli.stats.uncached"),
            head("cli.stats.cached"),
            head("cli.stats.output"),
            head("cli.stats.secs")
        ));
        let (mut u, mut c, mut o, mut s) = (0, 0, 0, 0.0);
        for (i, t) in self.calls.iter().enumerate() {
            let call = &t.call;
            self.term.print(&format!(
                "{:>3}  {:<10} {:<28} {:>9} {:>9} {:>7} {:>6.1}\n",
                i + 1,
                call.shape,
                call.model,
                call.uncached,
                call.cached,
                call.output,
                t.secs
            ));
            u += call.uncached;
            c += call.cached;
            o += call.output;
            s += t.secs;
        }
        self.term.print(&format!(
            "{:>3}  {:<10} {:<28} {:>9} {:>9} {:>7} {:>6.1}\n",
            "",
            self.msg("cli.stats.total"),
            "",
            u,
            c,
            o,
            s
        ));
    }
}
