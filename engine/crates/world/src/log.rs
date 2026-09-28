//! The append-only session log: storage only. What goes into the bytes is the agent's;
//! this module guarantees that sent bytes are never rewritten and that a segment's
//! opening block is built only from material its readers may see.

use rusqlite::{Connection, OptionalExtension, Row};

use crate::fact::{self, Fact};
use crate::types::{ChannelId, PersonId, SegmentId};
use crate::wrapup::{self, Summary};
use crate::{Error, Result, channel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub id: SegmentId,
    pub channel: ChannelId,
    pub seq: u32,
    pub prefix_a: Vec<u8>,
    pub prefix_b: Vec<u8>,
    pub prefix_c: Vec<u8>,
    /// JSON: the request shape that stays byte-identical for the segment's lifetime.
    pub shape: String,
    pub opened_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub seq: u32,
    pub role: Role,
    pub bytes: Vec<u8>,
    pub at: i64,
}

fn read_segment(row: &Row<'_>) -> rusqlite::Result<Segment> {
    Ok(Segment {
        id: row.get("id")?,
        channel: row.get("channel")?,
        seq: row.get("seq")?,
        prefix_a: row.get("prefix_a")?,
        prefix_b: row.get("prefix_b")?,
        prefix_c: row.get("prefix_c")?,
        shape: row.get("shape")?,
        opened_at: row.get("opened_at")?,
    })
}

/// The segment a channel currently appends to.
pub fn current(conn: &Connection, channel: ChannelId) -> Result<Option<Segment>> {
    let Some(id) = channel::get(conn, channel)?.log_segment else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id, channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at
             FROM log_segment WHERE id = ?1",
            [id],
            read_segment,
        )
        .optional()?)
}

/// Open the channel's next segment with its prefix, written once and never rewritten.
/// A rollover passes the previous segment's summary, which must be this channel's.
pub fn open_segment(
    conn: &Connection,
    channel: ChannelId,
    prefix: [&[u8]; 3],
    shape: &str,
    summary: Option<&Summary>,
    now: i64,
) -> Result<SegmentId> {
    if let Some(s) = summary {
        wrapup::check_summary_target(s, channel)?;
    }
    serde_json::from_str::<serde_json::Value>(shape)?;
    let seq: u32 = conn.query_row(
        "SELECT coalesce(max(seq) + 1, 0) FROM log_segment WHERE channel = ?1",
        [channel],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO log_segment(channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (channel, seq, prefix[0], prefix[1], prefix[2], shape, now),
    )?;
    let id = SegmentId(conn.last_insert_rowid());
    channel::set_log_segment(conn, channel, id)?;
    Ok(id)
}

/// Append one entry after the prefix; returns its sequence number.
pub fn append(conn: &Connection, segment: SegmentId, role: Role, bytes: &[u8], at: i64) -> Result<u32> {
    let seq: u32 = conn.query_row(
        "SELECT coalesce(max(seq) + 1, 0) FROM log_entry WHERE segment = ?1",
        [segment],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO log_entry(segment, seq, role, bytes, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        (segment, seq, role.as_str(), bytes, at),
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(_, _) => Error::Invalid(format!("segment {segment} does not exist")),
        e => e.into(),
    })?;
    Ok(seq)
}

pub fn entries(conn: &Connection, segment: SegmentId) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare("SELECT seq, role, bytes, at FROM log_entry WHERE segment = ?1 ORDER BY seq")?;
    let rows = stmt.query_map([segment], |r| {
        Ok(Entry {
            seq: r.get(0)?,
            role: Role::parse(&r.get::<_, String>(1)?),
            bytes: r.get(2)?,
            at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The memories a segment's opening block may carry for `speaker`: in a channel only
/// he and the user read, everything he may recall; in one others read, only what every
/// person there may recall ([`fact::shared_visible`]). The host has none.
pub fn opening_memories(conn: &Connection, channel: ChannelId, speaker: Option<&PersonId>) -> Result<Vec<Fact>> {
    fact::visible_to(conn, speaker, channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::Audience;
    use crate::testing::*;

    #[test]
    fn segments_number_per_channel_and_entries_append() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        assert_eq!(current(&w, c).unwrap(), None);
        let s0 = open_segment(&w, c, [b"A", b"B", b"C"], "{}", None, 1).unwrap();
        assert_eq!(append(&w, s0, Role::User, b"hi", 2).unwrap(), 0);
        assert_eq!(append(&w, s0, Role::Assistant, b"yo", 3).unwrap(), 1);
        assert_eq!(entries(&w, s0).unwrap()[1].role, Role::Assistant);
        let summary = Summary {
            channel: c,
            audience: Audience::Participants(c),
            text: "s".into(),
        };
        let s1 = open_segment(&w, c, [b"A", b"B", b"C2"], "{}", Some(&summary), 4).unwrap();
        let cur = current(&w, c).unwrap().unwrap();
        assert_eq!((cur.id, cur.seq), (s1, 1));
        assert!(open_segment(&w, c, [b"A", b"B", b"C"], "not json", None, 5).is_err());
        assert!(append(&w, SegmentId(99), Role::User, b"x", 6).is_err());
    }

    #[test]
    fn a_summary_cannot_open_another_channel() {
        let w = world();
        let a = channel::direct(&w, &p("a")).unwrap();
        let b = channel::direct(&w, &p("b")).unwrap();
        let summary = Summary {
            channel: a,
            audience: Audience::Participants(a),
            text: "private".into(),
        };
        assert!(open_segment(&w, b, [b"A", b"B", b"C"], "{}", Some(&summary), 1).is_err());
        assert_eq!(current(&w, b).unwrap(), None);
    }
}
