//! Presence: the user opened the app or came back from idle. Only then is anything
//! computed; nothing runs while he is away.
//!
//! First the digest (no model call): new messages, answered requests, due commitments,
//! who came by, whose birthday it is. When it is non-empty and the quota band allows,
//! the host says one in-world line about the items it has not announced yet. Then at most one person opens with a seed.

use std::collections::BTreeMap;

use rusqlite::Connection;

use crate::clock::{Date, Now, days_from_civil};
use crate::params::Params;
use crate::quota::{self, CallKind};
use crate::seed::{self, Held, Limits, Occasion, Planned, TopicMatch, WordsHit};
use crate::types::{ChannelId, FactId, MessageId, MonthDay, PersonId, TaskId, Tier};
use crate::{Call, Result, bond, channel, fact, message, session, settings, task};

/// Something that is waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Unread messages from him; `at` is the newest.
    Messaged { at: i64 },
    /// He answered a request.
    Replied {
        task: TaskId,
        question: String,
        at: i64,
    },
    /// A commitment with him is due and has not been brought up.
    CommitmentDue {
        fact: FactId,
        text: String,
        due: i64,
    },
    /// He opened with a seed and the user has not read it.
    CameBy { message: MessageId, at: i64 },
    /// It is his birthday today (his corpus profile's field). Only for someone the user
    /// has talked to: with hundreds of people, nearly every day has a birthday.
    Birthday,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub person: PersonId,
    /// Where a click goes; `None` when there is no channel with him yet.
    pub channel: Option<ChannelId>,
    pub activity: Activity,
}

/// The waiting panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Digest {
    pub items: Vec<Waiting>,
}

impl Digest {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The items the host has not announced yet. A birthday is brought up unprompted at
    /// most one in [`BIRTHDAY_GAP_DAYS`] days, and one at a time; the panel and the
    /// digest the user asks for still list them all.
    pub fn unannounced(&self, conn: &Connection, today: Date) -> Result<Digest> {
        let seen = announced(conn)?;
        let birthday_ok = match settings::get::<Date>(conn, BIRTHDAY_SAID)? {
            Some(last) => days_from_civil(today) - days_from_civil(last) >= BIRTHDAY_GAP_DAYS,
            None => true,
        };
        let mut birthday_left = usize::from(birthday_ok);
        let items = self
            .items
            .iter()
            .filter(|w| !seen.contains(&w.key(today)))
            .filter(|w| {
                if w.activity != Activity::Birthday {
                    return true;
                }
                let keep = birthday_left > 0;
                birthday_left = 0;
                keep
            })
            .cloned()
            .collect();
        Ok(Digest { items })
    }
}

impl Waiting {
    /// Stable identity of this item for as long as it is unchanged: a new message, reply
    /// or due time makes a new key, and a birthday is new each local day.
    fn key(&self, today: Date) -> String {
        let (kind, id) = match &self.activity {
            Activity::Messaged { at } => ("messaged", at.to_string()),
            Activity::Replied { task, at, .. } => ("replied", format!("{task:?}@{at}")),
            Activity::CommitmentDue { fact, due, .. } => ("due", format!("{fact:?}@{due}")),
            Activity::CameBy { message, at } => ("cameby", format!("{message:?}@{at}")),
            Activity::Birthday => (
                "birthday",
                format!("{}-{:02}-{:02}", today.year, today.month, today.day),
            ),
        };
        // A birthday stays the same item when a direct channel with him appears.
        let ch = match self.activity {
            Activity::Birthday => String::new(),
            _ => self.channel.map(|c| format!("{c:?}")).unwrap_or_default(),
        };
        format!("{}|{ch}|{kind}|{id}", self.person)
    }
}

const ANNOUNCED: &str = "presence.announced";

/// The local date the host last brought up a birthday on its own.
const BIRTHDAY_SAID: &str = "presence.birthday_said";

/// Days between two birthdays the host brings up unprompted.
pub const BIRTHDAY_GAP_DAYS: i64 = 7;

