//! Take back the user's own message and everything after it in its channel, as if it had
//! never been sent.
//!
//! What goes: the channel's later messages, the model log from that instant (entries
//! after it, and any segment opened after it), the memories written in the channel since
//! (a hurt gives its trust back), the seeds those memories or messages queued, the
//! sessions opened there, and the pictures only those messages carried. What stays: the
//! meter (the calls were made), trust earned by shared turns, and sessions that were open
//! before the message and closed after it.
//!
//! The log is append-only for the prefix cache: cutting its tail is allowed because the
//! bytes before the cut are untouched, so the cached prefix is still valid.
//!
//! The cut is a time: the message's `at`. Log entries, segments, facts and
//! sessions carry no link to the message that caused them, only a timestamp, and a person
//! cannot send a message in the same millisecond as the end of the last reply.

use rusqlite::{Connection, OptionalExtension};

use crate::fact::{self};
use crate::types::{Actor, ChannelId, FactKind, MessageId};
use crate::{Error, Result, channel, message};

/// What a rewind did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewound {
    /// The text of the message taken back, for the input box.
    pub text: String,
    /// Messages removed, the first being the one chosen.
    pub removed: u32,
    /// Memories deleted.
    pub facts: Vec<crate::FactId>,
}

/// Take back `message`, which must be the user's own in `channel`. The caller makes sure no
/// turn is running there. All or nothing, also when called inside a caller's transaction.
pub fn rewind(conn: &Connection, channel: ChannelId, message: MessageId) -> Result<Rewound> {
    crate::atomic(conn, |conn| rewind_in(conn, channel, message))
}

