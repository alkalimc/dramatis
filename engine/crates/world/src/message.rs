//! What was said, as the user sees it. The model-facing bytes live in the session log.

use rusqlite::{Connection, OptionalExtension, Row};
use serde_json::Value;

use crate::types::{Actor, ChannelId, ChannelKind, MessageId, PersonId};
use crate::{Error, Result, bond, channel, not_found};

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub channel: ChannelId,
    pub author: Actor,
    pub text: String,
    pub tool_calls: Option<Value>,
    pub attachment: Option<Value>,
    pub at: i64,
}

const COLUMNS: &str = "id, channel, author, text, tool_calls, attachment, at";

fn read(row: &Row<'_>) -> rusqlite::Result<Message> {
    let json = |col: &str| -> rusqlite::Result<Option<Value>> {
        let raw: Option<String> = row.get(col)?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
    };
    Ok(Message {
        id: row.get("id")?,
        channel: row.get("channel")?,
        author: row.get("author")?,
        text: row.get("text")?,
        tool_calls: json("tool_calls")?,
        attachment: json("attachment")?,
        at: row.get("at")?,
    })
}

/// Store a message. The author must be the user or a participant, and the channel must
/// accept messages. Moves `last_seen`: a person's own message, or the user's message in
/// that person's direct channel.
pub fn append(
    conn: &Connection,
    channel: ChannelId,
    author: &Actor,
    text: &str,
    tool_calls: Option<&Value>,
    attachment: Option<&Value>,
    at: i64,
) -> Result<MessageId> {
    let ch = channel::get(conn, channel)?;
    if !ch.is_open() {
        return Err(Error::ChannelClosed(channel));
    }
    if !ch.has(author) {
        return Err(match author {
            Actor::Person(p) => Error::NotParticipant {
                person: p.clone(),
                channel,
            },
            _ => Error::Invalid(format!("{} is not in channel {channel}", author.as_str())),
        });
    }
    if let Actor::Person(p) = author {
        bond::ensure_available(conn, p)?;
    }
    conn.execute(
        "INSERT INTO message(channel, author, text, tool_calls, attachment, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            channel,
            author,
            text,
            tool_calls.map(Value::to_string),
            attachment.map(Value::to_string),
            at,
        ),
    )?;
    let id = MessageId(conn.last_insert_rowid());
    match author {
        Actor::Person(p) => bond::touch_user(conn, p, at)?,
        Actor::User if ch.kind == ChannelKind::Direct => {
            if let Some(p) = ch.person() {
                bond::touch_user(conn, p, at)?;
            }
        }
        _ => {}
    }
    Ok(id)
}

pub fn get(conn: &Connection, id: MessageId) -> Result<Message> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM message WHERE id = ?1"),
        [id],
        read,
    )
    .optional()?
    .ok_or_else(|| not_found("message", id))
}

/// Up to `limit` messages before `before` (all when `None`), oldest first.
pub fn history(
    conn: &Connection,
    channel: ChannelId,
    before: Option<MessageId>,
    limit: u32,
) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM message WHERE channel = ?1 AND (?2 IS NULL OR id < ?2)
         ORDER BY id DESC LIMIT ?3"
    ))?;
    let mut rows: Vec<Message> = stmt
        .query_map((channel, before, limit), read)?
        .collect::<rusqlite::Result<_>>()?;
    rows.reverse();
    Ok(rows)
}

/// Messages after `after` (all when `None`), oldest first.
pub fn since(
    conn: &Connection,
    channel: ChannelId,
    after: Option<MessageId>,
) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM message WHERE channel = ?1 AND (?2 IS NULL OR id > ?2) ORDER BY id"
    ))?;
    let rows = stmt.query_map((channel, after), read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn last(conn: &Connection, channel: ChannelId) -> Result<Option<Message>> {
    Ok(history(conn, channel, None, 1)?.pop())
}

/// When the user last wrote anywhere.
pub fn last_user_message_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT max(at) FROM message WHERE author = 'user'",
        [],
        |r| r.get(0),
    )?)
}

/// The user's recent words in channels `person` is in, newest first: the material the
/// "your words hit his units" seed searches. Only what he could see: channels he takes
/// part in, never another person's direct channel.
pub fn user_words_seen_by(
    conn: &Connection,
    person: &PersonId,
    since: i64,
    limit: u32,
) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM message m
         WHERE m.author = 'user' AND m.at >= ?2
           AND m.channel IN (SELECT channel FROM channel_participant WHERE participant = ?1)
         ORDER BY m.at DESC, m.id DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map((person, since, limit), read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use crate::types::{Mode, Origin};

    #[test]
    fn append_checks_membership_and_moves_last_seen() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let m = append(&w, c, &Actor::User, "hi", None, None, 10).unwrap();
        assert_eq!(bond::user_bond(&w, &p("a")).unwrap().last_seen, Some(10));
        assert!(matches!(
            append(&w, c, &Actor::Person(p("b")), "x", None, None, 11),
            Err(Error::NotParticipant { .. })
        ));
        let calls = serde_json::json!([{"tool": "search"}]);
        let r = append(&w, c, &Actor::Person(p("a")), "yo", Some(&calls), None, 12).unwrap();
        assert_eq!(get(&w, r).unwrap().tool_calls, Some(calls));
        assert_eq!(history(&w, c, None, 10).unwrap().len(), 2);
        assert_eq!(history(&w, c, Some(r), 10).unwrap()[0].id, m);
        assert_eq!(since(&w, c, Some(m)).unwrap().len(), 1);
        assert_eq!(last_user_message_at(&w).unwrap(), Some(10));
    }

    #[test]
    fn disabled_group_receives_nothing() {
        let w = world();
        let g = channel::create_group(&w, &[p("a")], None, Origin::User).unwrap();
        channel::set_mode(&w, g, Mode::Disabled).unwrap();
        assert!(matches!(
            append(&w, g, &Actor::User, "hi", None, None, 1),
            Err(Error::ChannelClosed(_))
        ));
    }

    #[test]
    fn user_words_are_limited_to_what_he_could_see() {
        let w = world();
        let mine = channel::direct(&w, &p("a")).unwrap();
        let other = channel::direct(&w, &p("b")).unwrap();
        append(&w, mine, &Actor::User, "to a", None, None, 5).unwrap();
        append(&w, other, &Actor::User, "to b", None, None, 6).unwrap();
        let seen = user_words_seen_by(&w, &p("a"), 0, 10).unwrap();
        assert_eq!(
            seen.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            ["to a"]
        );
    }
}
