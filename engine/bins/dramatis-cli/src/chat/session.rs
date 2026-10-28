//! The state of one terminal session and what each command does with it.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use agent::Agent;
use api::types::{Author, PersonId as ApiPerson};
use world::{Actor, ChannelId, PersonId};

use super::command::{Command, HELP};
use super::show::{self, Offer, Show, Term, Working, person_name};

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

/// One model call of this session, from the meter.
struct Call {
    shape: String,
    model: String,
    uncached: i64,
    cached: i64,
    output: i64,
    /// Time since the previous call finished, or since the command started.
    secs: f64,
}

/// A stored memory, as read from the world.
struct Memory {
    id: i64,
    kind: String,
    audience: String,
    audience_ref: String,
    text: String,
}

pub struct Session {
    agent: Arc<Agent>,
    term: Arc<Term>,
    show: Arc<Show>,
    roster: Vec<Entry>,
    current: Option<Current>,
    /// The host's latest choices, numbered from 1 for `/go`.
    offers: Vec<Offer>,
    calls: Vec<Call>,
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
    pub fn new(agent: Arc<Agent>, term: Arc<Term>, show: Arc<Show>) -> Self {
        let roster = agent
            .with_index(|ix| ix.folio().persons())
            .unwrap_or_default()
            .into_iter()
            .map(|p| Entry {
                persona: agent.has_persona(&p.person_id),
                forms: p.forms.into_iter().map(|f| f.page).collect(),
                id: p.person_id,
                display: p.display,
            })
            .collect();
        let metered = agent
            .with_world(|w| w.query_row("SELECT COALESCE(MAX(id), 0) FROM meter", [], |r| r.get(0)))
            .unwrap_or(0);
        Self {
            agent,
            term,
            show,
            roster,
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
        let user = show::author_name(&self.agent, &Author::User);
        let channel = self.current.as_ref().map_or("-", |c| c.label.as_str());
        format!(
            "{} {}> ",
            style.dim(&format!("[{channel}]")),
            style.bold(&user)
        )
    }

    /// Set the user's name; a changed name rolls the channels over, as in the app.
    pub async fn set_user(&mut self, name: &str) -> bool {
        match self
            .agent
            .with_world(|w| world::settings::set_user_name(w, Some(name)))
        {
            Ok(true) => {
                if let Err(e) = self.agent.on_rename().await {
                    self.term.error(&e.to_string());
                }
                true
            }
            Ok(false) => true,
            Err(e) => {
                self.term.error(&e.to_string());
                false
            }
        }
    }

    pub async fn handle(&mut self, command: Command) -> Flow {
        match command {
            Command::Noop => {}
            Command::Quit => return Flow::Quit,
            Command::Help => self.term.print(&format!("{HELP}\n")),
            Command::Invalid(text) => self.term.print(&format!("{text}\n")),
            Command::List(filter) => self.list(filter.as_deref()),
            Command::Talk(who) => self.talk(&who),
            Command::Group(who) => self.group(&who),
            Command::Host => self.host(),
            Command::Go(n) => self.go(n),
            Command::Name(name) => {
                if self.set_user(&name).await {
                    self.term.note(&format!("you are called {name} now"));
                }
            }
            Command::Say(text) => self.say(&text).await,
            Command::Wrapup => self.wrapup().await,
            Command::Memories(who) => self.memories(who.as_deref()),
            Command::History(n) => self.history(n),
            Command::Search(query) => self.search(&query),
            Command::Stats => self.stats(),
        }
        Flow::Continue
    }

    // ---- who ----

    fn list(&self, filter: Option<&str>) {
        let needle = filter.map(str::to_lowercase);
        let host = if self.agent.personas().host() {
            "available"
        } else {
            "not available"
        };
        self.term.print(&format!(
            "host: {} ({host})\n",
            self.agent.wording().get("host.name")
        ));
        let mut shown = 0;
        for e in self.roster.iter().filter(|e| e.persona) {
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
    fn resolve(&self, query: &str) -> Result<PersonId, String> {
        let q = query.trim();
        let lower = q.to_lowercase();
        let exact: Vec<&Entry> = self
            .roster
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
                .agent
                .with_index(|ix| ix.folio().persons_for_alias(q))
                .unwrap_or_default();
            let by_alias: Vec<&Entry> = self
                .roster
                .iter()
                .filter(|e| aliased.contains(&e.id))
                .collect();
            if !by_alias.is_empty() {
                by_alias
            } else {
                self.roster
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

    fn talk(&mut self, who: &str) {
        match self.resolve(who) {
            Ok(person) => self.open_person(person),
            Err(e) => self.term.print(&format!("{e}\n")),
        }
    }

    fn open_person(&mut self, person: PersonId) {
        match self.agent.open_direct(&person) {
            Ok(channel) => {
                let name = person_name(&self.agent, person.as_str());
                self.term.note(&format!(
                    "talking to {name} ({}); /history shows earlier lines",
                    person.as_str()
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

    fn group(&mut self, who: &[String]) {
        let mut members: Vec<PersonId> = Vec::new();
        for w in who {
            match self.resolve(w) {
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
        match self.agent.create_group(&members, None) {
            Ok(channel) => {
                let names: Vec<String> = members
                    .iter()
                    .map(|p| person_name(&self.agent, p.as_str()))
                    .collect();
                self.term.note(&format!("group with {}", names.join(", ")));
                self.current = Some(Current {
                    channel,
                    kind: Kind::Group(members),
                    label: names.join("+"),
                });
            }
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    /// Switch to choice `n` of the host's latest offer.
    fn go(&mut self, n: usize) {
        let Some(offer) = self.offers.get(n - 1).cloned() else {
            return self.term.print(if self.offers.is_empty() {
                "the host has offered nothing to go to\n"
            } else {
                "no such choice; the host's list is above\n"
            });
        };
        match offer {
            Offer::Person { id, .. } => self.open_person(PersonId(id)),
            Offer::Group { channel, .. } => {
                let Ok(id) = channel.0.parse::<i64>().map(ChannelId) else {
                    return self.term.error("bad group id");
                };
                let members = match self.agent.with_world(|w| world::channel::get(w, id)) {
                    Ok(c) => c.persons(),
                    Err(e) => return self.term.error(&e.to_string()),
                };
                let names: Vec<String> = members
                    .iter()
                    .map(|p| person_name(&self.agent, p.as_str()))
                    .collect();
                self.term.note(&format!("group with {}", names.join(", ")));
                self.current = Some(Current {
                    channel: id,
                    kind: Kind::Group(members),
                    label: names.join("+"),
                });
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
            self.term.note(&format!("  {} {}", i + 1, o.text()));
        }
        self.term.note("  /go <n> takes you there");
        self.offers = offers;
    }

    fn host(&mut self) {
        if !self.agent.personas().host() {
            return self.term.print("this corpus has no host\n");
        }
        match self.agent.with_world(|w| world::channel::host(w)) {
            Ok(channel) => {
                let name = self.agent.wording().get("host.name").to_owned();
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

    /// Run a model-facing future; Ctrl-C drops it instead of ending the session. What was
    /// already stored stays stored, so the cancelled turn may leave the user's line
    /// without a reply.
    async fn interruptible<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            r = work => Some(r),
            _ = tokio::signal::ctrl_c() => {
                self.show.abort();
                self.term.note("cancelled; the message stays in the history");
                None
            }
        }
    }

    async fn say(&mut self, text: &str) {
        let Some(channel) = self.current.as_ref().map(|c| c.channel) else {
            return self
                .term
                .print("no channel yet: /talk <person> or /host first\n");
        };
        let (agent, started, at) = (self.agent.clone(), Instant::now(), now_ms());
        let who = self.current.as_ref().map_or("", |c| c.label.as_str());
        self.term
            .note(&show::working_text(&self.agent, who, Working::Typing));
        let result = self
            .interruptible(async move {
                let posted = agent.post(channel, text, None)?;
                agent.respond(posted).await
            })
            .await;
        if let Some(Err(e)) = result {
            self.term.error(&e.to_string());
        }
        self.offer();
        self.footer(at, started);
    }

    /// Count the calls made since `at` and say how many and how long.
    fn footer(&mut self, at: i64, started: Instant) {
        let n = self.collect(at);
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
    fn collect(&mut self, at: i64) -> usize {
        let rows: Vec<(i64, i64, String, String, i64, i64, i64)> = self
            .agent
            .with_world(|w| {
                let mut s = w.prepare(
                    "SELECT id, at, shape, model, uncached, cached, output FROM meter \
                     WHERE id > ?1 ORDER BY id",
                )?;
                s.query_map([self.metered], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        let mut prev = at;
        for (id, end, shape, model, uncached, cached, output) in &rows {
            self.calls.push(Call {
                shape: shape.clone(),
                model: model.clone(),
                uncached: *uncached,
                cached: *cached,
                output: *output,
                secs: (end - prev).max(0) as f64 / 1000.0,
            });
            prev = *end;
            self.metered = *id;
        }
        rows.len()
    }

    async fn wrapup(&mut self) {
        let Some(channel) = self.current.as_ref().map(|c| c.channel) else {
            return self
                .term
                .print("no channel yet: /talk <person> or /host first\n");
        };
        if matches!(self.current.as_ref().map(|c| &c.kind), Some(Kind::Host)) {
            return self
                .term
                .print("the host keeps no memories; nothing to wrap up\n");
        }
        let before = self.last_memory();
        let (agent, started, at) = (self.agent.clone(), Instant::now(), now_ms());
        let who = self.current.as_ref().map_or("", |c| c.label.as_str());
        self.term
            .note(&show::working_text(&self.agent, who, Working::Typing));
        let result = self
            .interruptible(async move { agent.wrapup(channel).await })
            .await;
        match result {
            Some(Err(e)) => self.term.error(&e.to_string()),
            Some(Ok(())) => {
                let written: Vec<Memory> = self
                    .memories_where("id > ?1 AND retracted = 0", before)
                    .unwrap_or_default();
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
        self.footer(at, started);
    }

    // ---- memories ----

    fn last_memory(&self) -> i64 {
        self.agent
            .with_world(|w| w.query_row("SELECT COALESCE(MAX(id), 0) FROM fact", [], |r| r.get(0)))
            .unwrap_or(0)
    }

    fn memories_where(&self, cond: &str, after: i64) -> rusqlite::Result<Vec<Memory>> {
        self.agent.with_world(|w| {
            let mut s = w.prepare(&format!(
                "SELECT id, kind, audience_kind, COALESCE(audience_ref, ''), text FROM fact \
                 WHERE {cond} ORDER BY id"
            ))?;
            s.query_map([after], |r| {
                Ok(Memory {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    audience: r.get(2)?,
                    audience_ref: r.get(3)?,
                    text: r.get(4)?,
                })
            })?
            .collect()
        })
    }

    /// Who may see a memory, in words.
    fn audience(&self, m: &Memory) -> String {
        match m.audience.as_str() {
            "world" => "everyone".into(),
            "self" => format!("only {}", person_name(&self.agent, &m.audience_ref)),
            _ => {
                let names: Vec<String> = m
                    .audience_ref
                    .parse::<i64>()
                    .ok()
                    .and_then(|c| {
                        self.agent
                            .with_world(|w| world::channel::participants(w, ChannelId(c)))
                            .ok()
                    })
                    .unwrap_or_default()
                    .iter()
                    .map(|a| match a {
                        Actor::User => "you".to_owned(),
                        Actor::Host => self.agent.wording().get("host.name").to_owned(),
                        Actor::Person(p) => person_name(&self.agent, p.as_str()),
                    })
                    .collect();
                format!("channel {} ({})", m.audience_ref, names.join(", "))
            }
        }
    }

    fn print_memories(&self, memories: &[Memory]) {
        for m in memories {
            self.term
                .print(&format!("#{} [{}] {}\n", m.id, m.kind, m.text));
            self.term
                .note(&format!("    seen by: {}", self.audience(m)));
        }
    }

    fn memories(&self, who: Option<&str>) {
        let mut all = match self.memories_where("retracted = 0 AND ?1 >= 0", 0) {
            Ok(m) => m,
            Err(e) => return self.term.error(&e.to_string()),
        };
        if let Some(who) = who {
            let person = match self.resolve(who) {
                Ok(p) => p,
                Err(e) => return self.term.print(&format!("{e}\n")),
            };
            match self
                .agent
                .with_world(|w| world::fact::visible_to_person(w, &person))
            {
                Ok(visible) => all.retain(|m| visible.iter().any(|f| f.id.0 == m.id)),
                Err(e) => return self.term.error(&e.to_string()),
            }
        }
        if all.is_empty() {
            return self.term.note("no memories");
        }
        self.print_memories(&all);
        self.term.note(&format!("{} memories", all.len()));
    }

    // ---- reading back ----

    fn history(&self, n: usize) {
        let Some(c) = &self.current else {
            return self
                .term
                .print("no channel yet: /talk <person> or /host first\n");
        };
        match self.agent.history(c.channel, None, n as u32) {
            Ok(messages) if messages.is_empty() => self.term.note("nothing said yet"),
            Ok(messages) => {
                for m in &messages {
                    self.term.print(&self.show.whole(&self.agent, m));
                }
            }
            Err(e) => self.term.error(&e.to_string()),
        }
    }

    /// Whose retrieval `/search` runs as: the person of a direct channel, the last person
    /// who spoke in a group (else its first member), nobody for the host.
    fn speaker(&self, c: &Current) -> Option<PersonId> {
        match &c.kind {
            Kind::Person(p) => Some(p.clone()),
            Kind::Host => None,
            Kind::Group(members) => {
                let last = self.agent.history(c.channel, None, 50).ok().and_then(|h| {
                    h.iter().rev().find_map(|m| match &m.author {
                        Author::Person { id: ApiPerson(p) } => Some(PersonId(p.clone())),
                        _ => None,
                    })
                });
                last.or_else(|| members.first().cloned())
            }
        }
    }

    fn search(&self, query: &str) {
        let Some(c) = &self.current else {
            return self
                .term
                .print("no channel yet: /talk <person> or /host first\n");
        };
        let as_person = self.speaker(c);
        let memories = match self
            .agent
            .with_world(|w| world::fact::visible_to(w, as_person.as_ref(), c.channel))
        {
            Ok(m) => m,
            Err(e) => return self.term.error(&e.to_string()),
        };
        let ids: Vec<String> = memories.iter().map(agent::assemble::memory_id).collect();
        let pairs: Vec<(&str, &str)> = ids
            .iter()
            .zip(&memories)
            .map(|(id, f)| (id.as_str(), f.text.as_str()))
            .collect();
        let request = index::SearchRequest {
            query: query.to_owned(),
            as_person: as_person.as_ref().map(|p| p.0.clone()),
            ..index::SearchRequest::default()
        };
        let response = match self
            .agent
            .with_index(|ix| ix.search_with_memories(&request, &pairs))
        {
            Ok(r) => r,
            Err(e) => return self.term.error(&e.to_string()),
        };
        match &as_person {
            Some(p) => self
                .term
                .note(&format!("as {}", person_name(&self.agent, p.as_str()))),
            None => self.term.note("as the host: whole corpus, no memories"),
        }
        self.term
            .note(&format!("confidence: {:?}", response.confidence.level).to_lowercase());
        for (i, hit) in response.hits.iter().enumerate() {
            let tag = hit
                .source
                .map_or(String::new(), |s| format!("{s:?} ").to_lowercase());
            match &hit.item {
                index::Item::Unit(u) => {
                    let head = if u.header.is_empty() {
                        &u.title
                    } else {
                        &u.header
                    };
                    self.term.print(&format!(
                        "{}. {}{} {}\n",
                        i + 1,
                        self.term.style.dim(&tag),
                        head,
                        self.term
                            .style
                            .dim(&format!("({}, {:.1})", u.id, hit.score)),
                    ));
                    self.term.print(&format!("   {}\n", excerpt(&u.text, 100)));
                }
                index::Item::Memory { id, text } => {
                    self.term.print(&format!(
                        "{}. {}memory {}\n   {}\n",
                        i + 1,
                        self.term.style.dim(&tag),
                        id,
                        excerpt(text, 100)
                    ));
                }
            }
        }
        if response.hits.is_empty() {
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
        for (i, call) in self.calls.iter().enumerate() {
            self.term.print(&format!(
                "{:>3}  {:<10} {:<28} {:>9} {:>9} {:>7} {:>6.1}\n",
                i + 1,
                call.shape,
                call.model,
                call.uncached,
                call.cached,
                call.output,
                call.secs
            ));
            u += call.uncached;
            c += call.cached;
            o += call.output;
            s += call.secs;
        }
        self.term.print(&format!(
            "{:>3}  {:<10} {:<28} {:>9} {:>9} {:>7} {:>6.1}\n",
            "", "total", "", u, c, o, s
        ));
    }
}
