//! Seeds: the only reason anyone speaks unprompted.
//!
//! Event seeds (a commitment falling due, a conclusion or pinned question that hit a
//! person's own units) come before daily ones (the user's birthday, a memory about the
//! user worth asking after, the user's recent words hitting his units, a daily event of
//! the day he is part of); within a class
//! whoever has gone longest without talking with the user goes first. At most one person
//! opens per occasion, and four experience limits hold on every tier. A seed held back
//! stays undelivered for the next chance.
//!
//! Retrieval is the caller's: hits arrive as person lists with the chunk ids that matched.

use rusqlite::{Connection, OptionalExtension};

use crate::channel::{self, Channel};
use crate::clock::{self, Now};
use crate::event;
use crate::fact::{self, About, Audience};
use crate::params::{Pace, QuietHours, Seed as SeedParams, Session as SessionParams};
use crate::quota::{self, Band};
use crate::session::{self, Budget, Cause};
use crate::types::{ChannelId, FactId, FactKind, MessageId, Mode, PersonId, TaskId};
use crate::{Result, away, bond, message, settings, task};

/// What a seed is, with what it points at.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SeedKind {
    /// A commitment fell due. Also thaws a frozen person.
    Commitment(FactId),
    /// A new conclusion hit his units.
    Conclusion(FactId),
    /// A pinned question hit his units.
    Pinned(TaskId),
    /// The user's birthday, in this local year.
    Birthday(i32),
    /// The newest memory about the user he can see, not yet used as a seed.
    Memory(FactId),
    /// The user's recent words (this message) hit his units.
    Words(MessageId),
    /// A daily event of today that involves him (the fact that tells of it).
    Happening(FactId),
    /// He has not talked with the user for a while, or the group has been silent: he
    /// drops by with nothing in particular to say. Today's date as `yyyymmdd`.
    Visit(i32),
}

impl SeedKind {
    pub fn is_event(&self) -> bool {
        matches!(
            self,
            Self::Commitment(_) | Self::Conclusion(_) | Self::Pinned(_)
        )
    }

    fn encode(&self) -> (&'static str, String) {
        match self {
            Self::Commitment(f) => ("commitment", f.0.to_string()),
            Self::Conclusion(f) => ("conclusion", f.0.to_string()),
            Self::Pinned(t) => ("pinned", t.0.to_string()),
            Self::Birthday(y) => ("birthday", y.to_string()),
            Self::Memory(f) => ("memory", f.0.to_string()),
            Self::Words(m) => ("words", m.0.to_string()),
            Self::Happening(f) => ("event", f.0.to_string()),
            Self::Visit(d) => ("visit", d.to_string()),
        }
    }

    fn decode(kind: &str, reference: &str) -> Option<Self> {
        let n: i64 = reference.parse().ok()?;
        Some(match kind {
            "commitment" => Self::Commitment(FactId(n)),
            "conclusion" => Self::Conclusion(FactId(n)),
            "pinned" => Self::Pinned(TaskId(n)),
            "birthday" => Self::Birthday(i32::try_from(n).ok()?),
            "memory" => Self::Memory(FactId(n)),
            "words" => Self::Words(MessageId(n)),
            "event" => Self::Happening(FactId(n)),
            "visit" => Self::Visit(i32::try_from(n).ok()?),
            _ => return None,
        })
    }
}

/// One person's retrieval hit: the chunk ids of his own units that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub person: PersonId,
    pub material: Vec<String>,
}

/// An event that searched the roster: a conclusion written, a question pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Conclusion(FactId),
    Pinned(TaskId),
}

/// The user's recent words hitting a person's units (daily seed), found by the caller's
/// retrieval over [`message::user_words_seen_by`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordsHit {
    pub person: PersonId,
    pub message: MessageId,
    pub material: Vec<String>,
}

/// Queue the persons an event's retrieval hit. Only enabled people get such seeds; the
/// disabled are never evaluated. Returns how many were queued (a repeat is a no-op).
pub fn queue_hits(conn: &Connection, event: Event, hits: &[Hit], now: i64) -> Result<u32> {
    let kind = match event {
        Event::Conclusion(f) => SeedKind::Conclusion(f),
        Event::Pinned(t) => SeedKind::Pinned(t),
    };
    let (k, r) = kind.encode();
    let mut n = 0;
    for h in hits {
        if bond::mode(conn, &h.person)? != Mode::Enabled {
            continue;
        }
        n += conn.execute(
            "INSERT INTO seed(person, kind, ref, created_at, material) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT DO NOTHING",
            (&h.person, k, &r, now, serde_json::to_string(&h.material)?),
        )? as u32;
    }
    Ok(n)
}

/// A seed that could open, before the limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub person: PersonId,
    pub kind: SeedKind,
    /// Chunk ids for the opening's attachment.
    pub material: Vec<String>,
    /// People the seed involves besides him (the other participants of the channel a
    /// commitment was made in).
    pub involved: Vec<PersonId>,
    /// A channel the seed belongs to (a commitment made in a group).
    pub origin: Option<ChannelId>,
    pub last_seen: Option<i64>,
}

/// The retrieval-dependent part of "where it opens": does a group's topic match the seed
/// (its retrieval overlapping the seed's hits)? The caller implements it over its index.
pub trait TopicMatch {
    fn matches(&self, seed: &Candidate, group: &Channel) -> bool;
}

/// No topic matching: groups are chosen by members alone.
pub struct NoTopic;