fn rewind_in(conn: &Connection, ch: ChannelId, id: MessageId) -> Result<Rewound> {
    let c = channel::get(conn, ch)?;
    if c.deleted_at.is_some() {
        return Err(Error::ChannelClosed(ch));
    }
    let m = message::get(conn, id)?;
    if m.channel != ch {
        return Err(Error::Invalid(format!(
            "message {id} is not in channel {ch}"
        )));
    }
    if m.author != Actor::User {
        return Err(Error::Invalid(
            "only your own messages can be sent again".into(),
        ));
    }
    let t = m.at;

    // The log: segments opened since go whole; the one before keeps its earlier entries.
    let last: Option<i64> = conn
        .query_row(
            "SELECT id FROM log_segment WHERE channel = ?1 AND opened_at < ?2
             ORDER BY seq DESC LIMIT 1",
            (ch, t),
            |r| r.get(0),
        )
        .optional()?;
    conn.execute(
        "UPDATE channel SET log_segment = ?2 WHERE id = ?1",
        (ch, last),
    )?;
    conn.execute(
        "DELETE FROM log_segment WHERE channel = ?1 AND opened_at >= ?2",
        (ch, t),
    )?;
    conn.execute(
        "DELETE FROM log_entry WHERE at >= ?2
           AND segment IN (SELECT id FROM log_segment WHERE channel = ?1)",
        (ch, t),
    )?;

    // Memories written in this channel since, other than the user's own edits.
    let audience = ch.0.to_string();
    let mut facts = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id FROM fact WHERE audience_kind = 'participants' AND audience_ref = ?1
           AND created_at >= ?2 AND (author <> 'user' OR kind = 'hurt') ORDER BY id",
    )?;
    for f in stmt.query_map((&audience, t), |r| r.get::<_, crate::FactId>(0))? {
        facts.push(f?);
    }
    drop(stmt);

    // Seeds: those the dropped memories or messages queued, and the openings delivered
    // here since (a commitment brought up again may be brought up again).
    let removed_messages: Vec<MessageId> = {
        let mut stmt = conn.prepare("SELECT id FROM message WHERE channel = ?1 AND id >= ?2")?;
        stmt.query_map((ch, id), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for f in &facts {
        conn.execute(
            "DELETE FROM seed WHERE kind IN ('commitment', 'memory') AND ref = ?1",
            [f.0.to_string()],
        )?;
    }
    for m in &removed_messages {
        conn.execute(
            "DELETE FROM seed WHERE (kind = 'words' AND ref = ?1) OR message = ?2",
            (m.0.to_string(), m),
        )?;
    }
    conn.execute(
        "UPDATE fact SET delivered = 0 WHERE kind = 'commitment' AND delivered = 1
           AND CAST(id AS TEXT) IN (SELECT ref FROM seed WHERE kind = 'commitment'
                                    AND channel = ?1 AND delivered_at >= ?2)",
        (ch, t),
    )?;
    conn.execute(
        "DELETE FROM seed WHERE channel = ?1 AND delivered_at >= ?2",
        (ch, t),
    )?;

    for f in &facts {
        let fact = fact::get(conn, *f)?;
        if fact.kind == FactKind::Hurt && !fact.retracted {
            crate::trust::undo_hurt(conn, &fact)?;
        }
        conn.execute("DELETE FROM fact WHERE id = ?1", [f])?;
    }

    // Sessions opened since.
    conn.execute(
        "DELETE FROM session WHERE channel = ?1 AND opened_at >= ?2",
        (ch, t),
    )?;
    conn.execute(
        "DELETE FROM message WHERE channel = ?1 AND id >= ?2",
        (ch, id),
    )?;
    conn.execute(
        "UPDATE channel SET read_up_to = (SELECT max(id) FROM message WHERE channel = ?1)
         WHERE id = ?1 AND read_up_to >= ?2",
        (ch, id),
    )?;

    // A picture only the removed messages named has no reason to stay.
    crate::picture::drop_orphans(conn)?;

    Ok(Rewound {
        text: m.text,
        removed: removed_messages.len() as u32,
        facts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{About, Audience, NewFact};
    use crate::log::{self, Role};
    use crate::testing::*;
    use crate::{bond, session};

    fn say(w: &Connection, c: ChannelId, who: &Actor, text: &str, ms: i64) -> MessageId {
        message::append(w, c, who, text, None, None, ms).unwrap()
    }

    #[test]
    fn takes_back_messages_log_memories_requests_and_sessions_after_the_message() {
        let w = world();
        let a = p("a");
        let c = channel::direct(&w, &a).unwrap();
        let person = Actor::Person(a.clone());
        let s0 = log::open_segment(&w, c, [b"A", b"B", b"C"], "{}", None, 10).unwrap();
        say(&w, c, &Actor::User, "one", 100);
        log::append(&w, s0, Role::User, b"u1", 101).unwrap();
        say(&w, c, &person, "r1", 102);
        log::append(&w, s0, Role::Assistant, b"a1", 102).unwrap();
        let keep = NewFact::new(
            person.clone(),
            Audience::Participants(c),
            FactKind::Fact,
            "early",
        );
        let keep = fact::write(&w, &keep, 103).unwrap().id();

        let m2 = say(&w, c, &Actor::User, "two", 200);
        log::append(&w, s0, Role::User, b"u2", 201).unwrap();
        say(&w, c, &person, "r2", 202);
        log::append(&w, s0, Role::Assistant, b"a2", 202).unwrap();
        let s1 = log::open_segment(&w, c, [b"A", b"B", b"C"], "{}", None, 203).unwrap();
        let late = NewFact::new(
            person.clone(),
            Audience::Participants(c),
            FactKind::Commitment,
            "later",
        )
        .due(900);
        let late = fact::write(&w, &late, 204).unwrap().id();
        let edited = NewFact::new(
            Actor::User,
            Audience::Participants(c),
            FactKind::Fact,
            "mine",
        );
        let edited = fact::write(&w, &edited, 205).unwrap().id();
        session::open(
            &w,
            c,
            &a,
            session::Budget::User,
            session::Cause::Addressed,
            206,
        )
        .unwrap();
        w.execute(
            "INSERT INTO seed(person, kind, ref, created_at) VALUES ('a', 'commitment', ?1, 208)",
            [late.0.to_string()],
        )
        .unwrap();

        let out = rewind(&w, c, m2).unwrap();
        assert_eq!(out.text, "two");
        assert_eq!(out.removed, 2);
        assert_eq!(out.facts, vec![late]);

        let left: Vec<_> = message::history(&w, c, None, 10)
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert_eq!(left, ["one", "r1"]);
        let cur = log::current(&w, c).unwrap().unwrap();
        assert_eq!(cur.id, s0);
        let bytes: Vec<_> = log::entries(&w, s0)
            .unwrap()
            .into_iter()
            .map(|e| e.bytes)
            .collect();
        assert_eq!(bytes, [b"u1".to_vec(), b"a1".to_vec()]);
        assert!(log::entries(&w, s1).unwrap().is_empty());
        assert!(fact::get(&w, keep).is_ok());
        assert!(fact::get(&w, edited).is_ok());
        assert!(fact::get(&w, late).is_err());
        assert!(session::open_sessions(&w).unwrap().is_empty());
        let seeds: i64 = w
            .query_row("SELECT count(*) FROM seed", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seeds, 0);
    }

    #[test]
    fn rewinding_drops_a_picture_only_the_removed_messages_named() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let shared = crate::picture::put(&w, "image/png", &[1]).unwrap();
        let alone = crate::picture::put(&w, "image/png", &[2]).unwrap();
        let named = |h: &str| serde_json::json!({ "picture": h });
        let keep = message::append(&w, c, &Actor::User, "a", None, Some(&named(&shared)), 100);
        keep.unwrap();
        let m = message::append(&w, c, &Actor::User, "b", None, Some(&named(&shared)), 200);
        let m2 = message::append(&w, c, &Actor::User, "c", None, Some(&named(&alone)), 300);
        rewind(&w, c, m.unwrap()).unwrap();
        assert!(m2.is_ok());
        assert!(crate::picture::get(&w, &shared).unwrap().is_some());
        assert!(crate::picture::get(&w, &alone).unwrap().is_none());
    }

    #[test]
    fn rewinding_the_first_message_leaves_no_log() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let m = say(&w, c, &Actor::User, "hello", 100);
        let s = log::open_segment(&w, c, [b"A", b"B", b"C"], "{}", None, 100).unwrap();
        log::append(&w, s, Role::User, b"u", 101).unwrap();
        rewind(&w, c, m).unwrap();
        assert_eq!(log::current(&w, c).unwrap(), None);
        assert!(message::history(&w, c, None, 5).unwrap().is_empty());
    }

    #[test]
    fn a_hurt_gives_its_trust_back() {
        let w = world();
        let a = p("a");
        let c = channel::direct(&w, &a).unwrap();
        bond::user_bond(&w, &a).unwrap();
        let before = bond::user_bond(&w, &a).unwrap().trust_exact;
        let m = say(&w, c, &Actor::User, "rude", 100);
        let hurt = NewFact::new(
            Actor::User,
            Audience::Participants(c),
            FactKind::Hurt,
            "rude",
        )
        .about(About::Person(a.clone()));
        let id = fact::write(&w, &hurt, 150).unwrap().id();
        crate::trust::take_hurt(&w, &fact::get(&w, id).unwrap(), &Default::default()).unwrap();
        assert!(bond::user_bond(&w, &a).unwrap().trust_exact < before);
        rewind(&w, c, m).unwrap();
        assert_eq!(bond::user_bond(&w, &a).unwrap().trust_exact, before);
        assert!(fact::get(&w, id).is_err());
    }

    #[test]
    fn refuses_other_authors_other_channels_and_deleted_groups() {
        let w = world();
        let a = p("a");
        let c = channel::direct(&w, &a).unwrap();
        let reply = say(&w, c, &Actor::Person(a.clone()), "hi", 100);
        assert!(matches!(rewind(&w, c, reply), Err(Error::Invalid(_))));
        let mine = say(&w, c, &Actor::User, "x", 101);
        let other = channel::direct(&w, &p("b")).unwrap();
        assert!(matches!(rewind(&w, other, mine), Err(Error::Invalid(_))));
        assert!(rewind(&w, c, MessageId(999)).is_err());
        assert!(message::get(&w, mine).is_ok());
    }

    fn count(w: &Connection, table: &str) -> i64 {
        w.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// Archived channel, announced items for removed messages: the
    /// rewind removes whole rows only, and the announced record never blocks news.
    #[test]
    fn rewinds_an_archived_channel_with_announced_items() {
        use crate::presence::{self, Birthdays};
        let w = world();
        let a = p("a");
        let c = channel::direct(&w, &a).unwrap();
        say(&w, c, &Actor::User, "one", 100);
        let m2 = say(&w, c, &Actor::User, "two", 200);
        say(&w, c, &Actor::Person(a.clone()), "reply", 210);
        channel::set_archived(&w, c, true).unwrap();

        let none: Vec<(crate::PersonId, crate::types::MonthDay)> = Vec::new();
        let now = crate::clock::Now::utc(crate::testing::T0);
        let before = presence::digest(&w, now, &none as &dyn Birthdays).unwrap();
        assert!(!before.is_empty());
        presence::mark_announced(&w, &before, &before, now.local_date()).unwrap();

        let out = rewind(&w, c, m2).unwrap();
        assert_eq!(out.removed, 2);
        assert!(
            channel::get(&w, c).unwrap().archived,
            "rewind leaves the list as it was"
        );
        assert_eq!(message::history(&w, c, None, 10).unwrap().len(), 1);

        // The next record drops keys of items that are gone; until then they are inert.
        let after = presence::digest(&w, now, &none as &dyn Birthdays).unwrap();
        presence::mark_announced(&w, &after, &presence::Digest::default(), now.local_date())
            .unwrap();
        let kept: Vec<String> = crate::settings::get(&w, "presence.announced")
            .unwrap()
            .unwrap_or_default();
        assert!(kept.is_empty(), "{kept:?}");
    }

    #[test]
    fn a_failure_midway_removes_nothing() {
        let w = world();
        let a = p("a");
        let c = channel::direct(&w, &a).unwrap();
        let m = say(&w, c, &Actor::User, "one", 100);
        say(&w, c, &Actor::Person(a), "r", 101);
        // The last step deletes messages; make it fail after the earlier steps ran.
        w.execute_batch(
            "CREATE TRIGGER stop BEFORE DELETE ON message BEGIN SELECT RAISE(ABORT, 'stop'); END;",
        )
        .unwrap();
        assert!(rewind(&w, c, m).is_err());
        assert!(w.is_autocommit());
        assert_eq!(count(&w, "message"), 2);
    }

    #[test]
    fn rewind_inside_a_caller_transaction_is_a_savepoint() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let m = say(&w, c, &Actor::User, "one", 100);
        let tx = w.unchecked_transaction().unwrap();
        rewind(&w, c, m).unwrap();
        assert_eq!(count(&w, "message"), 0);
        drop(tx);
        assert_eq!(count(&w, "message"), 1, "the caller decides");
    }

    #[test]
    fn a_rewind_takes_the_write_lock_up_front_so_another_connection_cannot_split_it() {
        let dir = std::env::temp_dir().join(format!("world-rewind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("w.sqlite");
        let w = crate::World::open(&path).unwrap();
        let other = crate::World::open(&path).unwrap();
        let c = channel::direct(&w, &p("a")).unwrap();
        let m = say(&w, c, &Actor::User, "one", 100);
        w.busy_timeout(std::time::Duration::from_millis(50))
            .unwrap();
        // Another connection holds the write lock: the rewind is refused outright instead
        // of reading a snapshot that the other's commit would then invalidate.
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(rewind(&w, c, m).is_err());
        assert!(w.is_autocommit());
        other.execute_batch("COMMIT").unwrap();
        // A write the other connection committed meanwhile is seen, not a stale snapshot.
        let later = say(&other, c, &Actor::Person(p("a")), "r", 101);
        let out = rewind(&w, c, m).unwrap();
        assert_eq!(out.removed, 2, "{later:?}");
        assert_eq!(count(&w, "message"), 0);
        drop((w, other));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
