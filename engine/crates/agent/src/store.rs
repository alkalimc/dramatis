//! The few reads and writes on the world database that `world` has no function for yet.
//!
//! A streamed reply needs its message id before its text is known (every delta carries
//! it), so the row is written empty when the first delta arrives and completed when the
//! reply is. Until `world::message` offers that, the update is done here, on the same
//! columns `world::message::append` writes.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use world::{ChannelId, Citation, MessageId, Shape, TaskId};

/// What `message.attachment` holds for a message the agent wrote. The UI's view of a
/// message reads it through [`crate::view::message`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageMeta {
    /// The passages the reply cites, in order.
    pub cites: Vec<Citation>,
    /// How sure retrieval was about the material this reply had.
    pub confidence: Option<index::Level>,
    /// The request this message answers.
    pub task: Option<TaskId>,
    /// A retelling of something said elsewhere.
    pub relayed: bool,
    /// The request's note, relative to the office directory.
    pub note: Option<String>,
}

impl MessageMeta {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Complete a message: its final text, its tool actions and its meta.
pub fn complete_message(
    conn: &Connection,
    id: MessageId,
    text: &str,
    actions: Option<&Value>,
    meta: &MessageMeta,
) -> world::Result<()> {
    let meta = (!meta.is_empty())
        .then(|| serde_json::to_string(meta))
        .transpose()?;
    let actions = actions.map(serde_json::to_string).transpose()?;
    conn.execute(
        "UPDATE message SET text = ?2, tool_calls = ?3, attachment = ?4 WHERE id = ?1",
        rusqlite::params![id, text, actions, meta],
    )?;
    Ok(())
}

/// Remove a message that never got any text (its stream failed before the first delta).
pub fn discard_message(conn: &Connection, id: MessageId) -> world::Result<()> {
    conn.execute("DELETE FROM message WHERE id = ?1", [id])?;
    Ok(())
}

/// Input plus output tokens of the latest call on a channel since `since`: how full the
/// endpoint's context is.
pub fn last_context_tokens(
    conn: &Connection,
    channel: ChannelId,
    since: i64,
) -> world::Result<Option<u64>> {
    Ok(conn
        .query_row(
            "SELECT uncached + cached + output FROM meter WHERE channel = ?1 AND at >= ?2
             ORDER BY at DESC, id DESC LIMIT 1",
            (channel, since),
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|n| n.max(0) as u64))
}

/// Metered totals of one call shape over a span.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShapeTotals {
    pub calls: u64,
    pub uncached: u64,
    pub cached: u64,
    pub output: u64,
    pub points: f64,
}

pub fn totals_by_shape(
    conn: &Connection,
    since: i64,
    until: i64,
) -> world::Result<Vec<(Shape, ShapeTotals)>> {
    let mut stmt = conn.prepare(
        "SELECT shape, count(*), sum(uncached), sum(cached), sum(output), sum(points)
         FROM meter WHERE at > ?1 AND at <= ?2 GROUP BY shape ORDER BY shape",
    )?;
    let rows = stmt.query_map((since, until), |r| {
        let n = |i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
        Ok((
            r.get::<_, Shape>(0)?,
            ShapeTotals {
                calls: n(1)?,
                uncached: n(2)?,
                cached: n(3)?,
                output: n(4)?,
                points: r.get(5)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every channel with a log segment, in id order.
pub fn channels_with_segments(conn: &Connection) -> world::Result<Vec<ChannelId>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM channel WHERE log_segment IS NOT NULL AND deleted_at IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Read the meta a message carries, if any.
pub fn meta_of(attachment: Option<&Value>) -> MessageMeta {
    attachment
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}
