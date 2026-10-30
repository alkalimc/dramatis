//! Conversations. The user is implicitly in every channel; everyone else is listed as a
//! participant. A direct channel belongs to one person (or to the host) and keeps that
//! owner when a colleague is pulled in.

use rusqlite::{Connection, OptionalExtension, Row};

use crate::types::{Actor, ChannelId, ChannelKind, MessageId, Mode, Origin, PersonId, SegmentId};
use crate::{Error, Result, bond, not_found};

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub id: ChannelId,
    pub kind: ChannelKind,
    pub origin: Origin,
    pub topic: Option<String>,
    pub mode: Mode,
    pub log_segment: Option<SegmentId>,
    pub read_up_to: Option<MessageId>,
    /// Whose direct channel this is; `None` for a group.
    pub owner: Option<Actor>,
    pub deleted_at: Option<i64>,
    /// A title the user gave it.
    pub name: Option<String>,
    /// File name of the user's picture for it (under the home's uploads directory).
    pub avatar: Option<String>,
    pub archived: bool,
    /// Persons and possibly the host, by id.
    pub participants: Vec<Actor>,
}

impl Channel {
    /// The person whose direct channel this is.
    pub fn person(&self) -> Option<&PersonId> {
        self.owner.as_ref().and_then(Actor::person)
    }

    pub fn persons(&self) -> Vec<PersonId> {
        self.participants
            .iter()
            .filter_map(|a| a.person().cloned())
            .collect()
    }

    pub fn has(&self, actor: &Actor) -> bool {
        match actor {
            Actor::User => true,
            _ => self.participants.contains(actor),
        }
    }

    pub fn is_host_channel(&self) -> bool {
        self.owner == Some(Actor::Host)
    }

    /// The user's favorites: a place for saved messages and notes, with no one in it.
    pub fn is_favorites(&self) -> bool {
        self.kind == ChannelKind::Favorites
    }

    /// Accepts new messages: not deleted, and not a disabled group.
    pub fn is_open(&self) -> bool {
        self.deleted_at.is_none()
            && !(self.kind == ChannelKind::Group && self.mode == Mode::Disabled)
    }
}

const COLUMNS: &str = "id, kind, origin, topic, mode, log_segment, read_up_to, direct_with, deleted_at, name, avatar, archived";

fn read(row: &Row<'_>) -> rusqlite::Result<Channel> {
    let owner: Option<String> = row.get("direct_with")?;
    Ok(Channel {
        id: row.get("id")?,
        kind: row.get("kind")?,
        origin: row.get("origin")?,
        topic: row.get("topic")?,
        mode: row.get("mode")?,
        log_segment: row.get("log_segment")?,
        read_up_to: row.get("read_up_to")?,
        owner: owner.as_deref().map(Actor::parse),
        deleted_at: row.get("deleted_at")?,
        name: row.get("name")?,
        avatar: row.get("avatar")?,
        archived: row.get::<_, i64>("archived")? != 0,
        participants: Vec::new(),
    })
}

