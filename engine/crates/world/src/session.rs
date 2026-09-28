//! Triggered sessions. A frozen person never speaks first, but being addressed, asked,
//! pulled in or reaching a due commitment thaws him for one budgeted session; when it
//! ends he is frozen again. Modes are never rewritten for this: "thawed" is having an
//! open session, so ending one cannot leave anyone accidentally enabled.

use rusqlite::{Connection, OptionalExtension, Row};

use crate::params;
use crate::types::{ChannelId, Mode, PersonId, TaskId};
use crate::{Error, Result, bond, channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub i64);

/// What opened a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The user wrote to him or named him (in a group: he was the one addressed).
    Addressed,
    Ask,
    /// Pulled in by a colleague.
    Join,
    /// A commitment with him fell due.
    Commitment,
    /// He opened with a seed.
    Opening,
}

impl Cause {
    fn as_str(self) -> &'static str {
        match self {
            Self::Addressed => "addressed",
            Self::Ask => "ask",
            Self::Join => "join",
            Self::Commitment => "commitment",
            Self::Opening => "opening",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "ask" => Self::Ask,
            "join" => Self::Join,
            "commitment" => Self::Commitment,
            "opening" => Self::Opening,
            _ => Self::Addressed,
        }
    }
}

/// What a session may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// A request's turns ([`crate::task`]); ends when the request is done.
    Task(TaskId),
    /// A fixed number of replies (a colleague pulled into an ordinary conversation
    /// answers once); ends when they are used.
    Replies(u32),
    /// Each reply is paid for by the user's own turn; ends on idle or close.
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub channel: ChannelId,
    pub person: PersonId,
    pub cause: Cause,
    pub budget: Budget,
    pub opened_at: i64,
    pub last_at: i64,
    pub closed_at: Option<i64>,
}

const COLUMNS: &str = "id, channel, person, cause, task, replies_left, opened_at, last_at, closed_at";

fn read(row: &Row<'_>) -> rusqlite::Result<Session> {
    let task: Option<TaskId> = row.get("task")?;
    let replies: Option<u32> = row.get("replies_left")?;
    Ok(Session {
        id: SessionId(row.get("id")?),
        channel: row.get("channel")?,
        person: row.get("person")?,
        cause: Cause::parse(&row.get::<_, String>("cause")?),
        budget: match (task, replies) {
            (Some(t), _) => Budget::Task(t),
            (None, Some(n)) => Budget::Replies(n),
            (None, None) => Budget::User,
        },
        opened_at: row.get("opened_at")?,
        last_at: row.get("last_at")?,
        closed_at: row.get("closed_at")?,
    })
}