fn announced(conn: &Connection) -> Result<Vec<String>> {
    Ok(settings::get(conn, ANNOUNCED)?.unwrap_or_default())
}

/// Record that the host has told the user about `said`. Keys of items no longer in the
/// `full` digest are dropped, so the record stays as small as the panel.
pub fn mark_announced(conn: &Connection, full: &Digest, said: &Digest, today: Date) -> Result<()> {
    let mut keys = announced(conn)?;
    keys.extend(said.items.iter().map(|w| w.key(today)));
    if said.items.iter().any(|w| w.activity == Activity::Birthday) {
        settings::set(conn, BIRTHDAY_SAID, &today)?;
    }
    let live: Vec<String> = full.items.iter().map(|w| w.key(today)).collect();
    keys.retain(|k| live.contains(k));
    keys.sort();
    keys.dedup();
    settings::set(conn, ANNOUNCED, &keys)
}

/// Who has a birthday on a day: the corpus's profile field, supplied by the caller so
/// this crate never reads the corpus.
pub trait Birthdays {
    fn born_on(&self, day: MonthDay) -> Vec<PersonId>;
}

impl Birthdays for Vec<(PersonId, MonthDay)> {
    fn born_on(&self, day: MonthDay) -> Vec<PersonId> {
        self.iter()
            .filter(|(_, d)| *d == day)
            .map(|(p, _)| p.clone())
            .collect()
    }
}

/// The digest. Disabled people never appear in it. Order: activity kind (replies, due
/// commitments, visits, messages, birthdays), then time.
pub fn digest(conn: &Connection, now: Now, birthdays: &dyn Birthdays) -> Result<Digest> {
    let off = bond::disabled(conn)?;
    let mut items = Vec::new();

    let mut reply_msgs = Vec::new();
    for t in task::replied_unread(conn)? {
        let (Some(person), Some(reply)) = (t.assignee.clone(), t.reply) else {
            continue;
        };
        reply_msgs.push(reply);
        let at = message::get(conn, reply)?.at;
        items.push(Waiting {
            person,
            channel: t.channel,
            activity: Activity::Replied {
                task: t.id,
                question: t.question,
                at,
            },
        });
    }

    for f in fact::due_commitments(conn, now.ms)? {
        let reach = fact::audience_persons(conn, &f.audience)?.unwrap_or_default();
        let person = f
            .author
            .person()
            .filter(|p| reach.contains(p))
            .cloned()
            .or_else(|| reach.first().cloned());
        let Some(person) = person else { continue };
        let channel = match f.audience {
            fact::Audience::Participants(c) => Some(c),
            _ => channel::find_direct_with(conn, &person)?,
        };
        items.push(Waiting {
            person,
            channel,
            activity: Activity::CommitmentDue {
                fact: f.id,
                text: f.text,
                due: f.due.unwrap_or(now.ms),
            },
        });
    }

    let mut visit_msgs = Vec::new();
    for v in seed::unread_visits(conn)? {
        visit_msgs.push(v.message);
        items.push(Waiting {
            person: v.person,
            channel: Some(v.channel),
            activity: Activity::CameBy {
                message: v.message,
                at: v.at,
            },
        });
    }

    // Other unread messages, one row per person and channel.
    let mut newest: BTreeMap<(PersonId, ChannelId), i64> = BTreeMap::new();
    for ch in channel::list(conn)? {
        for m in message::since(conn, ch.id, ch.read_up_to)? {
            let Some(p) = m.author.person() else { continue };
            if reply_msgs.contains(&m.id) || visit_msgs.contains(&m.id) {
                continue;
            }
            let e = newest.entry((p.clone(), ch.id)).or_insert(m.at);
            *e = (*e).max(m.at);
        }
    }
    for ((person, channel), at) in newest {
        items.push(Waiting {
            person,
            channel: Some(channel),
            activity: Activity::Messaged { at },
        });
    }

    for person in birthdays.born_on(now.local_date().month_day()) {
        // Someone the user has not talked to is not news, whoever has a birthday.
        let Some(channel) = channel::find_direct_with(conn, &person)?
            .filter(|c| message::last(conn, *c).is_ok_and(|m| m.is_some()))
        else {
            continue;
        };
        let channel = Some(channel);
        items.push(Waiting {
            person,
            channel,
            activity: Activity::Birthday,
        });
    }

    items.retain(|w| !off.contains(&w.person));
    let rank = |a: &Activity| match a {
        Activity::Replied { at, .. } => (0, *at),
        Activity::CommitmentDue { due, .. } => (1, *due),
        Activity::CameBy { at, .. } => (2, *at),
        Activity::Messaged { at } => (3, *at),
        Activity::Birthday => (4, 0),
    };
    items.sort_by(|a, b| {
        rank(&a.activity)
            .cmp(&rank(&b.activity))
            .then(a.person.cmp(&b.person))
    });
    Ok(Digest { items })
}