pub fn participants(conn: &Connection, channel: ChannelId) -> Result<Vec<Actor>> {
    let mut stmt = conn.prepare(
        "SELECT participant FROM channel_participant WHERE channel = ?1 ORDER BY participant",
    )?;
    let rows = stmt.query_map([channel], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(conn: &Connection, id: ChannelId) -> Result<Channel> {
    let mut ch = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM channel WHERE id = ?1"),
            [id],
            read,
        )
        .optional()?
        .ok_or_else(|| not_found("channel", id))?;
    ch.participants = participants(conn, id)?;
    if ch.owner.is_none() && ch.kind == ChannelKind::Direct {
        // Schema-1 rows: the sole participant is the owner.
        ch.owner = ch.participants.first().cloned();
    }
    Ok(ch)
}

/// Every channel not deleted, newest first by last message.
pub fn list(conn: &Connection) -> Result<Vec<Channel>> {
    let mut stmt = conn.prepare(
        "SELECT c.id FROM channel c WHERE c.deleted_at IS NULL
         ORDER BY (SELECT max(at) FROM message m WHERE m.channel = c.id) DESC NULLS LAST, c.id",
    )?;
    let ids: Vec<ChannelId> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    ids.into_iter().map(|id| get(conn, id)).collect()
}

fn find_direct(conn: &Connection, owner: &str) -> Result<Option<ChannelId>> {
    let found = conn
        .query_row(
            "SELECT id FROM channel WHERE direct_with = ?1",
            [owner],
            |r| r.get(0),
        )
        .optional()?;
    if found.is_some() {
        return Ok(found);
    }
    // Schema-1 rows carry no owner: a direct channel whose only participant is them.
    Ok(conn
        .query_row(
            "SELECT c.id FROM channel c
             WHERE c.kind = 'direct' AND c.direct_with IS NULL
               AND (SELECT count(*) FROM channel_participant p WHERE p.channel = c.id) = 1
               AND EXISTS (SELECT 1 FROM channel_participant p
                           WHERE p.channel = c.id AND p.participant = ?1)
             ORDER BY c.id LIMIT 1",
            [owner],
            |r| r.get(0),
        )
        .optional()?)
}

fn create_direct(conn: &Connection, owner: &str) -> Result<ChannelId> {
    crate::atomic(conn, |conn| {
        conn.execute(
            "INSERT INTO channel(kind, origin, direct_with) VALUES ('direct', 'system', ?1)",
            [owner],
        )?;
        let id = ChannelId(conn.last_insert_rowid());
        conn.execute(
            "INSERT INTO channel_participant(channel, participant) VALUES (?1, ?2)",
            (id, owner),
        )?;
        Ok(id)
    })
}

/// A person's direct channel, if it exists yet.
pub fn find_direct_with(conn: &Connection, person: &PersonId) -> Result<Option<ChannelId>> {
    find_direct(conn, person.as_str())
}

/// The direct channel with a person, created on first use. A disabled person gets none:
/// nothing can be sent to him.
pub fn direct(conn: &Connection, person: &PersonId) -> Result<ChannelId> {
    if let Some(id) = find_direct(conn, person.as_str())? {
        return Ok(id);
    }
    bond::ensure_available(conn, person)?;
    create_direct(conn, person.as_str())
}

/// The host's own channel, created on first use.
pub fn host(conn: &Connection) -> Result<ChannelId> {
    match find_direct(conn, "host")? {
        Some(id) => Ok(id),
        None => create_direct(conn, "host"),
    }
}

/// The user's favorites, created on first use.
pub fn favorites(conn: &Connection) -> Result<ChannelId> {
    let found = conn
        .query_row("SELECT id FROM channel WHERE kind = 'favorites'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO channel(kind, origin) VALUES ('favorites', 'user')",
        [],
    )?;
    Ok(ChannelId(conn.last_insert_rowid()))
}

/// A new group. Every group starts frozen, whoever made it; disabled people cannot be
/// members.
pub fn create_group(
    conn: &Connection,
    members: &[PersonId],
    topic: Option<&str>,
    origin: Origin,
) -> Result<ChannelId> {
    crate::atomic(conn, |conn| {
        if members.is_empty() {
            return Err(Error::Invalid("a group needs at least one member".into()));
        }
        for m in members {
            bond::ensure_available(conn, m)?;
        }
        conn.execute(
            "INSERT INTO channel(kind, origin, topic) VALUES ('group', ?1, ?2)",
            (origin, topic),
        )?;
        let id = ChannelId(conn.last_insert_rowid());
        for m in members {
            conn.execute(
                "INSERT INTO channel_participant(channel, participant) VALUES (?1, ?2)
             ON CONFLICT DO NOTHING",
                (id, m),
            )?;
        }
        Ok(id)
    })
}

/// A group made from a proposal: the purpose is its topic and also its name, so the title
/// stays what the group was made for however its members change. Without a purpose the
/// name stays empty and the title is derived from the members.
pub fn create_proposed_group(
    conn: &Connection,
    members: &[PersonId],
    purpose: Option<&str>,
    origin: Origin,
) -> Result<ChannelId> {
    crate::atomic(conn, |conn| {
        let id = create_group(conn, members, purpose, origin)?;
        set_name(conn, id, purpose)?;
        Ok(id)
    })
}

/// Bring someone into a channel (a colleague pulled in, or the host appearing in a
/// group). Idempotent. Being a participant is what makes the channel's memories visible.
pub fn add_participant(conn: &Connection, channel: ChannelId, who: &Actor) -> Result<()> {
    let ch = get(conn, channel)?;
    if !ch.is_open() {
        return Err(Error::ChannelClosed(channel));
    }
    match who {
        Actor::User => return Ok(()),
        Actor::Host => {}
        Actor::Person(p) => bond::ensure_available(conn, p)?,
    }
    conn.execute(
        "INSERT INTO channel_participant(channel, participant) VALUES (?1, ?2)
         ON CONFLICT DO NOTHING",
        (channel, who),
    )?;
    Ok(())
}

/// Bring people into a live group, as the user's own change of its roster. Each must be
/// available. Returns who was not in it already, so a repeat adds nobody.
pub fn add_members(
    conn: &Connection,
    channel: ChannelId,
    persons: &[PersonId],
) -> Result<Vec<PersonId>> {
    crate::atomic(conn, |conn| {
        let ch = live_group(conn, channel)?;
        let mut added = Vec::new();
        for p in persons {
            bond::ensure_available(conn, p)?;
            if !ch.participants.contains(&Actor::Person(p.clone())) && !added.contains(p) {
                added.push(p.clone());
            }
        }
        for p in &added {
            conn.execute(
                "INSERT INTO channel_participant(channel, participant) VALUES (?1, ?2)
                 ON CONFLICT DO NOTHING",
                (channel, p),
            )?;
        }
        Ok(added)
    })
}

/// Bring the host into a live group, as the user's own change of its roster (the host
/// answers there when it is addressed). Returns whether it was not in it already.
pub fn add_host(conn: &Connection, channel: ChannelId) -> Result<bool> {
    crate::atomic(conn, |conn| {
        let ch = live_group(conn, channel)?;
        if ch.participants.contains(&Actor::Host) {
            return Ok(false);
        }
        add_participant(conn, channel, &Actor::Host)?;
        Ok(true)
    })
}

/// Take the host out of a live group; what it said stays in its history. Returns
/// whether it was in it.
pub fn remove_host(conn: &Connection, channel: ChannelId) -> Result<bool> {
    crate::atomic(conn, |conn| {
        let ch = live_group(conn, channel)?;
        if !ch.participants.contains(&Actor::Host) {
            return Ok(false);
        }
        conn.execute(
            "DELETE FROM channel_participant WHERE channel = ?1 AND participant = ?2",
            (channel, Actor::Host),
        )?;
        Ok(true)
    })
}

/// Take a person out of a live group. What they said stays in its history. A group keeps
/// at least one person. Returns whether the person was in it.
pub fn remove_member(conn: &Connection, channel: ChannelId, person: &PersonId) -> Result<bool> {
    crate::atomic(conn, |conn| {
        let ch = live_group(conn, channel)?;
        if !ch.participants.contains(&Actor::Person(person.clone())) {
            return Ok(false);
        }
        if ch.persons().len() <= 1 {
            return Err(Error::Invalid("a group keeps at least one person".into()));
        }
        conn.execute(
            "DELETE FROM channel_participant WHERE channel = ?1 AND participant = ?2",
            (channel, person),
        )?;
        Ok(true)
    })
}

fn live_group(conn: &Connection, channel: ChannelId) -> Result<Channel> {
    let ch = get(conn, channel)?;
    if ch.kind != ChannelKind::Group || ch.deleted_at.is_some() {
        return Err(Error::Invalid(format!(
            "channel {channel} is not a live group"
        )));
    }
    Ok(ch)
}

/// The live, unarchived group whose people are exactly `persons` (the host aside), if
/// any; the one with the newest message when several. With a `purpose`, only a group
/// whose name or topic is that purpose (same letters and digits, case and punctuation
/// aside) counts: the same people for another purpose are another group.
pub fn find_group_with(
    conn: &Connection,
    persons: &[PersonId],
    purpose: Option<&str>,
) -> Result<Option<Channel>> {
    let key = purpose.map(crate::fact::normalize);
    let same_purpose = |ch: &Channel| match &key {
        None => true,
        Some(k) => {
            !k.is_empty()
                && [ch.name.as_deref(), ch.topic.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|t| crate::fact::normalize(t) == *k)
        }
    };
    let mut want = persons.to_vec();
    want.sort();
    want.dedup();
    for ch in list(conn)? {
        if ch.kind != ChannelKind::Group || ch.archived {
            continue;
        }
        let mut have = ch.persons();
        have.sort();
        if have == want && same_purpose(&ch) {
            return Ok(Some(ch));
        }
    }
    Ok(None)
}

/// Live, unarchived groups a person is in, newest first.
pub fn groups_of(conn: &Connection, person: &PersonId) -> Result<Vec<Channel>> {
    let mut stmt = conn.prepare(
        "SELECT c.id FROM channel c JOIN channel_participant p ON p.channel = c.id
         WHERE p.participant = ?1 AND c.kind = 'group' AND c.deleted_at IS NULL
           AND c.archived = 0
         ORDER BY c.id DESC",
    )?;
    let ids: Vec<ChannelId> = stmt
        .query_map([person], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    ids.into_iter().map(|id| get(conn, id)).collect()
}

/// A group's mode. Direct channels have none of their own: their person's mode governs.
pub fn set_mode(conn: &Connection, channel: ChannelId, mode: Mode) -> Result<()> {
    let ch = get(conn, channel)?;
    if ch.kind != ChannelKind::Group || ch.deleted_at.is_some() {
        return Err(Error::Invalid(format!(
            "channel {channel} is not a live group"
        )));
    }
    conn.execute(
        "UPDATE channel SET mode = ?2 WHERE id = ?1",
        (channel, mode),
    )?;
    Ok(())
}

/// The user's title for a channel; `None` or blank clears it.
pub fn set_name(conn: &Connection, channel: ChannelId, name: Option<&str>) -> Result<()> {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    get(conn, channel)?;
    conn.execute(
        "UPDATE channel SET name = ?2 WHERE id = ?1",
        (channel, name),
    )?;
    Ok(())
}

/// A group's description; `None` or blank clears it. Returns whether it changed.
pub fn set_topic(conn: &Connection, channel: ChannelId, topic: Option<&str>) -> Result<bool> {
    let topic = topic.map(str::trim).filter(|t| !t.is_empty());
    let ch = get(conn, channel)?;
    if ch.kind != ChannelKind::Group {
        return Err(Error::Invalid(format!("channel {channel} is not a group")));
    }
    if ch.topic.as_deref() == topic {
        return Ok(false);
    }
    conn.execute(
        "UPDATE channel SET topic = ?2 WHERE id = ?1",
        (channel, topic),
    )?;
    Ok(true)
}

/// The user's picture for a channel (a file name), or back to none.
pub fn set_avatar(conn: &Connection, channel: ChannelId, file: Option<&str>) -> Result<()> {
    get(conn, channel)?;
    conn.execute(
        "UPDATE channel SET avatar = ?2 WHERE id = ?1",
        (channel, file),
    )?;
    Ok(())
}

/// Move a channel out of, or back into, the main list. The host's channel stays.
pub fn set_archived(conn: &Connection, channel: ChannelId, archived: bool) -> Result<()> {
    let ch = get(conn, channel)?;
    if archived && (ch.is_host_channel() || ch.is_favorites()) {
        return Err(Error::Invalid(
            "the host channel and the favorites cannot be archived".into(),
        ));
    }
    conn.execute(
        "UPDATE channel SET archived = ?2 WHERE id = ?1",
        (channel, archived),
    )?;
    Ok(())
}

/// Delete a group: its messages, sessions and logs go; the row and its participants stay,
/// so memories scoped to it remain with the people who were there.
pub fn delete_group(conn: &Connection, channel: ChannelId, now: i64) -> Result<()> {
    crate::atomic(conn, |conn| {
        let ch = get(conn, channel)?;
        if ch.kind != ChannelKind::Group {
            return Err(Error::Invalid(format!("channel {channel} is not a group")));
        }
        conn.execute(
            "UPDATE channel SET deleted_at = ?2, log_segment = NULL WHERE id = ?1",
            (channel, now),
        )?;
        conn.execute("DELETE FROM log_segment WHERE channel = ?1", [channel])?;
        conn.execute("DELETE FROM message WHERE channel = ?1", [channel])?;
        conn.execute(
            "UPDATE session SET closed_at = ?2 WHERE channel = ?1 AND closed_at IS NULL",
            (channel, now),
        )?;
        Ok(())
    })
}

/// Everything up to `up_to` has been seen. Never moves backwards.
pub fn mark_read(conn: &Connection, channel: ChannelId, up_to: MessageId) -> Result<()> {
    get(conn, channel)?;
    conn.execute(
        "UPDATE channel SET read_up_to = max(coalesce(read_up_to, ?2), ?2) WHERE id = ?1",
        (channel, up_to),
    )?;
    Ok(())
}

pub fn set_log_segment(conn: &Connection, channel: ChannelId, segment: SegmentId) -> Result<()> {
    conn.execute(
        "UPDATE channel SET log_segment = ?2 WHERE id = ?1",
        (channel, segment),
    )?;
    Ok(())
}

/// Live enabled groups a person is in: where he may speak unprompted.
pub fn enabled_groups_of(conn: &Connection, person: &PersonId) -> Result<Vec<Channel>> {
    let mut stmt = conn.prepare(
        "SELECT c.id FROM channel c JOIN channel_participant p ON p.channel = c.id
         WHERE p.participant = ?1 AND c.kind = 'group' AND c.mode = 'enabled'
           AND c.deleted_at IS NULL
         ORDER BY c.id",
    )?;
    let ids: Vec<ChannelId> = stmt
        .query_map([person], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    ids.into_iter().map(|id| get(conn, id)).collect()
}

/// The `as_person` a speaker's retrieval runs as in this channel. The host has none (its
/// search is the maintainer face: all of the corpus, no memories), and a person who is
/// not a participant cannot speak here at all.
pub fn as_person(
    conn: &Connection,
    channel: ChannelId,
    speaker: &Actor,
) -> Result<Option<PersonId>> {
    let ch = get(conn, channel)?;
    match speaker {
        Actor::User | Actor::Host => Ok(None),
        Actor::Person(p) if ch.has(speaker) => Ok(Some(p.clone())),
        Actor::Person(p) => Err(Error::NotParticipant {
            person: p.clone(),
            channel,
        }),
    }
}

/// Who answers a user turn in a group: the named participant, else the participant
/// whose own material matched best (`hits`, best first, from one retrieval over the
/// participants), else the previous speaker. Disabled people and non-participants are
/// never chosen. `None` when nobody qualifies.
pub fn address(
    conn: &Connection,
    channel: ChannelId,
    named: Option<&PersonId>,
    hits: &[PersonId],
) -> Result<Option<PersonId>> {
    let ch = get(conn, channel)?;
    let members = bond::filter_disabled(conn, ch.persons())?;
    if let Some(n) = named.filter(|n| members.contains(n)) {
        return Ok(Some(n.clone()));
    }
    if let Some(h) = hits.iter().find(|h| members.contains(h)) {
        return Ok(Some(h.clone()));
    }
    let mut stmt = conn.prepare(
        "SELECT author FROM message WHERE channel = ?1 AND author NOT IN ('user', 'host')
         ORDER BY at DESC, id DESC",
    )?;
    let authors = stmt.query_map([channel], |r| r.get::<_, PersonId>(0))?;
    for a in authors {
        let a = a?;
        if members.contains(&a) {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// Who adds one line after the addressed reply in a group: among the enabled members
/// other than `addressed`, the one whose own units matched best in the addressing
/// retrieval (`hits`: person and confidence), if that is above `min_confidence`. Only in
/// an enabled group, and only while the quota band lets unprompted speech through. One
/// per user turn; an interjection never triggers another, so the caller asks once.
pub fn interjector(
    conn: &Connection,
    channel: ChannelId,
    addressed: &PersonId,
    hits: &[(PersonId, f64)],
    min_confidence: f64,
    quota: &crate::quota::Status,
) -> Result<Option<PersonId>> {
    let ch = get(conn, channel)?;
    let unprompted = crate::quota::decide(quota, crate::quota::CallKind::Interjection).allowed();
    if ch.kind != ChannelKind::Group || ch.mode != Mode::Enabled || !ch.is_open() || !unprompted {
        return Ok(None);
    }
    let members = ch.persons();
    let mut best: Option<(&PersonId, f64)> = None;
    for (p, score) in hits {
        if p == addressed || !members.contains(p) || *score <= min_confidence {
            continue;
        }
        if bond::mode(conn, p)? != Mode::Enabled {
            continue;
        }
        if best.is_none_or(|(_, b)| *score > b) {
            best = Some((p, *score));
        }
    }
    Ok(best.map(|(p, _)| p.clone()))
}

/// The members who reply in order when a message speaks to several of them: `first`
/// (if it is among the candidates) and then the rest, those who spoke least recently
/// first (never-spoken ahead of all), ties in the order given; at most `max`. Only
/// members still in the group and not disabled count. Mode is not consulted: the user
/// spoke to them directly, so frozen members take part. Quota is the caller's to check
/// per turn.
pub fn round(
    conn: &Connection,
    channel: ChannelId,
    first: Option<&PersonId>,
    candidates: &[PersonId],
    max: u32,
) -> Result<Vec<PersonId>> {
    let ch = get(conn, channel)?;
    let members = bond::filter_disabled(conn, ch.persons())?;
    let mut stmt = conn.prepare(
        "SELECT author, MAX(id) FROM message WHERE channel = ?1
         AND author NOT IN ('user', 'host') GROUP BY author",
    )?;
    let last: std::collections::HashMap<PersonId, i64> = stmt
        .query_map([channel], |r| {
            Ok((r.get::<_, PersonId>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut rest: Vec<&PersonId> = candidates
        .iter()
        .filter(|p| members.contains(p) && Some(*p) != first)
        .collect();
    rest.sort_by_key(|p| last.get(*p).copied());
    let lead = first.filter(|f| candidates.contains(f) && members.contains(f));
    Ok(lead
        .into_iter()
        .chain(rest)
        .take(max as usize)
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    #[test]
    fn direct_channels_are_created_once_and_keep_their_owner() {
        let w = world();
        let a = direct(&w, &p("p1")).unwrap();
        assert_eq!(direct(&w, &p("p1")).unwrap(), a);
        add_participant(&w, a, &Actor::Person(p("p2"))).unwrap();
        let ch = get(&w, a).unwrap();
        assert_eq!(ch.person(), Some(&p("p1")));
        assert_eq!(ch.persons(), vec![p("p1"), p("p2")]);
        assert_ne!(direct(&w, &p("p2")).unwrap(), a);
        let h = host(&w).unwrap();
        assert!(get(&w, h).unwrap().is_host_channel());
        assert_eq!(host(&w).unwrap(), h);
    }

    #[test]
    fn members_join_and_leave_a_live_group() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        assert_eq!(add_members(&w, g, &[p("c"), p("a")]).unwrap(), vec![p("c")]);
        assert!(add_members(&w, g, &[p("c")]).unwrap().is_empty());
        assert_eq!(get(&w, g).unwrap().persons(), vec![p("a"), p("b"), p("c")]);
        assert!(remove_member(&w, g, &p("b")).unwrap());
        assert!(!remove_member(&w, g, &p("b")).unwrap());
        assert!(remove_member(&w, g, &p("c")).unwrap());
        assert!(remove_member(&w, g, &p("a")).is_err());
        assert_eq!(get(&w, g).unwrap().persons(), vec![p("a")]);
        let d = direct(&w, &p("a")).unwrap();
        assert!(add_members(&w, d, &[p("b")]).is_err());
        assert!(remove_member(&w, d, &p("a")).is_err());
    }

    #[test]
    fn a_group_is_found_by_exactly_its_people_while_live_and_unarchived() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        add_participant(&w, g, &Actor::Host).unwrap();
        let found = |who: &[&str]| {
            let who: Vec<PersonId> = who.iter().map(|x| p(x)).collect();
            find_group_with(&w, &who, None).unwrap().map(|c| c.id)
        };
        assert_eq!(found(&["b", "a"]), Some(g));
        assert_eq!(found(&["a"]), None);
        assert_eq!(found(&["a", "b", "c"]), None);
        let g2 = create_group(&w, &[p("a"), p("c")], None, Origin::User).unwrap();
        assert_eq!(groups_of(&w, &p("a")).unwrap()[0].id, g2);
        assert_eq!(groups_of(&w, &p("a")).unwrap().len(), 2);
        set_archived(&w, g, true).unwrap();
        assert_eq!(found(&["a", "b"]), None);
        assert_eq!(groups_of(&w, &p("a")).unwrap().len(), 1);
        delete_group(&w, g2, 1).unwrap();
        assert_eq!(found(&["a", "c"]), None);
        assert!(groups_of(&w, &p("a")).unwrap().is_empty());
    }

    #[test]
    fn a_proposed_group_is_named_by_its_purpose_and_reused_only_for_it() {
        let w = world();
        let who = [p("a"), p("b")];
        let named = create_proposed_group(&w, &who, Some("Plan a party"), Origin::User).unwrap();
        let plain = create_proposed_group(&w, &[p("a"), p("c")], None, Origin::User).unwrap();
        assert_eq!(
            get(&w, named).unwrap().name.as_deref(),
            Some("Plan a party")
        );
        assert_eq!(
            get(&w, named).unwrap().topic.as_deref(),
            Some("Plan a party")
        );
        assert_eq!(get(&w, plain).unwrap().name, None);
        let found = |who: &[PersonId], purpose: Option<&str>| {
            find_group_with(&w, who, purpose).unwrap().map(|c| c.id)
        };
        // No purpose: the people alone decide. Case and punctuation do not matter.
        assert_eq!(found(&who, None), Some(named));
        assert_eq!(found(&who, Some("  plan a PARTY! ")), Some(named));
        assert_eq!(found(&who, Some("plan-a-party")), Some(named));
        // Another purpose for the same people is another group.
        assert_eq!(found(&who, Some("Plan a trip")), None);
        assert_eq!(found(&who, Some("!!")), None);
        // The name the user gave counts as well as the topic made with it.
        set_name(&w, named, Some("Party crew")).unwrap();
        assert_eq!(found(&who, Some("party crew")), Some(named));
        assert_eq!(found(&who, Some("plan a party")), Some(named));
        // A group with no purpose of its own is found only without one.
        assert_eq!(found(&[p("a"), p("c")], Some("anything")), None);
        assert_eq!(found(&[p("a"), p("c")], None), Some(plain));
        // Archived and deleted groups are never reused.
        set_archived(&w, named, true).unwrap();
        assert_eq!(found(&who, Some("Plan a party")), None);
        set_archived(&w, named, false).unwrap();
        delete_group(&w, named, 1).unwrap();
        assert_eq!(found(&who, None), None);
    }

    #[test]
    fn name_avatar_and_archive_are_kept_and_a_person_unarchives() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        set_name(&w, g, Some("  Night shift ")).unwrap();
        set_avatar(&w, g, Some("x.png")).unwrap();
        set_archived(&w, g, true).unwrap();
        let ch = get(&w, g).unwrap();
        assert_eq!(ch.name.as_deref(), Some("Night shift"));
        assert_eq!(ch.avatar.as_deref(), Some("x.png"));
        assert!(ch.archived);
        // The user's own message leaves it archived; a person's brings it back.
        crate::message::append(&w, g, &Actor::User, "hi", None, None, 1).unwrap();
        assert!(get(&w, g).unwrap().archived);
        crate::message::append(&w, g, &Actor::Person(p("a")), "yo", None, None, 2).unwrap();
        assert!(!get(&w, g).unwrap().archived);
        set_name(&w, g, Some("   ")).unwrap();
        set_avatar(&w, g, None).unwrap();
        let ch = get(&w, g).unwrap();
        assert!(ch.name.is_none() && ch.avatar.is_none());
        let h = host(&w).unwrap();
        assert!(set_archived(&w, h, true).is_err());
    }

    #[test]
    fn a_failed_group_delete_leaves_the_group_whole() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        crate::message::append(&w, g, &Actor::User, "hi", None, None, 1).unwrap();
        crate::log::open_segment(&w, g, [b"a", b"b", b"c"], "{}", None, 1).unwrap();
        crate::session::open(
            &w,
            g,
            &p("a"),
            crate::session::Budget::User,
            crate::session::Cause::Addressed,
            1,
        )
        .unwrap();
        // The last statement of the delete (closing sessions) fails.
        w.execute_batch(
            "CREATE TRIGGER boom BEFORE UPDATE ON session
             BEGIN SELECT RAISE(ABORT, 'boom'); END",
        )
        .unwrap();
        assert!(delete_group(&w, g, 5).is_err());
        let ch = get(&w, g).unwrap();
        assert!(ch.deleted_at.is_none() && ch.log_segment.is_some());
        assert_eq!(crate::message::last(&w, g).unwrap().unwrap().text, "hi");
        assert!(crate::log::current(&w, g).unwrap().is_some());
    }

    #[test]
    fn groups_start_frozen_and_refuse_disabled_members() {
        let w = world();
        bond::set_mode(&w, &p("off"), Mode::Disabled).unwrap();
        assert!(matches!(
            create_group(&w, &[p("a"), p("off")], None, Origin::User),
            Err(Error::Disabled(_))
        ));
        assert!(matches!(direct(&w, &p("off")), Err(Error::Disabled(_))));
        let g = create_group(&w, &[p("a"), p("b")], Some("t"), Origin::User).unwrap();
        let ch = get(&w, g).unwrap();
        assert_eq!(
            (ch.mode, ch.kind, ch.topic.as_deref()),
            (Mode::Frozen, ChannelKind::Group, Some("t"))
        );
        assert!(add_participant(&w, g, &Actor::Person(p("off"))).is_err());
        set_mode(&w, g, Mode::Enabled).unwrap();
        assert_eq!(enabled_groups_of(&w, &p("a")).unwrap().len(), 1);
        set_mode(&w, g, Mode::Disabled).unwrap();
        assert!(!get(&w, g).unwrap().is_open());
        assert!(set_mode(&w, direct(&w, &p("a")).unwrap(), Mode::Enabled).is_err());
    }

    #[test]
    fn the_host_joins_and_leaves_a_live_group_without_counting_as_a_person() {
        let w = world();
        let g = create_group(&w, &[p("a")], None, Origin::User).unwrap();
        assert!(add_host(&w, g).unwrap());
        assert!(!add_host(&w, g).unwrap());
        let ch = get(&w, g).unwrap();
        assert!(ch.has(&Actor::Host));
        assert_eq!(ch.persons(), vec![p("a")]);
        // The last person still cannot leave, with the host there.
        assert!(remove_member(&w, g, &p("a")).is_err());
        assert!(remove_host(&w, g).unwrap());
        assert!(!remove_host(&w, g).unwrap());
        assert!(!get(&w, g).unwrap().has(&Actor::Host));
        let d = direct(&w, &p("a")).unwrap();
        assert!(add_host(&w, d).is_err());
        delete_group(&w, g, 1).unwrap();
        assert!(add_host(&w, g).is_err());
    }

    #[test]
    fn a_deleted_group_is_listed_found_and_changed_nowhere() {
        let w = world();
        let who = [p("a"), p("b")];
        let g = create_proposed_group(&w, &who, Some("tea"), Origin::User).unwrap();
        set_mode(&w, g, Mode::Enabled).unwrap();
        delete_group(&w, g, 7).unwrap();
        assert!(list(&w).unwrap().iter().all(|c| c.id != g));
        assert!(groups_of(&w, &p("a")).unwrap().is_empty());
        assert!(enabled_groups_of(&w, &p("a")).unwrap().is_empty());
        assert_eq!(find_group_with(&w, &who, None).unwrap(), None);
        assert_eq!(find_group_with(&w, &who, Some("tea")).unwrap(), None);
        assert!(add_members(&w, g, &[p("c")]).is_err());
        assert!(remove_member(&w, g, &p("b")).is_err());
        assert!(set_mode(&w, g, Mode::Frozen).is_err());
        // The same people can start a new one: it is not mistaken for the old.
        let again = create_proposed_group(&w, &who, Some("tea"), Origin::User).unwrap();
        assert_ne!(again, g);
        assert_eq!(
            find_group_with(&w, &who, Some("tea"))
                .unwrap()
                .map(|c| c.id),
            Some(again)
        );
    }

    #[test]
    fn deleting_a_group_keeps_participation() {
        let w = world();
        let g = create_group(&w, &[p("a")], None, Origin::User).unwrap();
        delete_group(&w, g, 5).unwrap();
        assert!(list(&w).unwrap().is_empty());
        assert!(matches!(
            add_participant(&w, g, &Actor::Person(p("b"))),
            Err(Error::ChannelClosed(_))
        ));
    }

    #[test]
    fn as_person_is_filled_from_the_speaker_never_the_model() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        assert_eq!(
            as_person(&w, g, &Actor::Person(p("a"))).unwrap(),
            Some(p("a"))
        );
        assert_eq!(as_person(&w, g, &Actor::Host).unwrap(), None);
        assert!(matches!(
            as_person(&w, g, &Actor::Person(p("c"))),
            Err(Error::NotParticipant { .. })
        ));
    }

    #[test]
    fn addressing_order() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b"), p("c")], None, Origin::User).unwrap();
        assert_eq!(
            address(&w, g, Some(&p("b")), &[p("a")]).unwrap(),
            Some(p("b"))
        );
        assert_eq!(
            address(&w, g, Some(&p("x")), &[p("x"), p("c")]).unwrap(),
            Some(p("c"))
        );
        assert_eq!(address(&w, g, None, &[]).unwrap(), None);
        crate::message::append(&w, g, &Actor::Person(p("a")), "hi", None, None, 1).unwrap();
        assert_eq!(address(&w, g, None, &[]).unwrap(), Some(p("a")));
        bond::set_mode(&w, &p("a"), Mode::Disabled).unwrap();
        assert_eq!(address(&w, g, Some(&p("a")), &[p("a")]).unwrap(), None);
    }

    #[test]
    fn round_orders_by_least_recent_speech_and_respects_the_cap() {
        let w = world();
        let g = create_group(&w, &[p("a"), p("b"), p("c"), p("d")], None, Origin::User).unwrap();
        let say = |who: &str, at: i64| {
            crate::message::append(&w, g, &Actor::Person(p(who)), "hi", None, None, at).unwrap();
        };
        say("c", 1);
        say("b", 2);
        let all = [p("a"), p("b"), p("c"), p("d")];
        // `a` leads; then `d` never spoke, `c` spoke before `b`.
        assert_eq!(
            round(&w, g, Some(&p("a")), &all, 4).unwrap(),
            vec![p("a"), p("d"), p("c"), p("b")]
        );
        assert_eq!(
            round(&w, g, Some(&p("b")), &all, 2).unwrap(),
            vec![p("b"), p("a")]
        );
        assert_eq!(
            round(&w, g, None, &all, 3).unwrap(),
            vec![p("a"), p("d"), p("c")]
        );
        assert!(round(&w, g, None, &all, 0).unwrap().is_empty());
        // Frozen members take part; disabled ones and strangers do not.
        bond::set_mode(&w, &p("d"), Mode::Disabled).unwrap();
        let more = [p("a"), p("b"), p("d"), p("x")];
        assert_eq!(
            round(&w, g, Some(&p("d")), &more, 4).unwrap(),
            vec![p("a"), p("b")]
        );
    }

    #[test]
    fn one_enabled_member_above_the_threshold_interjects() {
        let w = world();
        let open = crate::quota::status(&w, crate::Tier::Middle, 0, &Default::default()).unwrap();
        let g = create_group(&w, &[p("a"), p("b"), p("c"), p("d")], None, Origin::User).unwrap();
        for x in ["b", "c", "d"] {
            bond::set_mode(&w, &p(x), Mode::Enabled).unwrap();
        }
        let hits = [
            (p("a"), 0.9),
            (p("b"), 0.6),
            (p("c"), 0.7),
            (p("d"), 0.2),
            (p("x"), 0.99),
        ];
        assert_eq!(
            interjector(&w, g, &p("a"), &hits, 0.5, &open).unwrap(),
            None,
            "frozen group"
        );
        set_mode(&w, g, Mode::Enabled).unwrap();
        assert_eq!(
            interjector(&w, g, &p("a"), &hits, 0.5, &open).unwrap(),
            Some(p("c"))
        );
        assert_eq!(
            interjector(&w, g, &p("c"), &hits, 0.5, &open).unwrap(),
            Some(p("b"))
        );
        assert_eq!(
            interjector(&w, g, &p("a"), &hits, 0.8, &open).unwrap(),
            None
        );
        let quiet = crate::quota::Status {
            band: crate::quota::Band::Quiet,
            ..open
        };
        assert_eq!(
            interjector(&w, g, &p("a"), &hits, 0.5, &quiet).unwrap(),
            None
        );
    }

    #[test]
    fn read_marker_only_advances() {
        let w = world();
        let c = direct(&w, &p("a")).unwrap();
        mark_read(&w, c, MessageId(5)).unwrap();
        mark_read(&w, c, MessageId(3)).unwrap();
        assert_eq!(get(&w, c).unwrap().read_up_to, Some(MessageId(5)));
    }

    #[test]
    fn a_description_is_trimmed_cleared_and_only_a_groups() {
        let w = world();
        let g = create_group(&w, &[p("a")], None, Origin::User).unwrap();
        assert!(set_topic(&w, g, Some("  tea  ")).unwrap());
        assert_eq!(get(&w, g).unwrap().topic.as_deref(), Some("tea"));
        assert!(!set_topic(&w, g, Some("tea")).unwrap(), "unchanged");
        assert!(set_topic(&w, g, Some(" ")).unwrap());
        assert_eq!(get(&w, g).unwrap().topic, None);
        let d = direct(&w, &p("a")).unwrap();
        assert!(set_topic(&w, d, Some("x")).is_err());
    }
}
