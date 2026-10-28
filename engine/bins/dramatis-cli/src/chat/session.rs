//! The state of one terminal session and what each command does with it. Everything it
//! shows or does is a seam command the app issues too; the one exception is `/stats`,
//! which reads the meter through the bridge.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use api::error::ApiResult;
use api::types::{Author, ChannelId, FactId, PersonId};
use api::views::{Audience, FactFilter, FactScope, FactView, RosterRow, Settings};
use api::{Api, Engine};
use runtime::{AgentBridge, Call};

use super::command::{Command, HELP};
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
    current: Option<Current>,
    /// The host's latest choices, numbered from 1 for `/go`.
    offers: Vec<Offer>,
    calls: Vec<Timed>,
    /// The highest meter row already counted; rows before the session are not ours.
    metered: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    ) -> Self {
        let metered = bridge
            .calls_after(0)
            .await
            .ok()
            .and_then(|c| c.last().map(|c| c.id))
            .unwrap_or(0);
        Self {
            engine,
            bridge,
            term,
            show,
            names,
            current: None,
            offers: Vec::new(),
            calls: Vec::new(),
            metered,
        }
    }

    /// The input prompt: the channel in dim type, then the user's own name, since the
    /// line being typed is theirs and not the channel's.
    pub fn prompt(&self) -> String {
        let style = self.term.style;
        let channel = self.current.as_ref().map_or("-", |c| c.label.as_str());
        format!(
            "{} {}> ",
            style.dim(&format!("[{channel}]")),
            style.bold(&self.names.user())
        )
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

    pub async fn handle(&mut self, command: Command) -> Flow {
        match command {
            Command::Noop => {}
            Command::Quit => return Flow::Quit,
            Command::Help => self.term.print(&format!("{HELP}\n")),
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
            Command::Say(text) => self.say(&text).await,
            Command::Wrapup => self.wrapup().await,
            Command::Memories(who) => self.memories(who.as_deref()).await,
            Command::History(n) => self.history(n).await,
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
        tokio::select! {
            r = work => Some(r),
            _ = tokio::signal::ctrl_c() => {
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
                    .facts(None)
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
                }
            }
            None => {}
        }
        self.footer(at, started).await;
    }

    // ---- memories ----

    async fn facts(&self, person: Option<PersonId>) -> ApiResult<Vec<FactView>> {
        let filter = FactFilter {
            scope: FactScope::All,
            person,
            task: None,
        };
        Ok(self.engine.facts(filter).await?.items)
    }

    async fn fact_ids(&self) -> Vec<FactId> {
        self.facts(None)
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
        for m in memories {
            let kind = format!("{:?}", m.kind).to_lowercase();
            self.term
                .print(&format!("#{} [{kind}] {}\n", m.id.0, m.text));
            self.term
                .note(&format!("    seen by: {}", self.audience(m)));
        }
    }

    async fn memories(&self, who: Option<&str>) {
        let person = match who {
            Some(who) => match self.resolve(who).await {
                Ok(p) => Some(p),
                Err(e) => return self.term.print(&format!("{e}\n")),
            },
            None => None,
        };
        let all = match self.facts(person).await {
            Ok(m) => m,
            Err(e) => return self.term.error(&e.to_string()),
        };
        if all.is_empty() {
            return self.term.note("no memories");
        }
        self.print_memories(&all);
        self.term.note(&format!("{} memories", all.len()));
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