impl TopicMatch for NoTopic {
    fn matches(&self, _: &Candidate, _: &Channel) -> bool {
        false
    }
}

/// A planned opening: the call the agent makes. The seed is recorded as delivered only
/// once the agent reports the message ([`delivered`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opening {
    pub person: PersonId,
    pub channel: ChannelId,
    pub kind: SeedKind,
    pub material: Vec<String>,
    /// The fact the seed is about (commitment, conclusion, memory).
    pub fact: Option<FactId>,
}

/// Why nobody opens now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// There is no seed.
    NoSeed,
    QuietHours,
    /// The user wrote somewhere within `session.idle_timeout`.
    InConversation,
    /// The quota band is not open: unprompted speech is off.
    Quota,
    /// the tier's `daily_total` reached.
    DailyTotal,
    /// Every candidate is at `seed.per_agent_day` or has nowhere to open.
    PerAgent,
    /// The last opening, whoever's, was less than the tier's `gap_min` ago.
    TooSoon,
    /// The day's openings so far are all the waking day has earned by now.
    NotYet,
    /// The user is away: nothing comes to him.
    Away,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    Open(Opening),
    Held(Held),
}

/// When seeds are evaluated.
pub enum Occasion<'a> {
    /// The user opened the app or came back: daily seeds and every backlogged event.
    Presence { words: &'a [WordsHit] },
    /// Something happened (a commitment fell due, an event was queued): event seeds only.
    Event,
}

fn last_seen(conn: &Connection, p: &PersonId) -> Result<Option<i64>> {
    Ok(bond::user_bond(conn, p)?.last_seen)
}

