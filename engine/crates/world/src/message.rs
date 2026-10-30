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
    /// The earlier message this one answers.
    pub reply_to: Option<MessageId>,
}

const COLUMNS: &str = "id, channel, author, text, tool_calls, attachment, at, reply_to";

fn read(row: &Row<'_>) -> rusqlite::Result<Message> {
    let json = |col: &str| -> rusqlite::Result<Option<Value>> {
        let raw: Option<String> = row.get(col)?;
        raw.map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
    };
    Ok(Message {
        id: row.get("id")?,
        channel: row.get("channel")?,
        author: row.get("author")?,
        text: row.get("text")?,
        tool_calls: json("tool_calls")?,
        attachment: json("attachment")?,
        at: row.get("at")?,
        reply_to: row.get("reply_to")?,
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
    crate::atomic(conn, |conn| {
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
            Actor::Person(p) => {
                bond::touch_user(conn, p, at)?;
                // Someone speaking brings an archived channel back, as in a chat app.
                if ch.archived {
                    conn.execute("UPDATE channel SET archived = 0 WHERE id = ?1", [channel])?;
                }
            }
            Actor::User if ch.kind == ChannelKind::Direct => {
                if let Some(p) = ch.person() {
                    bond::touch_user(conn, p, at)?;
                }
            }
            _ => {}
        }
        Ok(id)
    })
}

/// Store a copy of a message in the favorites channel: the author stays the original
/// author, `at` is when it is saved. Nothing else moves: not `last_seen`, not the
/// archive flag. Anyone may be the author, the user's own notes included.
pub fn append_saved(
    conn: &Connection,
    channel: ChannelId,
    author: &Actor,
    text: &str,
    attachment: Option<&Value>,
    at: i64,
) -> Result<MessageId> {
    let ch = channel::get(conn, channel)?;
    if ch.kind != ChannelKind::Favorites {
        return Err(Error::Invalid(format!(
            "channel {channel} is not the favorites"
        )));
    }
    conn.execute(
        "INSERT INTO message(channel, author, text, attachment, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        (channel, author, text, attachment.map(Value::to_string), at),
    )?;
    Ok(MessageId(conn.last_insert_rowid()))
}

/// Mark a stored message as the answer to `reply_to`, which must be in the same channel.
pub fn set_reply_to(conn: &Connection, id: MessageId, reply_to: MessageId) -> Result<()> {
    let (own, theirs): (ChannelId, ChannelId) =
        (get(conn, id)?.channel, get(conn, reply_to)?.channel);
    if own != theirs {
        return Err(Error::Invalid(
            "the quoted message is in another channel".into(),
        ));
    }
    conn.execute(
        "UPDATE message SET reply_to = ?2 WHERE id = ?1",
        (id, reply_to),
    )?;
    Ok(())
}

/// Remove one message from the favorites channel; anywhere else it is refused.
pub fn delete_saved(conn: &Connection, id: MessageId) -> Result<()> {
    let m = get(conn, id)?;
    if channel::get(conn, m.channel)?.kind != ChannelKind::Favorites {
        return Err(Error::Invalid("only a saved message can be removed".into()));
    }
    conn.execute("DELETE FROM message WHERE id = ?1", [id])?;
    Ok(())
}

/// Fill in a message written empty: a streamed reply needs its id before its text is
/// known, so the row exists from the first delta and is completed when the reply is.
pub fn complete(
    conn: &Connection,
    id: MessageId,
    text: &str,
    tool_calls: Option<&Value>,
    attachment: Option<&Value>,
) -> Result<()> {
    conn.execute(
        "UPDATE message SET text = ?2, tool_calls = ?3, attachment = ?4 WHERE id = ?1",
        (
            id,
            text,
            tool_calls.map(Value::to_string),
            attachment.map(Value::to_string),
        ),
    )?;
    Ok(())
}

/// Replace the tool calls a stored message carries.
pub fn set_tool_calls(conn: &Connection, id: MessageId, tool_calls: &Value) -> Result<()> {
    conn.execute(
        "UPDATE message SET tool_calls = ?2 WHERE id = ?1",
        (id, tool_calls.to_string()),
    )?;
    Ok(())
}

/// Replace what a stored message's attachment holds.
pub fn set_attachment(conn: &Connection, id: MessageId, attachment: &Value) -> Result<()> {
    conn.execute(
        "UPDATE message SET attachment = ?2 WHERE id = ?1",
        (id, attachment.to_string()),
    )?;
    Ok(())
}

/// Remove a message that never got any text (its stream failed before the first delta).
pub fn discard(conn: &Connection, id: MessageId) -> Result<()> {
    conn.execute("DELETE FROM message WHERE id = ?1", [id])?;
    Ok(())
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

/// Messages whose text holds every one of `terms` (case-folded), in live channels only,
/// newest first, at most `limit`. A plain scan: a save holds a few thousand messages.
pub fn search(conn: &Connection, terms: &[String], limit: u32) -> Result<Vec<Message>> {
    let terms: Vec<String> = terms
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM message
         WHERE channel IN (SELECT id FROM channel WHERE deleted_at IS NULL)
         ORDER BY id DESC"
    ))?;
    let mut out = Vec::new();
    for row in stmt.query_map([], read)? {
        let m = row?;
        let text = m.text.to_lowercase();
        if terms.iter().all(|t| text.contains(t.as_str())) {
            out.push(m);
            if out.len() >= limit as usize {
                break;
            }
        }
    }
    Ok(out)
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

/// How many messages from someone other than the user lie after `after` (all when
/// `None`): what the channel's unread badge counts.
pub fn unread_count(
    conn: &Connection,
    channel: ChannelId,
    after: Option<MessageId>,
) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM message
         WHERE channel = ?1 AND author <> 'user' AND (?2 IS NULL OR id > ?2)",
        (channel, after),
        |r| r.get(0),
    )?)
}