fn query(conn: &Connection, filter: &str, params: impl rusqlite::Params) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM session WHERE {filter} ORDER BY id"))?;
    let rows = stmt.query_map(params, read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(conn: &Connection, id: SessionId) -> Result<Session> {
    query(conn, "id = ?1", [id.0])?
        .pop()
        .ok_or_else(|| crate::not_found("session", id.0))
}

/// The person's open session in a channel.
pub fn current(conn: &Connection, channel: ChannelId, person: &PersonId) -> Result<Option<Session>> {
    Ok(query(
        conn,
        "channel = ?1 AND person = ?2 AND closed_at IS NULL",
        (channel, person),
    )?
    .pop())
}

/// Every open session.
pub fn open_sessions(conn: &Connection) -> Result<Vec<Session>> {
    query(conn, "closed_at IS NULL", [])
}

/// Open (or extend) a person's session in a channel. A disabled person cannot be
/// triggered. If one is already open there it is kept and its budget replaced when the
/// new one is narrower (a request), so a request opened mid-conversation is what the
/// session spends from then on.
pub fn open(
    conn: &Connection,
    channel: ChannelId,
    person: &PersonId,
    budget: Budget,
    cause: Cause,
    now: i64,
) -> Result<SessionId> {
    bond::ensure_available(conn, person)?;
    let ch = channel::get(conn, channel)?;
    if !ch.is_open() {
        return Err(Error::ChannelClosed(channel));
    }
    let (task, replies) = match budget {
        Budget::Task(t) => (Some(t), None),
        Budget::Replies(n) => (None, Some(n)),
        Budget::User => (None, None),
    };
    if let Some(s) = current(conn, channel, person)? {
        let replace = match (s.budget, budget) {
            (_, Budget::Task(_)) => true,
            (Budget::Replies(_), Budget::User) => true,
            _ => false,
        };
        if replace {
            conn.execute(
                "UPDATE session SET task = ?2, replies_left = ?3, cause = ?4, last_at = ?5 WHERE id = ?1",
                (s.id.0, task, replies, cause.as_str(), now),
            )?;
        } else {
            conn.execute("UPDATE session SET last_at = ?2 WHERE id = ?1", (s.id.0, now))?;
        }
        return Ok(s.id);
    }
    conn.execute(
        "INSERT INTO session(channel, person, cause, task, replies_left, opened_at, last_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        (channel, person, cause.as_str(), task, replies, now),
    )?;
    Ok(SessionId(conn.last_insert_rowid()))
}

/// Whether he may reply in this channel now, and why not. Enabled people may always
/// answer where they take part; anyone else needs an open session with budget left.
pub fn may_reply(conn: &Connection, channel: ChannelId, person: &PersonId) -> Result<bool> {
    let mode = bond::mode(conn, person)?;
    if mode == Mode::Disabled || !channel::get(conn, channel)?.is_open() {
        return Ok(false);
    }
    match current(conn, channel, person)? {
        Some(s) => Ok(match s.budget {
            Budget::Replies(n) => n > 0,
            Budget::Task(t) => crate::task::get(conn, t)?.status == crate::types::TaskStatus::Active,
            Budget::User => true,
        }),
        None => Ok(mode == Mode::Enabled),
    }
}

/// He replied: record the activity and spend a reply from a reply budget. A session
/// whose replies are used up closes; the returned channel then needs a wrap-up if no
/// other session there is still open.
pub fn replied(conn: &Connection, channel: ChannelId, person: &PersonId, now: i64) -> Result<Option<ChannelId>> {
    let Some(s) = current(conn, channel, person)? else {
        return Ok(None);
    };
    conn.execute("UPDATE session SET last_at = ?2 WHERE id = ?1", (s.id.0, now))?;
    if let Budget::Replies(n) = s.budget {
        let left = n.saturating_sub(1);
        conn.execute("UPDATE session SET replies_left = ?2 WHERE id = ?1", (s.id.0, left))?;
        if left == 0 {
            return close(conn, s.id, now);
        }
    }
    Ok(None)
}

/// Activity in a channel (a user message) keeps every open session there alive.
pub fn touch_channel(conn: &Connection, channel: ChannelId, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE session SET last_at = max(last_at, ?2) WHERE channel = ?1 AND closed_at IS NULL",
        (channel, now),
    )?;
    Ok(())
}

fn open_in(conn: &Connection, channel: ChannelId) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM session WHERE channel = ?1 AND closed_at IS NULL",
        [channel],
        |r| r.get(0),
    )?)
}

/// Close one session. Returns the channel when that was its last open session: the
/// conversation there has ended and needs a wrap-up.
pub fn close(conn: &Connection, id: SessionId, now: i64) -> Result<Option<ChannelId>> {
    let s = get(conn, id)?;
    if s.closed_at.is_some() {
        return Ok(None);
    }
    conn.execute("UPDATE session SET closed_at = ?2 WHERE id = ?1", (id.0, now))?;
    Ok((open_in(conn, s.channel)? == 0).then_some(s.channel))
}

/// A request is done: its session ends.
pub fn close_for_task(conn: &Connection, task: TaskId, now: i64) -> Result<Option<ChannelId>> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM session WHERE task = ?1 AND closed_at IS NULL")?;
        let rows = stmt.query_map([task], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut ended = None;
    for id in ids {
        ended = close(conn, SessionId(id), now)?.or(ended);
    }
    Ok(ended)
}

/// The user closed the conversation: every session in it ends.
pub fn close_channel(conn: &Connection, channel: ChannelId, now: i64) -> Result<Option<ChannelId>> {
    let open = query(conn, "channel = ?1 AND closed_at IS NULL", [channel])?;
    let had = !open.is_empty();
    for s in open {
        close(conn, s.id, now)?;
    }
    Ok(had.then_some(channel))
}