fn used(conn: &Connection, person: Option<&PersonId>, kind: &SeedKind) -> Result<bool> {
    let (k, r) = kind.encode();
    Ok(conn
        .query_row(
            "SELECT 1 FROM seed WHERE kind = ?1 AND ref = ?2 AND delivered_at IS NOT NULL
             AND (?3 IS NULL OR person = ?3) LIMIT 1",
            (k, r, person),
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Commitments due now, as seeds for the one person each is with. The disabled are
/// skipped; frozen people count (a due commitment is a trigger).
fn commitment_candidates(conn: &Connection, now: i64) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for f in fact::due_commitments(conn, now)? {
        let reach = fact::audience_persons(conn, &f.audience)?.unwrap_or_default();
        let reach = bond::filter_disabled(conn, reach)?;
        let pick = |p: Option<&PersonId>| p.filter(|p| reach.contains(p)).cloned();
        let about = match &f.about {
            Some(About::Person(p)) => Some(p),
            _ => None,
        };
        let person = pick(f.author.person())
            .or_else(|| pick(about))
            .or_else(|| reach.first().cloned());
        let Some(person) = person else { continue };
        let origin = match f.audience {
            Audience::Participants(c) => Some(c),
            _ => None,
        };
        out.push(Candidate {
            last_seen: last_seen(conn, &person)?,
            involved: reach.iter().filter(|p| **p != person).cloned().collect(),
            person,
            kind: SeedKind::Commitment(f.id),
            material: Vec::new(),
            origin,
        });
    }
    Ok(out)
}

/// Queued event hits still valid: the conclusion is live, the question still pinned,
/// the person enabled.
fn queued_candidates(conn: &Connection) -> Result<Vec<Candidate>> {
    let rows: Vec<(PersonId, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT person, kind, ref, material FROM seed WHERE delivered_at IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::new();
    for (person, k, r, material) in rows {
        let Some(kind) = SeedKind::decode(&k, &r) else {
            continue;
        };
        let valid = match &kind {
            SeedKind::Conclusion(f) => !fact::get(conn, *f)?.retracted,
            SeedKind::Pinned(t) => task::get(conn, *t)?.pinned,
            _ => false,
        };
        if !valid || bond::mode(conn, &person)? != Mode::Enabled {
            continue;
        }
        out.push(Candidate {
            last_seen: last_seen(conn, &person)?,
            person,
            kind,
            material: serde_json::from_str(&material)?,
            involved: Vec::new(),
            origin: None,
        });
    }
    Ok(out)
}

/// The three daily seeds for every enabled person.
fn daily_candidates(
    conn: &Connection,
    now: Now,
    words: &[WordsHit],
    pace: &Pace,
) -> Result<Vec<Candidate>> {
    let enabled = bond::enabled(conn)?;
    let today = now.local_date();
    let birthday = settings::user_birthday(conn)?.filter(|b| *b == today.month_day());
    let mut out = Vec::new();
    for p in &enabled {
        let mut push = |kind: SeedKind, material: Vec<String>| -> Result<()> {
            out.push(Candidate {
                person: p.clone(),
                kind,
                material,
                involved: Vec::new(),
                origin: None,
                last_seen: last_seen(conn, p)?,
            });
            Ok(())
        };
        if birthday.is_some() && !used(conn, Some(p), &SeedKind::Birthday(today.year))? {
            push(SeedKind::Birthday(today.year), Vec::new())?;
        }
        let mut memory = None;
        for f in fact::visible_to_person(conn, p)?.into_iter().rev() {
            if f.kind != FactKind::Fact || f.about != Some(About::User) {
                continue;
            }
            let k = SeedKind::Memory(f.id);
            if !used(conn, None, &k)? {
                memory = Some(k);
                break;
            }
        }
        if let Some(k) = memory {
            push(k, Vec::new())?;
        }
        // Today's happening he is part of: the newest he has not brought up yet.
        for f in event::facts_for(conn, p, now)?.into_iter().rev() {
            let k = SeedKind::Happening(f.id);
            if !used(conn, Some(p), &k)? {
                push(k, Vec::new())?;
                break;
            }
        }
    }
    out.extend(visit_candidates(conn, &enabled, now, pace)?);
    for w in words {
        if !enabled.contains(&w.person) {
            continue;
        }
        // Only words he could have heard: the user's, in a channel he takes part in.
        let m = message::get(conn, w.message)?;
        let heard = m.author == crate::Actor::User
            && channel::get(conn, m.channel)?.has(&crate::Actor::Person(w.person.clone()));
        let kind = SeedKind::Words(w.message);
        if heard && !used(conn, Some(&w.person), &kind)? {
            out.push(Candidate {
                person: w.person.clone(),
                kind,
                material: w.material.clone(),
                involved: Vec::new(),
                origin: None,
                last_seen: last_seen(conn, &w.person)?,
            });
        }
    }
    Ok(out)
}

/// Visits: an enabled person the user has not talked with for the tier's `visit_days` (and
/// who has not opened for that long) may drop by; a member of an enabled group silent
/// for the tier's `group_quiet_hours` may speak in it. They have nothing to say, so they come
/// after every seed that has.
fn visit_candidates(
    conn: &Connection,
    enabled: &[PersonId],
    now: Now,
    pace: &Pace,
) -> Result<Vec<Candidate>> {
    let d = now.local_date();
    let kind = SeedKind::Visit(d.year * 10_000 + i32::from(d.month) * 100 + i32::from(d.day));
    let away = now.ms - i64::from(pace.visit_days) * clock::DAY_MS;
    let quiet = now.ms - i64::from(pace.group_quiet_hours) * clock::HOUR_MS;
    let mut out = Vec::new();
    for p in enabled {
        let seen = last_seen(conn, p)?;
        let candidate = |origin: Option<ChannelId>| Candidate {
            person: p.clone(),
            kind: kind.clone(),
            material: Vec::new(),
            involved: Vec::new(),
            origin,
            last_seen: seen,
        };
        for g in channel::enabled_groups_of(conn, p)? {
            let silent = message::last(conn, g.id)?.is_none_or(|m| m.at <= quiet);
            if silent && !g.archived {
                out.push(candidate(Some(g.id)));
            }
        }
        if seen.is_none_or(|t| t <= away) && delivered_count(conn, Some(p), away, now.ms)? == 0 {
            out.push(candidate(None));
        }
    }
    Ok(out)
}

/// The ordering rule: events first, a visit last (one in a group before one in a direct
/// chat), and within each whoever has gone longest without talking with the user (never =
/// longest). Ties break by person, then
/// seed, for determinism.
pub fn order(candidates: &mut [Candidate]) {
    let key = |c: &Candidate| {
        (
            !c.kind.is_event(),
            matches!(c.kind, SeedKind::Visit(_)),
            c.origin.is_none(),
            c.last_seen.unwrap_or(i64::MIN),
        )
    };
    candidates.sort_by(|a, b| {
        key(a)
            .cmp(&key(b))
            .then_with(|| (&a.person, &a.kind, a.origin).cmp(&(&b.person, &b.kind, b.origin)))
    });
}

/// Every candidate for the occasion, ordered.
pub fn candidates(
    conn: &Connection,
    occasion: &Occasion<'_>,
    now: Now,
    pace: &Pace,
) -> Result<Vec<Candidate>> {
    let mut out = commitment_candidates(conn, now.ms)?;
    out.extend(queued_candidates(conn)?);
    if let Occasion::Presence { words } = occasion {
        out.extend(daily_candidates(conn, now, words, pace)?);
    }
    order(&mut out);
    Ok(out)
}

/// When the newest unprompted opening was delivered, or the newest daily event made.
pub fn last_opening(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT nullif(max(coalesce((SELECT max(delivered_at) FROM seed), 0), \
                coalesce((SELECT max(at) FROM daily_event), 0)), 0)",
        [],
        |r| r.get(0),
    )?)
}

/// The waking day cut into `total` equal parts, each holding one slot at a place drawn
/// from the date (and `salt`, so two kinds of slot do not fall together), so what is
/// spread over the day does not come together.
pub struct Slots {
    /// Minutes into the waking day.
    since: u32,
    part: u32,
    total: u32,
    len: u32,
    day: u64,
    salt: u64,
}

impl Slots {
    pub fn new(quiet: Option<&QuietHours>, now: Now, total: u32, salt: u64) -> Self {
        let (start, len) = clock::waking_window(quiet);
        let total = total.max(1);
        let d = now.local_date();
        Self {
            since: (now.local_minute() + 1440 - start) % 1440,
            part: (len / total).max(1),
            total,
            len,
            day: u64::from(d.year.unsigned_abs()) * 400
                + u64::from(d.month) * 31
                + u64::from(d.day),
            salt,
        }
    }

    /// Minutes into the waking day at which slot `i` comes.
    pub fn at(&self, i: u32) -> u32 {
        let mix = (self.day * 31 + u64::from(i) + 1 + self.salt * 1000)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.part * i + u32::try_from((mix >> 33) % u64::from(self.part)).unwrap_or(0)
    }

    /// How many slots have come by now.
    pub fn due(&self) -> u32 {
        (0..self.total)
            .filter(|i| self.at(*i) <= self.since)
            .count() as u32
    }

    /// The part of the day now is in, as `[from, to)` instants, and whether its slot has
    /// come. The last part runs to the end of the waking day.
    pub fn current(&self, now: Now) -> (i64, i64, bool) {
        let i = (self.since / self.part).min(self.total - 1);
        let minute = now.ms - now.ms.rem_euclid(clock::MINUTE_MS);
        let from = minute - i64::from(self.since - self.part * i) * clock::MINUTE_MS;
        let end = if i + 1 == self.total {
            self.len
        } else {
            self.part * (i + 1)
        };
        let to = from + i64::from(end - self.part * i) * clock::MINUTE_MS;
        (from, to, self.at(i) <= self.since)
    }

    /// The instant the waking day (the one `now` is in) began.
    fn start(&self, now: Now) -> i64 {
        now.ms - now.ms.rem_euclid(clock::MINUTE_MS) - i64::from(self.since) * clock::MINUTE_MS
    }

    /// How many slots came after `since` and by now: what a stretch away spent.
    pub fn passed_since(&self, now: Now, since: i64) -> u32 {
        let start = self.start(now);
        (0..self.total)
            .filter(|i| {
                self.at(*i) <= self.since
                    && start + i64::from(self.at(*i)) * clock::MINUTE_MS > since
            })
            .count() as u32
    }

    /// The end of the part now is in, when its slot came after `since`: that part's slot
    /// was spent away.
    pub fn part_spent_since(&self, now: Now, since: i64) -> Option<i64> {
        let i = (self.since / self.part).min(self.total - 1);
        let came = self.start(now) + i64::from(self.at(i)) * clock::MINUTE_MS;
        (self.at(i) <= self.since && came > since).then(|| self.current(now).1)
    }
}

/// Unprompted openings delivered in `[since, now]`, optionally for one person.
pub fn delivered_count(
    conn: &Connection,
    person: Option<&PersonId>,
    since: i64,
    now: i64,
) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM seed WHERE delivered_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR person = ?3)",
        (since, now, person),
        |r| r.get(0),
    )?)
}

