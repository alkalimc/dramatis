//! Seeds: the only reason anyone speaks unprompted.
//!
//! Event seeds (a commitment falling due, a conclusion or pinned question that hit a
//! person's own units) come before daily ones (the user's birthday, a memory about the
//! user worth asking after, the user's recent words hitting his units); within a class
//! whoever has gone longest without talking with the user goes first. At most one person
//! opens per occasion, and four experience limits hold on every tier. A seed held back
//! stays undelivered for the next chance.
//!
//! Retrieval is the caller's: hits arrive as person lists with the chunk ids that matched.

use rusqlite::{Connection, OptionalExtension};

use crate::channel::{self, Channel};
use crate::clock::{self, Now};
use crate::fact::{self, About, Audience};
use crate::params::{Seed as SeedParams, Session as SessionParams};
use crate::quota::{self, Band};
use crate::session::{self, Budget, Cause};
use crate::types::{ChannelId, FactId, FactKind, MessageId, Mode, PersonId, TaskId};
use crate::{Result, bond, message, settings, task};

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
    /// `seed.daily_total` reached.
    DailyTotal,
    /// Every candidate is at `seed.per_agent_day` or has nowhere to open.
    PerAgent,
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
            material: serde_json::from_str(&material).unwrap_or_default(),
            involved: Vec::new(),
            origin: None,
        });
    }
    Ok(out)
}

/// The three daily seeds for every enabled person.
fn daily_candidates(conn: &Connection, now: Now, words: &[WordsHit]) -> Result<Vec<Candidate>> {
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
        let memory = fact::visible_to_person(conn, p)?
            .into_iter()
            .filter(|f| f.kind == FactKind::Fact && f.about == Some(About::User))
            .rev()
            .map(|f| SeedKind::Memory(f.id))
            .find(|k| !used(conn, None, k).unwrap_or(true));
        if let Some(k) = memory {
            push(k, Vec::new())?;
        }
    }
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

/// The ordering rule: events first, then whoever has gone longest without talking with
/// the user (never = longest). Ties break by person, then seed, for determinism.
pub fn order(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        (
            !a.kind.is_event(),
            a.last_seen.unwrap_or(i64::MIN),
            &a.person,
            &a.kind,
        )
            .cmp(&(
                !b.kind.is_event(),
                b.last_seen.unwrap_or(i64::MIN),
                &b.person,
                &b.kind,
            ))
    });
}

/// Every candidate for the occasion, ordered.
pub fn candidates(conn: &Connection, occasion: &Occasion<'_>, now: Now) -> Result<Vec<Candidate>> {
    let mut out = commitment_candidates(conn, now.ms)?;
    out.extend(queued_candidates(conn)?);
    if let Occasion::Presence { words } = occasion {
        out.extend(daily_candidates(conn, now, words)?);
    }
    order(&mut out);
    Ok(out)
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
    let all = candidates(conn, occasion, now)?;
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
    if delivered_count(conn, None, day, now.ms)? >= limits.seed.daily_total {
        return Ok(Planned::Held(Held::DailyTotal));
    }
    for c in all {
        if delivered_count(conn, Some(&c.person), day, now.ms)? >= limits.seed.per_agent_day {
            continue;
        }
        let Some(channel) = place(conn, &c, topic)? else {
            continue;
        };
        let fact = match c.kind {
            SeedKind::Commitment(f) | SeedKind::Conclusion(f) | SeedKind::Memory(f) => Some(f),
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
    use crate::params::Quota;
    use crate::testing::*;
    use crate::types::{Actor, Origin, Tier};

    struct Env {
        seed: SeedParams,
        session: SessionParams,
        quota: quota::Status,
    }

    impl Env {
        fn new(w: &Connection) -> Self {
            Self {
                seed: SeedParams::default(),
                session: SessionParams::default(),
                quota: quota::status(w, Tier::Middle, T0, &Quota::default()).unwrap(),
            }
        }

        fn limits(&self) -> Limits<'_> {
            Limits {
                seed: &self.seed,
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
        let cands = candidates(&w, &Occasion::Event, at(T0)).unwrap();
        assert_eq!(cands.len(), 1);
        task::set_pinned(&w, case, false).unwrap();
        assert!(candidates(&w, &Occasion::Event, at(T0)).unwrap().is_empty());
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
        env.seed.daily_total = 0;
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
        let cands = candidates(&w, &Occasion::Presence { words: &[] }, at(T0)).unwrap();
        assert_eq!(
            cands
                .iter()
                .filter(|c| matches!(c.kind, SeedKind::Birthday(2026)))
                .count(),
            2
        );
        assert!(
            candidates(&w, &Occasion::Event, at(T0)).unwrap().is_empty(),
            "daily seeds only on presence"
        );
        assert!(
            candidates(&w, &Occasion::Presence { words: &[] }, at(T0 + DAY))
                .unwrap()
                .is_empty()
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
        let cands = candidates(&w, &Occasion::Presence { words: &words }, at(T0 + DAY)).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].person, p("a"));
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
}