/// Whether this moment is a presence occasion: the app was just opened (`opened`), or
/// the user is back after at least `session.idle_timeout` without writing.
pub fn is_return(conn: &Connection, now: i64, opened: bool, params: &Params) -> Result<bool> {
    if opened {
        return Ok(true);
    }
    Ok(!seed::in_conversation(conn, now, &params.session)?)
}

/// What presence computed, and the calls it asks for, in order: wrap-ups of
/// conversations that ended while idle, the host's line, one opening.
#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub digest: Digest,
    pub calls: Vec<Call>,
    /// Why nobody opens, when nobody does.
    pub held: Option<Held>,
}

/// The whole presence step. `words` are the user's recent words that hit an enabled
/// person's own units (the caller's retrieval over
/// [`message::user_words_seen_by`]). Calls the quota band forbids are not planned: in
/// the quiet band the host stays silent and nobody opens; exhausted, nothing is called
/// (wrap-ups included; their sessions are closed and the conversation is simply not
/// summarised).
pub fn on_presence(
    conn: &Connection,
    now: Now,
    tier: Tier,
    params: &Params,
    birthdays: &dyn Birthdays,
    words: &[WordsHit],
    topic: &dyn TopicMatch,
) -> Result<Presence> {
    let ended = session::expire_idle(conn, now.ms, &params.session)?;
    let status = quota::status(conn, tier, now.ms, &params.quota)?;
    let mut calls = Vec::new();
    if quota::decide(&status, CallKind::UserInitiated).allowed() {
        calls.extend(ended.into_iter().map(|channel| Call::Wrapup { channel }));
    }
    let digest = digest(conn, now, birthdays)?;
    // Only news: the same waiting items are not announced twice.
    let news = digest.unannounced(conn, now.local_date())?;
    if !news.is_empty() && quota::decide(&status, CallKind::HostLine).allowed() {
        calls.push(Call::HostLine(news));
    }
    let limits = Limits {
        seed: &params.seed,
        session: &params.session,
        quota: &status,
    };
    let held = match seed::plan(conn, &Occasion::Presence { words }, now, &limits, topic)? {
        Planned::Open(o) => {
            calls.push(Call::Opening(o));
            None
        }
        Planned::Held(h) => Some(h),
    };
    Ok(Presence {
        digest,
        calls,
        held,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{Audience, NewFact};
    use crate::params::Quota;
    use crate::quota::{Usage, record_usage};
    use crate::seed::NoTopic;
    use crate::testing::*;
    use crate::types::{Actor, FactKind, Mode, Shape};

    fn none() -> Vec<(PersonId, MonthDay)> {
        Vec::new()
    }

    #[test]
    fn empty_world_empty_digest_and_no_call() {
        let w = world();
        let pres = on_presence(
            &w,
            at(T0),
            Tier::Middle,
            &Params::default(),
            &none(),
            &[],
            &NoTopic,
        )
        .unwrap();
        assert!(pres.digest.is_empty());
        assert!(pres.calls.is_empty());
        assert_eq!(pres.held, Some(Held::NoSeed));
    }

    #[test]
    fn digest_collects_every_kind_and_skips_disabled() {
        let w = world();
        let office = std::env::temp_dir();
        // A reply.
        let a = task::ask(
            &w,
            &p("a"),
            "q",
            Some(3),
            None,
            Tier::Middle,
            &Default::default(),
            T0,
        )
        .unwrap();
        task::report(&w, &p("a"), a.task, "answer", &[], None, &office, T0 + 1).unwrap();
        // A message.
        let cb = channel::direct(&w, &p("b")).unwrap();
        message::append(&w, cb, &Actor::Person(p("b")), "hi", None, None, T0 + 2).unwrap();
        message::append(&w, cb, &Actor::Person(p("b")), "there", None, None, T0 + 3).unwrap();
        // A due commitment.
        let f = fact::write(
            &w,
            &NewFact::new(
                Actor::User,
                Audience::Participants(cb),
                FactKind::Commitment,
                "call",
            )
            .due(T0),
            0,
        )
        .unwrap()
        .id();
        // A disabled person's message.
        let cd = channel::direct(&w, &p("d")).unwrap();
        message::append(&w, cd, &Actor::Person(p("d")), "psst", None, None, T0).unwrap();
        bond::set_mode(&w, &p("d"), Mode::Disabled).unwrap();
        // The user has talked to e, not to f.
        let ce = channel::direct(&w, &p("e")).unwrap();
        message::append(&w, ce, &Actor::User, "hi", None, None, T0).unwrap();
        let born = vec![
            (p("e"), MonthDay { month: 3, day: 10 }),
            (p("f"), MonthDay { month: 3, day: 10 }),
        ];

        let d = digest(&w, at(T0 + 10), &born).unwrap();
        let kinds: Vec<_> = d
            .items
            .iter()
            .map(|i| (i.person.0.as_str(), &i.activity))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (
                    "a",
                    &Activity::Replied {
                        task: a.task,
                        question: "q".into(),
                        at: T0 + 1
                    }
                ),
                (
                    "b",
                    &Activity::CommitmentDue {
                        fact: f,
                        text: "call".into(),
                        due: T0
                    }
                ),
                ("b", &Activity::Messaged { at: T0 + 3 }),
                ("e", &Activity::Birthday),
            ]
        );
        // Reading clears what was read.
        let last = message::last(&w, cb).unwrap().unwrap().id;
        channel::mark_read(&w, cb, last).unwrap();
        let d = digest(&w, at(T0 + 10), &born).unwrap();
        assert_eq!(d.items.len(), 3);
    }

    #[test]
    fn host_speaks_only_about_a_non_empty_digest_while_the_band_is_open() {
        let w = world();
        let cb = channel::direct(&w, &p("b")).unwrap();
        message::append(&w, cb, &Actor::Person(p("b")), "hi", None, None, T0).unwrap();
        let params = Params::default();
        let pres = on_presence(
            &w,
            at(T0 + HOUR),
            Tier::Middle,
            &params,
            &none(),
            &[],
            &NoTopic,
        )
        .unwrap();
        assert!(matches!(pres.calls.as_slice(), [Call::HostLine(_)]));

        let mut quiet = params.clone();
        quiet.quota = Quota {
            quiet_at: 0.5,
            ..Quota::default()
        };
        quiet.quota.window_1h.middle = 100.0;
        quiet.quota.call_points = 1.0;
        record_usage(
            &w,
            T0 + HOUR,
            Shape::Direct,
            None,
            "m",
            Usage {
                uncached: 60,
                ..Usage::default()
            },
            None,
            &params.cost,
        )
        .unwrap();
        let pres = on_presence(
            &w,
            at(T0 + HOUR),
            Tier::Middle,
            &quiet,
            &none(),
            &[],
            &NoTopic,
        )
        .unwrap();
        assert!(!pres.digest.is_empty(), "the panel still shows");
        assert!(pres.calls.is_empty());
        let pres = on_presence(
            &w,
            at(T0 + HOUR),
            Tier::Ultra,
            &quiet,
            &none(),
            &[],
            &NoTopic,
        )
        .unwrap();
        assert_eq!(pres.calls.len(), 1, "no bands without windows");
    }

    #[test]
    fn announced_items_are_news_only_once() {
        let w = world();
        let cb = channel::direct(&w, &p("b")).unwrap();
        message::append(&w, cb, &Actor::Person(p("b")), "hi", None, None, T0).unwrap();
        let ce = channel::direct(&w, &p("e")).unwrap();
        message::append(&w, ce, &Actor::User, "hi", None, None, T0).unwrap();
        let born = vec![(p("e"), MonthDay { month: 3, day: 10 })];
        let params = Params::default();
        let run =
            |ms: i64| on_presence(&w, at(ms), Tier::Middle, &params, &born, &[], &NoTopic).unwrap();
        let first = run(T0 + HOUR);
        let [Call::HostLine(news)] = first.calls.as_slice() else {
            panic!("one line")
        };
        assert_eq!(news.items.len(), 2);
        mark_announced(&w, &first.digest, news, at(T0 + HOUR).local_date()).unwrap();
        let again = run(T0 + HOUR + 1);
        assert!(again.calls.is_empty());
        assert_eq!(again.digest.items.len(), 2, "the panel still shows both");
        // A new message is news; the birthday is not.
        message::append(
            &w,
            cb,
            &Actor::Person(p("b")),
            "hey",
            None,
            None,
            T0 + HOUR + 2,
        )
        .unwrap();
        let third = run(T0 + HOUR + 3);
        let [Call::HostLine(news)] = third.calls.as_slice() else {
            panic!("one line")
        };
        assert_eq!(news.items.len(), 1);
        assert_eq!(news.items[0].person, p("b"));
        // The birthday is a new item on another day.
        let b = &first.digest.items[1];
        assert_ne!(
            b.key(at(T0).local_date()),
            b.key(at(T0 + 24 * HOUR).local_date())
        );
    }

    #[test]
    fn a_birthday_is_brought_up_at_most_once_a_week_and_one_at_a_time() {
        let w = world();
        for who in ["e", "f"] {
            let c = channel::direct(&w, &p(who)).unwrap();
            message::append(&w, c, &Actor::User, "hi", None, None, T0).unwrap();
        }
        let params = Params::default();
        let day = 24 * HOUR;
        let on = |ms: i64, born: &Vec<(PersonId, MonthDay)>| {
            on_presence(&w, at(ms), Tier::Middle, &params, born, &[], &NoTopic).unwrap()
        };
        // Two known people and a stranger share the day: the panel lists the two the
        // user knows, the host brings up one.
        let md = at(T0).local_date().month_day();
        let born = vec![(p("e"), md), (p("f"), md), (p("z"), md)];
        let first = on(T0, &born);
        assert_eq!(first.digest.items.len(), 2);
        let [Call::HostLine(news)] = first.calls.as_slice() else {
            panic!("one line")
        };
        assert_eq!(news.items.len(), 1);
        mark_announced(&w, &first.digest, news, at(T0).local_date()).unwrap();
        // Days later, someone else's birthday is only in the panel.
        let md = at(T0 + 3 * day).local_date().month_day();
        let later = on(T0 + 3 * day, &vec![(p("f"), md)]);
        assert_eq!(later.digest.items.len(), 1);
        assert!(later.calls.is_empty(), "{:?}", later.calls);
        // A week on, it may be brought up again.
        let md = at(T0 + 7 * day).local_date().month_day();
        let week = on(T0 + 7 * day, &vec![(p("f"), md)]);
        assert!(matches!(week.calls.as_slice(), [Call::HostLine(_)]));
    }

    #[test]
    fn idle_conversations_end_with_a_wrapup_on_presence() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        session::open(
            &w,
            c,
            &p("a"),
            session::Budget::User,
            session::Cause::Addressed,
            T0,
        )
        .unwrap();
        let params = Params::default();
        assert!(is_return(&w, T0 + HOUR, false, &params).unwrap());
        let pres = on_presence(
            &w,
            at(T0 + HOUR),
            Tier::Middle,
            &params,
            &none(),
            &[],
            &NoTopic,
        )
        .unwrap();
        assert_eq!(pres.calls, vec![Call::Wrapup { channel: c }]);
    }
}