/// Whether the user is in a conversation: he wrote somewhere within the idle timeout.
pub fn in_conversation(conn: &Connection, now: i64, session: &SessionParams) -> Result<bool> {
    Ok(message::last_user_message_at(conn)?.is_some_and(|t| now - t < session.idle_timeout_ms()))
}

/// Where a candidate opens: an enabled group he is in that the seed matches (everyone
/// it involves is there, or its topic matches), else his direct channel, which needs him
/// enabled unless the seed is a due commitment. `None`: nowhere.
fn place(conn: &Connection, c: &Candidate, topic: &dyn TopicMatch) -> Result<Option<ChannelId>> {
    for g in channel::enabled_groups_of(conn, &c.person)? {
        let members = g.persons();
        let all_there = !c.involved.is_empty() && c.involved.iter().all(|p| members.contains(p));
        if Some(g.id) == c.origin || all_there || topic.matches(c, &g) {
            return Ok(Some(g.id));
        }
    }
    let mode = bond::mode(conn, &c.person)?;
    let may = mode == Mode::Enabled
        || (mode == Mode::Frozen && matches!(c.kind, SeedKind::Commitment(_)));
    if !may {
        return Ok(None);
    }
    Ok(Some(channel::direct(conn, &c.person)?))
}

/// Everything [`plan`] needs besides the database.
pub struct Limits<'a> {
    pub seed: &'a SeedParams,
    /// What the user's tier sets of the pacing ([`Pacing::get`](crate::params::Pacing::get)).
    pub pace: &'a Pace,
    pub session: &'a SessionParams,
    /// The current quota status ([`quota::status`]); on the tier without windows it is
    /// always open, so only the experience limits apply.
    pub quota: &'a quota::Status,
}

/// Pick at most one opening for this occasion, or say why nobody opens.
pub fn plan(
    conn: &Connection,
    occasion: &Occasion<'_>,
    now: Now,
    limits: &Limits<'_>,
    topic: &dyn TopicMatch,
) -> Result<Planned> {
    if away::is_away(conn)? {
        return Ok(Planned::Held(Held::Away));
    }
    let all = candidates(conn, occasion, now, limits.pace)?;
    if all.is_empty() {
        return Ok(Planned::Held(Held::NoSeed));
    }
    if limits.quota.band != Band::Open {
        return Ok(Planned::Held(Held::Quota));
    }
    if settings::quiet_hours(conn, limits.seed)?.is_some_and(|q| clock::in_quiet_hours(&q, now)) {
        return Ok(Planned::Held(Held::QuietHours));
    }
    if in_conversation(conn, now.ms, limits.session)? {
        return Ok(Planned::Held(Held::InConversation));
    }
    let day = now.local_day_start();
    // Slots that passed while he was away count as used.
    let today = delivered_count(conn, None, day, now.ms)? + away::spent_slots(conn, now)?;
    if today >= limits.pace.daily_total {
        return Ok(Planned::Held(Held::DailyTotal));
    }
    if matches!(occasion, Occasion::Presence { .. }) {
        if clock::too_soon(last_opening(conn)?, now.ms, limits.pace.gap_min) {
            return Ok(Planned::Held(Held::TooSoon));
        }
        let quiet = settings::quiet_hours(conn, limits.seed)?;
        if Slots::new(quiet.as_ref(), now, limits.pace.daily_total, 0).due() <= today {
            return Ok(Planned::Held(Held::NotYet));
        }
    }
    for c in all {
        if delivered_count(conn, Some(&c.person), day, now.ms)? >= limits.seed.per_agent_day {
            continue;
        }
        let Some(channel) = place(conn, &c, topic)? else {
            continue;
        };
        let fact = match c.kind {
            SeedKind::Commitment(f)
            | SeedKind::Conclusion(f)
            | SeedKind::Memory(f)
            | SeedKind::Happening(f) => Some(f),
            _ => None,
        };
        return Ok(Planned::Open(Opening {
            person: c.person,
            channel,
            kind: c.kind,
            material: c.material,
            fact,
        }));
    }
    Ok(Planned::Held(Held::PerAgent))
}

