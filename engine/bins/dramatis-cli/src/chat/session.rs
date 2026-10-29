//! The state of one terminal session and what each command does with it. Everything it
//! shows or does is a seam command the app issues too; the one exception is `/stats`,
//! which reads the meter through the bridge.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use api::error::ApiResult;
use api::types::{Author, ChannelId, FactId, PersonId, Tier};
use api::views::{Audience, FactFilter, FactScope, FactView, QuietHours, RosterRow, Settings};
use api::{Api, Engine};
use runtime::{AgentBridge, Call};

use super::command::{Command, PasswordAct, Save, parse};
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
    /// What the next line answers (y/n), when a command asked.
    pending: Option<Pending>,
    /// Asks for a password with nothing echoed; `None` is no answer. Replaced in tests.
    secret: Secret,
    /// No save is open (first run, the last one is gone, or the user logged out): every
    /// line answers the save list until one opens.
    picking: bool,
    /// The saves of the list on screen, numbered from 1.
    pickable: Vec<String>,
}

/// Asks for a password and returns what was typed.
type Secret = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What `/forget`, `/restore` and `/edit` do to a listed memory.
enum Change {
    Forget,
    Restore,
    Edit(String),
}

/// A destructive step waiting for a yes.
enum Pending {
    DeleteSave(String),
    Overwrite { name: String, path: String },
    PasswordOn,
    PasswordOff,
}

/// How many times a password is asked for before giving up.
const TRIES: usize = 3;