/// Close every session idle for `session.idle_timeout`; returns the channels whose
/// conversation thereby ended (each needs one wrap-up), in id order.
pub fn expire_idle(conn: &Connection, now: i64, params: &params::Session) -> Result<Vec<ChannelId>> {
    let stale = query(
        conn,
        "closed_at IS NULL AND last_at + ?1 <= ?2",
        (params.idle_timeout_ms(), now),
    )?;
    let mut ended = Vec::new();
    for s in stale {
        if let Some(c) = close(conn, s.id, now)? {
            ended.push(c);
        }
    }
    ended.sort();
    ended.dedup();
    Ok(ended)
}

/// Whether he is thawed anywhere right now.
pub fn is_thawed(conn: &Connection, person: &PersonId) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM session WHERE person = ?1 AND closed_at IS NULL LIMIT 1",
            [person],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Session as SessionParams;
    use crate::testing::*;
    use crate::types::Origin;

    #[test]
    fn frozen_people_reply_only_inside_a_triggered_session() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        assert!(!may_reply(&w, c, &p("a")).unwrap());
        let s = open(&w, c, &p("a"), Budget::User, Cause::Addressed, T0).unwrap();
        assert!(is_thawed(&w, &p("a")).unwrap());
        assert!(may_reply(&w, c, &p("a")).unwrap());
        assert_eq!(bond::mode(&w, &p("a")).unwrap(), Mode::Frozen, "the mode itself never changes");
        // Idle timeout ends it and asks for a wrap-up; he is frozen again.
        let idle = SessionParams::default();
        assert!(expire_idle(&w, T0 + idle.idle_timeout_ms() - 1, &idle).unwrap().is_empty());
        assert_eq!(expire_idle(&w, T0 + idle.idle_timeout_ms(), &idle).unwrap(), vec![c]);
        assert!(!is_thawed(&w, &p("a")).unwrap());
        assert!(!may_reply(&w, c, &p("a")).unwrap());
        assert!(get(&w, s).unwrap().closed_at.is_some());
    }

    #[test]
    fn activity_keeps_a_session_alive() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        open(&w, c, &p("a"), Budget::User, Cause::Addressed, T0).unwrap();
        let idle = SessionParams::default();
        touch_channel(&w, c, T0 + 20 * 60_000).unwrap();
        assert!(expire_idle(&w, T0 + idle.idle_timeout_ms(), &idle).unwrap().is_empty());
    }

    #[test]
    fn a_pulled_in_colleague_answers_once() {
        let w = world();
        let g = channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        open(&w, g, &p("a"), Budget::User, Cause::Addressed, T0).unwrap();
        open(&w, g, &p("b"), Budget::Replies(1), Cause::Join, T0).unwrap();
        assert!(may_reply(&w, g, &p("b")).unwrap());
        // His reply ends his session, but the channel's conversation goes on.
        assert_eq!(replied(&w, g, &p("b"), T0 + 1).unwrap(), None);
        assert!(!may_reply(&w, g, &p("b")).unwrap());
        assert_eq!(close_channel(&w, g, T0 + 2).unwrap(), Some(g));
        assert_eq!(close_channel(&w, g, T0 + 3).unwrap(), None);
    }

    #[test]
    fn disabled_cannot_be_triggered_and_enabled_need_no_session() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        bond::set_mode(&w, &p("a"), Mode::Enabled).unwrap();
        assert!(may_reply(&w, c, &p("a")).unwrap());
        bond::set_mode(&w, &p("a"), Mode::Disabled).unwrap();
        assert!(matches!(
            open(&w, c, &p("a"), Budget::User, Cause::Addressed, T0),
            Err(Error::Disabled(_))
        ));
        assert!(!may_reply(&w, c, &p("a")).unwrap());
    }

    #[test]
    fn a_request_session_ends_with_the_request() {
        let w = world();
        let a = crate::task::ask(&w, &p("a"), "q", Some(2), None, crate::Tier::Middle, &Default::default(), T0).unwrap();
        let s = current(&w, a.channel, &p("a")).unwrap().unwrap();
        assert_eq!((s.budget, s.cause), (Budget::Task(a.task), Cause::Ask));
        assert_eq!(close_for_task(&w, a.task, T0 + 1).unwrap(), Some(a.channel));
        assert!(!is_thawed(&w, &p("a")).unwrap());
    }
}