/// The agent made the opening call and stored `message`: the seed is used, so the same
/// thing is brought up once. A due commitment is marked delivered and, for a frozen
/// person, thaws him for one session in which he may answer the user's reply.
pub fn delivered(conn: &Connection, opening: &Opening, message: MessageId, now: i64) -> Result<()> {
    let (k, r) = opening.kind.encode();
    conn.execute(
        "INSERT INTO seed(person, kind, ref, created_at, delivered_at, channel, message, material)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)
         ON CONFLICT(person, kind, ref) DO UPDATE SET
             delivered_at = excluded.delivered_at, channel = excluded.channel,
             message = excluded.message",
        (
            &opening.person,
            k,
            &r,
            now,
            opening.channel,
            message,
            serde_json::to_string(&opening.material)?,
        ),
    )?;
    let cause = match opening.kind {
        SeedKind::Commitment(f) => {
            fact::mark_delivered(conn, f)?;
            Cause::Commitment
        }
        _ => Cause::Opening,
    };
    session::open(
        conn,
        opening.channel,
        &opening.person,
        Budget::User,
        cause,
        now,
    )?;
    Ok(())
}

/// A delivered opening, for the digest's "came by".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub person: PersonId,
    pub channel: ChannelId,
    pub message: MessageId,
    pub at: i64,
}