/// A password cannot be recovered; the person turning it on must hear that first.
const FORGET_WARNING: &str =
    "A forgotten password cannot be recovered: nobody can open this save without it.\n";

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
            pending: None,
            secret: Box::new(super::secret::read),
            picking: false,
            pickable: Vec::new(),
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
        if self.picking {
            return format!("{} ", self.term.style.bold("save>"));
        }
        let channel = self.current.as_ref().map_or("-", |c| c.label.as_str());
        format!(
            "{} {}> ",
            style.dim(&format!("[{channel}]")),
            style.bold(&self.names.user())
        )
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
            self.term.error(&e.to_string());
            return false;
        }
        // The rollover may ask the model for a summary.
        self.interruptible(self.bridge.idle()).await;
        self.report_failures();
        true
    }

    /// One line of input: the answer to a question just asked, else a command or message.
    pub async fn input(&mut self, line: &str) -> Flow {
        if self.picking {
            return self.pick(line).await;
        }
        let Some(pending) = self.pending.take() else {
            return self.handle(parse(line)).await;
        };
        if matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            self.confirmed(pending).await;
            self.refresh_completions().await;
        } else {
            self.term.note("cancelled");
        }
        Flow::Continue
    }

    pub async fn handle(&mut self, command: Command) -> Flow {
        match command {
            Command::Noop => {}
            Command::Quit => return Flow::Quit,
            Command::Help => self.term.print(&format!("{}\n", self.help())),
            Command::Invalid(text) => self.term.print(&format!("{text}\n")),
            Command::List(filter) => self.list(filter.as_deref()).await,
            Command::Talk(who) => self.talk(&who).await,
            Command::Group(who) => self.group(&who).await,
            Command::Host => self.host().await,
            Command::Go(n) => self.go(n).await,
            Command::Name(name) => {
                if self.set_user(&name).await {
                    self.term.note(&format!("you are called {name} now"));
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
            Command::Tier(level) => self.tier(level.as_deref()).await,
            Command::Quiet(arg) => self.quiet(arg.as_deref()).await,
            Command::Save(cmd) => {
                self.save(cmd).await;
                self.refresh_completions().await;
            }
            Command::History(n) => self.history(n).await,
            Command::Resend(n) => self.resend(n).await,
            Command::Sources(n) => self.sources(n).await,
            Command::Search(query) => self.search(&query).await,
            Command::Stats => self.stats(),
        }
        Flow::Continue
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
            Err(e) => return self.term.error(&e.to_string()),
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
            Err(e) => return self.term.error(&e.to_string()),
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
            Err(e) => return self.term.error(&e.to_string()),
        };
        let needle = filter.map(str::to_lowercase);
        let host = if self.names.has_host() {
            "available"
        } else {
            "not available"
        };
        self.term
            .print(&format!("host: {} ({host})\n", self.names.host()));
        let mut shown = 0;
        for e in roster.iter().filter(|e| e.persona) {
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
        self.term.note(&format!("{shown} persons with a persona"));
    }

    /// Resolve what the user typed to one person with a persona: id, display name, another
    /// form's name, an alias the corpus records, then a unique fragment.
    async fn resolve(&self, query: &str) -> Result<PersonId, String> {
        let roster = self.roster().await.map_err(|e| e.to_string())?;
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
        let with_persona: Vec<&Entry> = found.iter().copied().filter(|e| e.persona).collect();
        match (found.len(), with_persona.as_slice()) {
            (0, _) => Err(format!(
                "nobody matches \"{q}\"; /list shows who can be talked to"
            )),
            (_, []) => Err(format!(
                "{} has no persona, so cannot be talked to",
                found[0].display
            )),
            (_, [one]) => Ok(PersonId(one.id.clone())),
            (_, many) => Err(format!(
                "\"{q}\" is ambiguous: {}",
                many.iter()
                    .take(8)
                    .map(|e| format!("{} ({})", e.display, e.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    async fn talk(&mut self, who: &str) {
        match self.resolve(who).await {
            Ok(person) => self.open_person(person).await,
            Err(e) => self.term.print(&format!("{e}\n")),
        }
    }

    async fn open_person(&mut self, person: PersonId) {
        match self.engine.open_direct(person.clone()).await {
            Ok(channel) => {
                let name = self.names.person(&person.0);
                self.term.note(&format!(
                    "talking to {name} ({}); /history shows earlier lines",
                    person.0
                ));
                self.current = Some(Current {
                    channel,
                    kind: Kind::Person(person),
                    label: name,
                });
            }
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    async fn group(&mut self, who: &[String]) {
        let mut members: Vec<PersonId> = Vec::new();
        for w in who {
            match self.resolve(w).await {
                Ok(p) if !members.contains(&p) => members.push(p),
                Ok(_) => {}
                Err(e) => return self.term.print(&format!("{e}\n")),
            }
        }
        if members.len() < 2 {
            return self
                .term
                .print("a group needs at least two different persons\n");
        }
        match self.engine.create_group(members.clone(), None).await {
            Ok(channel) => self.enter_group(channel, members),
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    fn enter_group(&mut self, channel: ChannelId, members: Vec<PersonId>) {
        let names: Vec<String> = members.iter().map(|p| self.names.person(&p.0)).collect();
        let label = self.names.group_label(&names);
        self.term.note(&format!("group with {label}"));
        self.current = Some(Current {
            channel,
            kind: Kind::Group(members),
            label,
        });
    }

    /// Switch to choice `n` of the host's latest offer.
    async fn go(&mut self, n: usize) {
        let Some(offer) = self.offers.get(n - 1).cloned() else {
            return self.term.print(if self.offers.is_empty() {
                "the host has offered nothing to go to\n"
            } else {
                "no such choice; the host's list is above\n"
            });
        };
        match offer {
            Offer::Person { id, .. } => self.open_person(PersonId(id)).await,
            group @ Offer::Group { .. } => {
                let Offer::Group { channel, .. } = &group else {
                    return;
                };
                self.enter_group(channel.clone(), group.members());
            }
            // Accepting is what creates it.
            group @ Offer::NewGroup { .. } => {
                let members = group.members();
                match self.engine.create_group(members.clone(), None).await {
                    Ok(channel) => self.enter_group(channel, members),
                    Err(e) => self.term.error(&e.to_string()),
                }
            }
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
        self.term.note("  /go <n> takes you there");
        self.offers = offers;
    }

    async fn host(&mut self) {
        if !self.names.has_host() {
            return self.term.print("this corpus has no host\n");
        }
        match self.engine.host_channel().await {
            Ok(channel) => {
                let name = self.names.host();
                self.term.note(&format!("talking to the host, {name}"));
                self.current = Some(Current {
                    channel,
                    kind: Kind::Host,
                    label: name,
                });
            }
            Err(e) => self.term.error(&e.to_string()),
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
                self.term.note("cancelled; the message stays in the history");
                None
            }
        }
    }

    /// What background work reported as failed: the endpoint's own text for a turn that
    /// ended without a reply.
    fn report_failures(&self) {
        for f in self.bridge.take_failures() {
            self.term.error(&f);
        }
    }

    async fn current_channel(&self) -> Option<ChannelId> {
        let channel = self.current.as_ref().map(|c| c.channel.clone());
        if channel.is_none() {
            self.term
                .print("no channel yet: /talk <person> or /host first\n");
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
            self.term.error(&e.to_string());
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
            self.term.note(&format!(
                "· {n} call{}, {:.1}s",
                if n == 1 { "" } else { "s" },
                started.elapsed().as_secs_f64()
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
            return self
                .term
                .print("the host keeps no memories; nothing to wrap up\n");
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
            Some(Err(e)) => self.term.error(&e.to_string()),
            Some(Ok(())) => {
                let written: Vec<FactView> = self
                    .facts(None, FactScope::All)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| !before.contains(&f.id))
                    .collect();
                if written.is_empty() {
                    self.term.note("wrapped up; no memories were written");
                } else {
                    self.term
                        .note(&format!("wrapped up; {} memories written", written.len()));
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
            Audience::World => "everyone".into(),
            Audience::Own { person } => format!("only {}", person.name),
            Audience::Participants { channel, persons } => {
                let names: Vec<&str> = persons.iter().map(|p| p.name.as_str()).collect();
                format!("channel {} ({})", channel.0, names.join(", "))
            }
        }
    }

    fn print_memories(&self, memories: &[FactView]) {
        for (i, m) in memories.iter().enumerate() {
            let kind = format!("{:?}", m.kind).to_lowercase();
            self.term
                .print(&format!("{}. [{kind}] {}\n", i + 1, m.text));
            self.term
                .note(&format!("    seen by: {}", self.audience(m)));
        }
    }

    async fn memories(&mut self, who: Option<&str>) {
        let person = match who {
            Some(who) => match self.resolve(who).await {
                Ok(p) => Some(p),
                Err(e) => return self.term.print(&format!("{e}\n")),
            },
            None => None,
        };
        let mut all = match self.facts(person.clone(), FactScope::All).await {
            Ok(m) => m,
            Err(e) => return self.term.error(&e.to_string()),
        };
        let live = all.len();
        // Deleted ones follow in the same numbering, so `/restore` has something to name.
        all.extend(
            self.facts(person, FactScope::Deleted)
                .await
                .unwrap_or_default(),
        );
        if all.is_empty() {
            return self.term.note("no memories");
        }
        for (i, m) in all.iter().enumerate() {
            let kind = format!("{:?}", m.kind).to_lowercase();
            let line = format!("{}. [{kind}] {}", i + 1, m.text);
            if i < live {
                self.term.print(&format!("{line}\n"));
                self.term
                    .note(&format!("    seen by: {}", self.audience(m)));
            } else {
                self.term.note(&format!("{line}  (deleted)"));
            }
        }
        self.term.note(&format!(
            "{live} memories, {} deleted; /forget, /restore or /edit <n>",
            all.len() - live
        ));
        self.listed = all;
    }

    async fn change_memory(&mut self, n: usize, change: Change) {
        let Some(m) = self.listed.get(n - 1).cloned() else {
            return self.term.note(if self.listed.is_empty() {
                "no list yet: /memories first"
            } else {
                "no such memory in the last list"
            });
        };
        let done = match (&change, m.retracted) {
            (Change::Forget, true) | (Change::Edit(_), true) => {
                return self.term.note("that one is deleted; /restore it first");
            }
            (Change::Restore, false) => return self.term.note("that one is not deleted"),
            (Change::Forget, false) => self.engine.delete_fact(m.id).await.map(|()| "deleted"),
            (Change::Restore, true) => self.engine.restore_fact(m.id).await.map(|()| "restored"),
            (Change::Edit(text), false) => self
                .engine
                .edit_fact(m.id, text.clone())
                .await
                .map(|_| "rewritten"),
        };
        match done {
            Ok(what) => {
                // Numbers would now point at different memories.
                self.listed.clear();
                self.term.note(&format!("{what}; /memories lists again"));
            }
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    // ---- reading back ----

    async fn history(&self, n: usize) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        match self.engine.history(channel, None, n as u32).await {
            Ok(messages) if messages.is_empty() => self.term.note("nothing said yet"),
            Ok(messages) => {
                for m in &messages {
                    self.term.print(&self.show.whole(m));
                }
            }
            Err(e) => self.term.error(&e.to_string()),
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
            Err(e) => return self.term.error(&e.to_string()),
        };
        let Some(at) = messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, m)| m.author == Author::User)
            .nth(n - 1)
            .map(|(i, _)| i)
        else {
            return self.term.note("no message of yours to send again");
        };
        let removed = messages.len() - at;
        match self.engine.rewind(channel, messages[at].id.clone()).await {
            Ok(text) => {
                self.term.note(&format!(
                    "took back {removed} message{}; edit it and send again",
                    if removed == 1 { "" } else { "s" }
                ));
                self.prefill = Some(text);
            }
            Err(api::ApiError::Invalid { field }) if field == "busy" => {
                self.term
                    .note("a reply is still being written here; try again when it ends");
            }
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    /// Print in full the sources of the n-th last reply that cites any.
    async fn sources(&self, n: usize) {
        let Some(channel) = self.current_channel().await else {
            return;
        };
        let messages = match self.engine.history(channel, None, Self::LOOKBACK).await {
            Ok(m) => m,
            Err(e) => return self.term.error(&e.to_string()),
        };
        let found = messages
            .iter()
            .rev()
            .filter(|m| m.author != Author::User && !m.cites.is_empty())
            .nth(n - 1)
            .and_then(|m| self.show.sources_in_full(m));
        match found {
            Some(text) => self.term.print(&text),
            None => self.term.note("no reply with sources there"),
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

    async fn search(&self, query: &str) {
        let Some(c) = &self.current else {
            return self
                .term
                .print("no channel yet: /talk <person> or /host first\n");
        };
        let as_person = self.speaker(c).await;
        let found = self
            .engine
            .search_corpus(query.to_owned(), as_person.iter().cloned().collect())
            .await;
        let found = match found {
            Ok(r) => r,
            Err(e) => return self.term.error(&e.to_string()),
        };
        match &as_person {
            Some(p) => self.term.note(&format!("as {}", self.names.person(&p.0))),
            None => self.term.note("as the host: whole corpus"),
        }
        self.term
            .note(&format!("confidence: {:?}", found.confidence).to_lowercase());
        for (i, hit) in found.hits.iter().enumerate() {
            let head = if hit.header.is_empty() {
                &hit.citation.page
            } else {
                &hit.header
            };
            self.term.print(&format!(
                "{}. {} {}\n",
                i + 1,
                head,
                self.term.style.dim(&format!("({})", hit.citation.chunk_id)),
            ));
            self.term
                .print(&format!("   {}\n", excerpt(&hit.text, 100)));
        }
        if found.hits.is_empty() {
            self.term.note("no hits");
        }
    }

    // ---- encryption ----

    /// The new password, typed twice.
    fn new_password(&self) -> Option<String> {
        let first = (self.secret)("new password: ");
        let again = first.as_ref().and_then(|_| (self.secret)("again: "));
        match (first, again) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(_), Some(_)) => {
                self.term.print("the two did not match\n");
                None
            }
            _ => {
                self.term.note("cancelled");
                None
            }
        }
    }

    async fn set_password(&self, old: Option<String>, new: Option<String>, done: &str) {
        match self.engine.set_password(old, new).await {
            Ok(()) => self.term.note(done),
            Err(e) => self.term.print(&format!("{}\n", Self::why(&e))),
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
            (None, true) => self
                .term
                .note("this save is encrypted; /password change or /password off"),
            (None, false) => self
                .term
                .note("this save is not encrypted; /password on turns it on"),
            (Some(PasswordAct::On), false) => {
                self.term.print(FORGET_WARNING);
                self.term.print("encrypt this save with a password? (y/n) ");
                self.pending = Some(Pending::PasswordOn);
            }
            (Some(PasswordAct::Off), true) => {
                self.term
                    .print("remove the password from this save? (y/n) ");
                self.pending = Some(Pending::PasswordOff);
            }
            (Some(PasswordAct::Change), true) => {
                let Some(old) = (self.secret)("current password: ") else {
                    return self.term.note("cancelled");
                };
                let Some(new) = self.new_password() else {
                    return;
                };
                self.set_password(Some(old), Some(new), "password changed")
                    .await;
            }
            (Some(PasswordAct::On), true)
            | (Some(PasswordAct::Change | PasswordAct::Off), false) => {
                self.term.print(if encrypted {
                    "this save is already encrypted; /password change or /password off\n"
                } else {
                    "this save is not encrypted; /password on first\n"
                })
            }
        }
    }

    /// `/bio`: what the people are told about you.
    async fn bio(&self, text: Option<String>) {
        let settings = match self.engine.settings().await {
            Ok(s) => s,
            Err(e) => return self.term.error(&e.to_string()),
        };
        let Some(text) = text else {
            return self.term.note(&match settings.bio {
                Some(b) => format!("bio: {b}"),
                None => "no bio; /bio <text> sets one".to_owned(),
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
                self.term.note(if next.bio.is_some() {
                    "bio saved"
                } else {
                    "bio cleared"
                })
            }
            Err(api::ApiError::Invalid { .. }) => self.term.print(&format!(
                "a bio is at most {} characters\n",
                world::settings::BIO_MAX
            )),
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    // ---- help and settings ----

    /// `/help`: the groups in the corpus's wording, then how to send.
    fn help(&self) -> String {
        let mut groups: Vec<String> = [
            "conversation",
            "channels",
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
                return self.term.print(&format!("levels: {}\n", all.join(", ")));
            };
            if let Err(e) = self.engine.set_tier(*tier).await {
                return self.term.error(&e.to_string());
            }
        }
        match self.engine.quota().await {
            Ok(q) => self.term.note(&format!(
                "tier {}, quota {}",
                name(q.tier),
                format!("{:?}", q.band).to_lowercase()
            )),
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    async fn quiet(&self, arg: Option<&str>) {
        let settings = match self.engine.settings().await {
            Ok(s) => s,
            Err(e) => return self.term.error(&e.to_string()),
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
                None => return self.term.print("usage: /quiet [on|off|HH:MM-HH:MM]\n"),
            },
        };
        if arg.is_some() {
            let next = Settings {
                quiet_hours: quiet.clone(),
                ..settings
            };
            if let Err(e) = self.engine.update_settings(next).await {
                return match e {
                    api::ApiError::Invalid { .. } => self
                        .term
                        .print("hours are 24-hour HH:MM, like 23:00-07:00\n"),
                    e => self.term.error(&e.to_string()),
                };
            }
        }
        self.term.note(&match quiet {
            Some(q) => format!("quiet hours on, {} to {}", q.from, q.to),
            None => "quiet hours off".to_owned(),
        });
    }

    // ---- saves ----

    /// What a save command's failure means in a terminal.
    fn why(e: &api::ApiError) -> String {
        match e {
            api::ApiError::Busy => "a reply is still being written; try again when it ends".into(),
            api::ApiError::Exists => "a save with that name already exists".into(),
            api::ApiError::Locked => {
                "that save is encrypted; /save switch to it and enter its password".into()
            }
            api::ApiError::WrongPassword => "wrong password".into(),
            api::ApiError::Invalid { field } if field == "new" => {
                "that is not a usable password".into()
            }
            api::ApiError::NotFound => "no such save (or file)".into(),
            api::ApiError::Invalid { field } if field == "name" => {
                "that is not a usable name (no slashes, no leading dot, at most 48 characters)"
                    .into()
            }
            api::ApiError::Invalid { field } if field == "file" => {
                "that file is not a world database this version can read".into()
            }
            other => other.to_string(),
        }
    }

    async fn save(&mut self, cmd: Save) {
        let done = match cmd {
            Save::List => return self.list_saves().await,
            Save::New(n) => self
                .engine
                .new_save(n.clone())
                .await
                .map(|()| format!("created {n}")),
            Save::Switch(n) => return self.switch_save(&n).await,
            Save::Logout => return self.log_out().await,
            Save::Copy(a, b) => self
                .engine
                .copy_save(a.clone(), b.clone())
                .await
                .map(|()| format!("copied {a} to {b}")),
            Save::Rename(a, b) => self
                .engine
                .rename_save(a.clone(), b.clone())
                .await
                .map(|()| format!("renamed {a} to {b}")),
            Save::Delete(n) => return self.ask_delete(n).await,
            Save::Export(n, path) => return self.export_save(n, path).await,
            Save::Import(path, name) => self
                .import(expand(&path), name)
                .await
                .map(|n| format!("imported as {n}; /save switch {n} opens it")),
        };
        match done {
            Ok(text) => self.term.note(&text),
            Err(e) => self.term.print(&format!("{}\n", Self::why(&e))),
        }
    }

    async fn list_saves(&self) {
        let saves = match self.engine.saves().await {
            Ok(s) => s,
            Err(e) => return self.term.error(&e.to_string()),
        };
        for s in &saves {
            let played = s.played_at.as_ref().map_or_else(
                || "-".to_owned(),
                |t| t.0.chars().take(16).collect::<String>().replace('T', " "),
            );
            self.term.print(&format!(
                "{} {:<24} {:>8} KB  {played}{}\n",
                if s.current { "*" } else { " " },
                s.name,
                s.size_kb,
                if s.encrypted { "  encrypted" } else { "" }
            ));
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
            return self.term.print(&format!("{}\n", Self::why(&e)));
        }
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
            self.term.error(&e.to_string());
        }
        self.term.note(&format!(
            "now playing {name}; /talk <person> or /host to begin"
        ));
    }

    /// Whether no save is open and the lines typed choose one.
    pub fn picking(&self) -> bool {
        self.picking
    }

    /// `/save logout`: close the open save, forget where it was, and show the save list.
    async fn log_out(&mut self) {
        if let Err(e) = self.engine.log_out().await {
            return self.term.print(&format!("{}\n", Self::why(&e)));
        }
        self.current = None;
        self.offers.clear();
        self.listed.clear();
        self.calls.clear();
        self.picking = true;
        self.term.note("logged out of the save");
        self.show_picker().await;
    }

    /// The numbered list of saves and what can be typed. Also the first thing a start
    /// without a usable last save shows, with the reason.
    pub async fn show_picker(&mut self) {
        let status = self.engine.app_status().await.ok();
        if let Some(gone) = status.as_ref().and_then(|s| s.missing_save.as_ref()) {
            self.term.print(&format!(
                "The save you used last, \"{gone}\", is not there any more (renamed or deleted elsewhere).\n"
            ));
        }
        let saves = self.engine.saves().await.unwrap_or_default();
        let dir = status.map_or_else(String::new, |s| s.saves_dir);
        self.term.print(&format!("Saves in {dir}\n"));
        if saves.is_empty() {
            self.term.print("  (none yet)\n");
        }
        for (i, s) in saves.iter().enumerate() {
            let played = s.played_at.as_ref().map_or_else(
                || "-".to_owned(),
                |t| t.0.chars().take(16).collect::<String>().replace('T', " "),
            );
            self.term.print(&format!(
                "  {:>2}  {:<24} {:>8} KB  {played}{}\n",
                i + 1,
                s.name,
                s.size_kb,
                if s.encrypted { "  encrypted" } else { "" }
            ));
        }
        self.pickable = saves.into_iter().map(|s| s.name).collect();
        self.term.print(
            "Type a number to open a save, \"new <name>\" to start one, \"import <path> [name]\" \
             to add a file, or \"quit\".\n",
        );
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
                    self.term.print("there is no save with that number\n");
                    return Flow::Continue;
                }
            },
            [name] if self.pickable.iter().any(|s| s == name) => (*name).to_owned(),
            _ => {
                self.term.print(
                    "No save is open. Type a number from the list, \"new <name>\", \
                     \"import <path> [name]\" or \"quit\".\n",
                );
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
        self.term.print(&format!("{}\n", Self::why(e)));
        Flow::Continue
    }

    /// Ask for the password of save `name` until it opens it or the tries run out.
    async fn unlock(&self, name: &str) -> bool {
        for _ in 0..TRIES {
            let Some(password) = (self.secret)(&format!("password for {name}: ")) else {
                self.term.note("cancelled");
                return false;
            };
            match self.engine.unlock(name.to_owned(), password).await {
                Ok(()) => return true,
                Err(api::ApiError::WrongPassword) => self.term.print("wrong password\n"),
                Err(e) => {
                    self.term.print(&format!("{}\n", Self::why(&e)));
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
                self.term.print("wrong password\n");
            }
            let Some(password) = (self.secret)("password of that file: ") else {
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
            return self
                .term
                .print("that is the save in use; /save switch to another first\n");
        }
        self.term
            .print(&format!("delete save {name} for good? (y/n) "));
        self.pending = Some(Pending::DeleteSave(name));
    }

    async fn export_save(&mut self, name: String, path: String) {
        let path = expand(&path);
        if std::path::Path::new(&path).exists() {
            self.term
                .print(&format!("{path} exists; replace it? (y/n) "));
            self.pending = Some(Pending::Overwrite { name, path });
            return;
        }
        self.export_now(&name, path).await;
    }

    async fn export_now(&self, name: &str, path: String) {
        match self.engine.export_save(name.to_owned(), path.clone()).await {
            Ok(()) => self.term.note(&format!("wrote {path}")),
            Err(e) => self.term.print(&format!("{}\n", Self::why(&e))),
        }
    }

    async fn confirmed(&mut self, pending: Pending) {
        match pending {
            Pending::DeleteSave(name) => match self.engine.delete_save(name.clone()).await {
                Ok(()) => self.term.note(&format!("deleted {name}")),
                Err(e) => self.term.print(&format!("{}\n", Self::why(&e))),
            },
            Pending::Overwrite { name, path } => self.export_now(&name, path).await,
            Pending::PasswordOn => {
                let Some(new) = self.new_password() else {
                    return;
                };
                self.set_password(None, Some(new), "this save is encrypted now")
                    .await;
            }
            Pending::PasswordOff => {
                let Some(old) = (self.secret)("password: ") else {
                    return self.term.note("cancelled");
                };
                self.set_password(Some(old), None, "this save is not encrypted now")
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
        let (started, at) = (Instant::now(), now_ms());
        let (engine, bridge) = (self.engine.clone(), self.bridge.clone());
        let ran = self
            .interruptible(async move {
                if opened {
                    engine.presence().await?;
                    bridge.idle().await;
                }
                bridge.tick().await;
                bridge.idle().await;
                Ok::<(), api::ApiError>(())
            })
            .await;
        if let Some(Err(e)) = ran {
            self.term.error(&e.to_string());
        }
        self.report_failures();
        self.offer();
        self.footer(at, started).await;
    }

    fn stats(&self) {
        if self.calls.is_empty() {
            return self.term.note("no model calls yet");
        }
        self.term.print(&format!(
            "{:>3}  {:<10} {:<28} {:>9} {:>9} {:>7} {:>6}\n",
            "#", "shape", "model", "uncached", "cached", "output", "secs"
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
            "", "total", "", u, c, o, s
        ));
    }
}