/// Messages of any channel with an id above `after`, oldest first, at most `limit`.
pub fn all_since(conn: &Connection, after: i64, limit: u32) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM message WHERE id > ?1 ORDER BY id LIMIT ?2"
    ))?;
    let rows = stmt.query_map((after, limit), read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn last(conn: &Connection, channel: ChannelId) -> Result<Option<Message>> {
    Ok(history(conn, channel, None, 1)?.pop())
}

/// When the user last wrote to anyone: a note kept in the favorites is to nobody.
pub fn last_user_message_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT max(at) FROM message
         WHERE author = 'user' AND channel NOT IN (SELECT id FROM channel WHERE kind = 'favorites')",
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
    fn search_finds_every_word_in_live_channels_newest_first() {
        let w = world();
        let a = channel::direct(&w, &p("a")).unwrap();
        let g = channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        append(&w, a, &Actor::User, "Tea at Noon", None, None, 1).unwrap();
        let later = append(
            &w,
            g,
            &Actor::Person(p("b")),
            "noon tea, then",
            None,
            None,
            2,
        )
        .unwrap();
        append(&w, a, &Actor::User, "only tea", None, None, 3).unwrap();
        let words = |q: &[&str]| q.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let found: Vec<_> = search(&w, &words(&["tea", "NOON"]), 10)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], later);
        assert_eq!(search(&w, &words(&["tea"]), 1).unwrap().len(), 1);
        assert!(search(&w, &words(&["  "]), 10).unwrap().is_empty());
        // A deleted group's messages are gone from the search.
        channel::delete_group(&w, g, 4).unwrap();
        assert_eq!(search(&w, &words(&["tea", "noon"]), 10).unwrap().len(), 1);
    }

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

    #[test]
    fn a_person_speaking_unarchives_in_the_same_unit_as_the_insert() {
        let w = world();
        let a = p("a");
        let c = channel::direct(&w, &a).unwrap();
        channel::set_archived(&w, c, true).unwrap();
        append(&w, c, &Actor::User, "hi", None, None, 1).unwrap();
        assert!(
            channel::get(&w, c).unwrap().archived,
            "the user's words do not"
        );

        // If clearing the flag fails, the message is not stored either.
        w.execute_batch(
            "CREATE TRIGGER keep BEFORE UPDATE OF archived ON channel
             BEGIN SELECT RAISE(ABORT, 'keep'); END;",
        )
        .unwrap();
        assert!(append(&w, c, &Actor::Person(a.clone()), "yo", None, None, 2).is_err());
        assert_eq!(history(&w, c, None, 10).unwrap().len(), 1);
        assert_eq!(bond::user_bond(&w, &a).unwrap().last_seen, Some(1));
        w.execute_batch("DROP TRIGGER keep").unwrap();

        append(&w, c, &Actor::Person(a), "yo", None, None, 3).unwrap();
        assert!(!channel::get(&w, c).unwrap().archived);
    }

    #[test]
    fn a_reply_points_at_a_message_of_its_own_channel_and_loses_it_with_the_message() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let other = channel::direct(&w, &p("b")).unwrap();
        let first = append(&w, c, &Actor::User, "one", None, None, 1).unwrap();
        let second = append(&w, c, &Actor::User, "two", None, None, 2).unwrap();
        let elsewhere = append(&w, other, &Actor::User, "x", None, None, 3).unwrap();
        assert_eq!(get(&w, second).unwrap().reply_to, None);
        set_reply_to(&w, second, first).unwrap();
        assert_eq!(get(&w, second).unwrap().reply_to, Some(first));
        assert!(set_reply_to(&w, second, elsewhere).is_err());
        // The quoted message going away leaves the reply standing, with nothing quoted.
        discard(&w, first).unwrap();
        assert_eq!(get(&w, second).unwrap().reply_to, None);
    }

    #[test]
    fn the_favorites_keep_any_author_and_time_and_move_nothing_else() {
        let w = world();
        let fav = channel::favorites(&w).unwrap();
        assert_eq!(channel::favorites(&w).unwrap(), fav);
        let c = channel::direct(&w, &p("a")).unwrap();
        let said = append(&w, c, &Actor::Person(p("a")), "hello", None, None, 5).unwrap();
        let before = bond::user_bond(&w, &p("a")).unwrap().last_seen;
        let kept = append_saved(&w, fav, &Actor::Person(p("a")), "hello", None, 5).unwrap();
        assert_ne!(kept, said);
        let m = get(&w, kept).unwrap();
        assert_eq!((m.author, m.at), (Actor::Person(p("a")), 5));
        assert_eq!(bond::user_bond(&w, &p("a")).unwrap().last_seen, before);
        // Only the favorites take a saved copy, and only they let one be removed.
        assert!(append_saved(&w, c, &Actor::User, "x", None, 1).is_err());
        assert!(delete_saved(&w, said).is_err());
        // A note is not the user writing to someone.
        append(&w, fav, &Actor::User, "note", None, None, 99).unwrap();
        assert_eq!(last_user_message_at(&w).unwrap(), None);
        delete_saved(&w, kept).unwrap();
        assert!(get(&w, kept).is_err());
        assert!(channel::set_archived(&w, fav, true).is_err());
    }
}