/// Delivered openings whose message the user has not read.
pub fn unread_visits(conn: &Connection) -> Result<Vec<Visit>> {
    let mut stmt = conn.prepare(
        "SELECT s.person, s.channel, s.message, s.delivered_at FROM seed s
         JOIN channel c ON c.id = s.channel
         WHERE s.delivered_at IS NOT NULL AND s.message IS NOT NULL AND c.deleted_at IS NULL
           AND (c.read_up_to IS NULL OR c.read_up_to < s.message)
         ORDER BY s.delivered_at",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Visit {
            person: r.get(0)?,
            channel: r.get(1)?,
            message: r.get(2)?,
            at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::NewFact;
    use crate::params::{Params, Quota};
    use crate::testing::*;
    use crate::types::{Actor, Origin, Tier};

    struct Env {
        seed: SeedParams,
        pace: Pace,
        session: SessionParams,
        quota: quota::Status,
    }

    impl Env {
        fn new(w: &Connection) -> Self {
            Self {
                seed: SeedParams::default(),
                pace: Pace::default(),
                session: SessionParams::default(),
                quota: quota::status(w, Tier::Middle, T0, &Quota::default()).unwrap(),
            }
        }

        fn limits(&self) -> Limits<'_> {
            Limits {
                seed: &self.seed,
                pace: &self.pace,
                session: &self.session,
                quota: &self.quota,
            }
        }
    }

    fn enable(w: &Connection, ps: &[&str]) {
        for x in ps {
            bond::set_mode(w, &p(x), Mode::Enabled).unwrap();
        }
    }

    fn memory_about_user(w: &Connection, person: &str, text: &str, at: i64) -> FactId {
        let c = channel::direct(w, &p(person)).unwrap();
        fact::write(
            w,
            &NewFact::new(
                Actor::Person(p(person)),
                Audience::Participants(c),
                FactKind::Fact,
                text,
            )
            .about(About::User),
            at,
        )
        .unwrap()
        .id()
    }

    fn presence(w: &Connection, env: &Env, now: Now) -> Planned {
        plan(
            w,
            &Occasion::Presence { words: &[] },
            now,
            &env.limits(),
            &NoTopic,
        )
        .unwrap()
    }

    fn deliver(w: &Connection, o: &Opening, at: i64) {
        let m = message::append(
            w,
            o.channel,
            &Actor::Person(o.person.clone()),
            "hello",
            None,
            None,
            at,
        )
        .unwrap();
        delivered(w, o, m, at).unwrap();
    }

    #[test]
    fn nothing_without_a_seed_and_only_enabled_people() {
        let w = world();
        let env = Env::new(&w);
        memory_about_user(&w, "frozen", "likes rain", 1);
        assert_eq!(presence(&w, &env, at(T0)), Planned::Held(Held::NoSeed));
        enable(&w, &["frozen"]);
        assert!(matches!(presence(&w, &env, at(T0)), Planned::Open(_)));
    }

    #[test]
    fn events_first_then_longest_unseen() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a", "b", "c"]);
        memory_about_user(&w, "a", "m-a", 1);
        memory_about_user(&w, "b", "m-b", 1);
        bond::touch_user(&w, &p("a"), 10).unwrap();
        bond::touch_user(&w, &p("b"), 5).unwrap();
        let Planned::Open(o) = presence(&w, &env, at(T0)) else {
            panic!()
        };
        assert_eq!(o.person, p("b"), "b has gone longer without talking");
        let conclusion = fact::write(
            &w,
            &NewFact::new(Actor::User, Audience::World, FactKind::Conclusion, "x"),
            2,
        )
        .unwrap()
        .id();
        queue_hits(
            &w,
            Event::Conclusion(conclusion),
            &[Hit {
                person: p("c"),
                material: vec!["u1".into()],
            }],
            3,
        )
        .unwrap();
        bond::touch_user(&w, &p("c"), 99).unwrap();
        let Planned::Open(o) = presence(&w, &env, at(T0)) else {
            panic!()
        };
        assert_eq!(
            (o.person.clone(), o.kind.clone()),
            (p("c"), SeedKind::Conclusion(conclusion))
        );
        assert_eq!(o.material, vec!["u1".to_owned()]);
    }

    #[test]
    fn queued_hits_skip_people_not_enabled() {
        let w = world();
        enable(&w, &["a"]);
        bond::set_mode(&w, &p("off"), Mode::Disabled).unwrap();
        let case = task::open_case(&w, "q", 1).unwrap();
        let hits = [
            Hit {
                person: p("a"),
                material: vec![],
            },
            Hit {
                person: p("frozen"),
                material: vec![],
            },
            Hit {
                person: p("off"),
                material: vec![],
            },
        ];
        assert_eq!(queue_hits(&w, Event::Pinned(case), &hits, 1).unwrap(), 1);
        assert_eq!(queue_hits(&w, Event::Pinned(case), &hits, 2).unwrap(), 0);
        let cands = candidates(&w, &Occasion::Event, at(T0), &Pace::default()).unwrap();
        assert_eq!(cands.len(), 1);
        task::set_pinned(&w, case, false).unwrap();
        assert!(
            candidates(&w, &Occasion::Event, at(T0), &Pace::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_due_commitment_thaws_a_frozen_person_in_his_direct_channel() {
        let w = world();
        let env = Env::new(&w);
        let c = channel::direct(&w, &p("fz")).unwrap();
        let f = fact::write(
            &w,
            &NewFact::new(
                Actor::User,
                Audience::Participants(c),
                FactKind::Commitment,
                "call back",
            )
            .due(T0 - 1),
            1,
        )
        .unwrap()
        .id();
        let Planned::Open(o) = plan(&w, &Occasion::Event, at(T0), &env.limits(), &NoTopic).unwrap()
        else {
            panic!()
        };
        assert_eq!((o.person.clone(), o.channel, o.fact), (p("fz"), c, Some(f)));
        assert!(!session::is_thawed(&w, &p("fz")).unwrap());
        deliver(&w, &o, T0);
        assert!(fact::get(&w, f).unwrap().delivered);
        assert!(session::is_thawed(&w, &p("fz")).unwrap());
        assert_eq!(bond::mode(&w, &p("fz")).unwrap(), Mode::Frozen);
        assert_eq!(
            plan(
                &w,
                &Occasion::Event,
                at(T0 + 2 * HOUR),
                &env.limits(),
                &NoTopic
            )
            .unwrap(),
            Planned::Held(Held::NoSeed),
            "brought up once"
        );
        // A disabled person's commitment is never a seed.
        bond::set_mode(&w, &p("off"), Mode::Disabled).unwrap();
        let g = channel::create_group(&w, &[p("fz")], None, Origin::User).unwrap();
        channel::add_participant(&w, g, &Actor::Host).unwrap();
        let _ = g;
    }

    #[test]
    fn limits_hold_seeds_back_without_losing_them() {
        let w = world();
        let mut env = Env::new(&w);
        enable(&w, &["a"]);
        memory_about_user(&w, "a", "m1", 1);
        // Quiet hours (default 23:00–08:00) at 02:00 UTC.
        let night = at(T0 - 10 * HOUR);
        assert_eq!(presence(&w, &env, night), Planned::Held(Held::QuietHours));
        // In conversation: the user wrote five minutes ago.
        let other = channel::direct(&w, &p("x")).unwrap();
        message::append(&w, other, &Actor::User, "hey", None, None, T0 - 5 * 60_000).unwrap();
        assert_eq!(
            presence(&w, &env, at(T0)),
            Planned::Held(Held::InConversation)
        );
        let later = at(T0 + HOUR);
        let Planned::Open(o) = presence(&w, &env, later) else {
            panic!()
        };
        deliver(&w, &o, later.ms);
        // Per person per day.
        memory_about_user(&w, "a", "m2", 2);
        assert_eq!(
            presence(&w, &env, at(T0 + 3 * HOUR)),
            Planned::Held(Held::PerAgent)
        );
        // Next local day it goes out: the held seed was kept.
        let Planned::Open(o2) = presence(&w, &env, at(T0 + DAY)) else {
            panic!()
        };
        assert_eq!(o2.kind, SeedKind::Memory(FactId(2)));
        // Quota band not open.
        env.quota.band = Band::Quiet;
        assert_eq!(presence(&w, &env, at(T0 + DAY)), Planned::Held(Held::Quota));
        env.quota.band = Band::Open;
        env.pace.daily_total = 0;
        assert_eq!(
            presence(&w, &env, at(T0 + DAY)),
            Planned::Held(Held::DailyTotal)
        );
    }

    #[test]
    fn birthday_and_words_are_daily_seeds() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a", "b"]);
        settings::set(&w, settings::USER_BIRTHDAY, "03-10").unwrap();
        let cands = candidates(
            &w,
            &Occasion::Presence { words: &[] },
            at(T0),
            &Pace::default(),
        )
        .unwrap();
        assert_eq!(
            cands
                .iter()
                .filter(|c| matches!(c.kind, SeedKind::Birthday(2026)))
                .count(),
            2
        );
        assert!(
            candidates(&w, &Occasion::Event, at(T0), &Pace::default())
                .unwrap()
                .is_empty(),
            "daily seeds only on presence"
        );
        let next = candidates(
            &w,
            &Occasion::Presence { words: &[] },
            at(T0 + DAY),
            &Pace::default(),
        )
        .unwrap();
        assert!(
            next.iter().all(|c| matches!(c.kind, SeedKind::Visit(_))),
            "no reason left but a visit"
        );

        let ca = channel::direct(&w, &p("a")).unwrap();
        let m = message::append(&w, ca, &Actor::User, "about ships", None, None, 1).unwrap();
        let words = [
            WordsHit {
                person: p("a"),
                message: m,
                material: vec!["u9".into()],
            },
            // b never heard it.
            WordsHit {
                person: p("b"),
                message: m,
                material: vec![],
            },
        ];
        let cands = candidates(
            &w,
            &Occasion::Presence { words: &words },
            at(T0 + DAY),
            &Pace::default(),
        )
        .unwrap();
        let said: Vec<_> = cands
            .iter()
            .filter(|c| !matches!(c.kind, SeedKind::Visit(_)))
            .collect();
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].person, p("a"));
        let _ = env;
    }

    #[test]
    fn opens_in_a_matching_enabled_group() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a", "b"]);
        let g = channel::create_group(&w, &[p("a"), p("b")], Some("ships"), Origin::User).unwrap();
        let f = fact::write(
            &w,
            &NewFact::new(
                Actor::User,
                Audience::Participants(g),
                FactKind::Commitment,
                "meet",
            )
            .due(T0 - 1),
            1,
        )
        .unwrap()
        .id();
        // The group is frozen: he opens in his direct channel.
        let Planned::Open(o) = plan(&w, &Occasion::Event, at(T0), &env.limits(), &NoTopic).unwrap()
        else {
            panic!()
        };
        assert_ne!(o.channel, g);
        channel::set_mode(&w, g, Mode::Enabled).unwrap();
        let Planned::Open(o) = plan(&w, &Occasion::Event, at(T0), &env.limits(), &NoTopic).unwrap()
        else {
            panic!()
        };
        assert_eq!((o.channel, o.fact), (g, Some(f)));

        struct Ships;
        impl TopicMatch for Ships {
            fn matches(&self, _: &Candidate, g: &Channel) -> bool {
                g.topic.as_deref() == Some("ships")
            }
        }
        memory_about_user(&w, "a", "likes ships", 2);
        fact::mark_delivered(&w, f).unwrap();
        let Planned::Open(o) = presence(&w, &env, at(T0)) else {
            panic!()
        };
        assert_ne!(
            o.channel, g,
            "a single-person seed does not pick a group by members"
        );
        let Planned::Open(o) = plan(
            &w,
            &Occasion::Presence { words: &[] },
            at(T0),
            &env.limits(),
            &Ships,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(o.channel, g);
    }

    #[test]
    fn an_enabled_person_with_no_other_reason_drops_by_after_days_of_silence() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a"]);
        // Never talked with: he may drop by in his own chat.
        let Planned::Open(o) = presence(&w, &env, at(T0)) else {
            panic!("a visit")
        };
        assert_eq!(o.person, p("a"));
        assert!(matches!(o.kind, SeedKind::Visit(_)));
        assert_eq!(o.channel, channel::direct(&w, &p("a")).unwrap());
        deliver(&w, &o, T0);
        // Not again within `visit_days`, however many occasions come.
        let gap = i64::from(env.pace.visit_days) * DAY;
        assert_eq!(
            presence(&w, &env, at(T0 + gap - HOUR)),
            Planned::Held(Held::NoSeed)
        );
        // Then he may come again.
        assert!(matches!(
            presence(&w, &env, at(T0 + gap + HOUR)),
            Planned::Open(_)
        ));
        // Someone the user talked with lately has no reason to come by.
        bond::touch_user(&w, &p("a"), T0 + gap).unwrap();
        assert_eq!(
            presence(&w, &env, at(T0 + gap + HOUR)),
            Planned::Held(Held::NoSeed)
        );
    }

    #[test]
    fn a_days_openings_spread_over_the_waking_day() {
        let w = world();
        let env = Env::new(&w);
        let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        enable(&w, &names);
        // A fake clock over two local days, ticking every ten minutes; every tick asks.
        let midnight = T0 - 12 * HOUR;
        let step = 10 * 60_000;
        let mut opened: Vec<i64> = Vec::new();
        let mut t = midnight;
        while t < midnight + 2 * DAY {
            if let Planned::Open(o) = presence(&w, &env, at(t)) {
                deliver(&w, &o, t);
                opened.push(t);
            }
            t += step;
        }
        let gap = i64::from(env.pace.gap_min) * 60_000;
        let quiet = env.seed.quiet_hours.clone();
        assert_eq!(opened.len(), 2 * env.pace.daily_total as usize);
        for pair in opened.windows(2) {
            assert!(pair[1] - pair[0] >= gap, "{pair:?} closer than the gap");
        }
        for ms in &opened {
            assert!(!clock::in_quiet_hours(&quiet, at(*ms)), "{ms} is quiet");
        }
        // The first day: one opening in each fifth of the waking day (08:00 to 23:00).
        let part = 3 * HOUR;
        for (i, ms) in opened.iter().take(5).enumerate() {
            let since = ms - midnight - 8 * HOUR;
            assert!((i as i64 * part..(i as i64 + 1) * part).contains(&since));
        }
    }

    #[test]
    fn slots_that_pass_while_away_are_spent_and_nothing_is_released_on_return() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a", "b", "c", "d", "e", "f", "g", "h"]);
        let midnight = T0 - 12 * HOUR;
        let step = 10 * 60_000;
        // Away from 08:00 to 13:59: the first two parts of the day go by.
        let params = Params::default();
        away::begin(&w, midnight + 8 * HOUR).unwrap();
        let mut t = midnight + 8 * HOUR;
        while t < midnight + 14 * HOUR - 60_000 {
            assert_eq!(presence(&w, &env, at(t)), Planned::Held(Held::Away));
            t += step;
        }
        away::end(&w, at(midnight + 14 * HOUR - 60_000), &params)
            .unwrap()
            .unwrap();
        // The rest of the day gives the three parts left, one each, and never two at once.
        let mut opened: Vec<i64> = Vec::new();
        let mut t = midnight + 14 * HOUR;
        while t < midnight + DAY {
            if let Planned::Open(o) = presence(&w, &env, at(t)) {
                deliver(&w, &o, t);
                opened.push(t);
            }
            t += step;
        }
        assert_eq!(opened.len(), 3, "{opened:?}");
        let gap = i64::from(env.pace.gap_min) * 60_000;
        assert!(opened.windows(2).all(|p| p[1] - p[0] >= gap));
        // The next day is whole again.
        let mut next = 0;
        let mut t = midnight + DAY;
        while t < midnight + 2 * DAY {
            if let Planned::Open(o) = presence(&w, &env, at(t)) {
                deliver(&w, &o, t);
                next += 1;
            }
            t += step;
        }
        assert_eq!(next, env.pace.daily_total);
    }

    #[test]
    fn the_lowest_and_the_unlimited_tier_open_as_tabled_and_keep_the_ceilings() {
        let table = crate::params::Pacing::default();
        for (tier, per_day, gap) in [(Tier::Low, 2, 30), (Tier::Ultra, 10, 10)] {
            let w = world();
            let mut env = Env::new(&w);
            env.pace = table.get(tier);
            let names: Vec<String> = (0..14).map(|i| format!("p{i}")).collect();
            enable(&w, &names.iter().map(String::as_str).collect::<Vec<_>>());
            let midnight = T0 - 12 * HOUR;
            let mut opened: Vec<i64> = Vec::new();
            let mut t = midnight;
            while t < midnight + 2 * DAY {
                if let Planned::Open(o) = presence(&w, &env, at(t)) {
                    deliver(&w, &o, t);
                    opened.push(t);
                }
                t += 5 * 60_000;
            }
            assert_eq!(opened.len(), 2 * per_day, "{tier:?}");
            assert!(opened.len() <= 2 * crate::params::VISITS_CEILING as usize);
            for pair in opened.windows(2) {
                assert!(pair[1] - pair[0] >= gap * 60_000, "{tier:?}: {pair:?}");
            }
        }
    }

    #[test]
    fn a_visit_comes_after_every_seed_that_has_something_to_say() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a", "b"]);
        // `a` has gone longest unseen; `b` has a memory worth asking after.
        bond::touch_user(&w, &p("b"), 1).unwrap();
        let m = memory_about_user(&w, "b", "likes tea", 1);
        let Planned::Open(o) = presence(&w, &env, at(T0 + 10 * DAY)) else {
            panic!()
        };
        assert_eq!((o.person, o.kind), (p("b"), SeedKind::Memory(m)));
    }

    #[test]
    fn a_silent_enabled_group_gets_a_word_from_a_member_and_a_busy_one_does_not() {
        let w = world();
        let env = Env::new(&w);
        enable(&w, &["a", "b"]);
        let g = channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        channel::set_mode(&w, g, Mode::Enabled).unwrap();
        let quiet = i64::from(env.pace.group_quiet_hours) * HOUR;
        message::append(&w, g, &Actor::User, "hi all", None, None, T0 - quiet + HOUR).unwrap();
        // Spoken in lately; the people's own chats are ignored here by their bond.
        bond::touch_user(&w, &p("a"), T0).unwrap();
        bond::touch_user(&w, &p("b"), T0).unwrap();
        assert_eq!(
            presence(&w, &env, at(T0 + 7 * HOUR)),
            Planned::Open(Opening {
                person: p("a"),
                channel: g,
                kind: SeedKind::Visit(20260310),
                material: Vec::new(),
                fact: None,
            })
        );
        // Archived groups are left alone.
        channel::set_archived(&w, g, true).unwrap();
        assert_eq!(
            presence(&w, &env, at(T0 + 7 * HOUR)),
            Planned::Held(Held::NoSeed)
        );
        channel::set_archived(&w, g, false).unwrap();
        // Spoken in again, it is no longer silent.
        message::append(
            &w,
            g,
            &Actor::User,
            "more",
            None,
            None,
            T0 + 7 * HOUR - 1000,
        )
        .unwrap();
        assert_eq!(
            presence(&w, &env, at(T0 + 7 * HOUR)),
            Planned::Held(Held::NoSeed)
        );
    }
}
