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

    /// Accepts new messages: not deleted, and not a disabled group.
    pub fn is_open(&self) -> bool {
        self.deleted_at.is_none()
            && !(self.kind == ChannelKind::Group && self.mode == Mode::Disabled)
    }
}

const COLUMNS: &str =
    "id, kind, origin, topic, mode, log_segment, read_up_to, direct_with, deleted_at";

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

/// A new group. Every group starts frozen, whoever made it; disabled people cannot be
/// members.
pub fn create_group(
    conn: &Connection,
    members: &[PersonId],
    topic: Option<&str>,
    origin: Origin,
) -> Result<ChannelId> {
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

/// Delete a group: its messages, sessions and logs go; the row and its participants stay,
/// so memories scoped to it remain with the people who were there.
pub fn delete_group(conn: &Connection, channel: ChannelId, now: i64) -> Result<()> {
    let ch = get(conn, channel)?;
    if ch.kind != ChannelKind::Group {
        return Err(Error::Invalid(format!("channel {channel} is not a group")));
    }
    conn.execute(
        "UPDATE channel SET deleted_at = ?2, log_segment = NULL WHERE id = ?1",
        (channel, now),
    )?;
    conn.execute("DELETE FROM log_segment WHERE channel = ?1", [channel])?;
    conn.execute(
        "UPDATE task SET reply = NULL WHERE reply IN (SELECT id FROM message WHERE channel = ?1)",
        [channel],
    )?;
    conn.execute("DELETE FROM message WHERE channel = ?1", [channel])?;
    conn.execute(
        "UPDATE session SET closed_at = ?2 WHERE channel = ?1 AND closed_at IS NULL",
        (channel, now),
    )?;
    Ok(())
}

pub fn set_topic(conn: &Connection, channel: ChannelId, topic: Option<&str>) -> Result<()> {
    get(conn, channel)?;
    conn.execute(
        "UPDATE channel SET topic = ?2 WHERE id = ?1",
        (channel, topic),
    )?;
    Ok(())
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

/// Channels a person takes part in (deleted groups included: participation is history).
pub fn channels_of(conn: &Connection, person: &PersonId) -> Result<Vec<ChannelId>> {
    let mut stmt = conn.prepare(
        "SELECT channel FROM channel_participant WHERE participant = ?1 ORDER BY channel",
    )?;
    let rows = stmt.query_map([person], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
    fn deleting_a_group_keeps_participation() {
        let w = world();
        let g = create_group(&w, &[p("a")], None, Origin::User).unwrap();
        delete_group(&w, g, 5).unwrap();
        assert_eq!(channels_of(&w, &p("a")).unwrap(), vec![g]);
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
}
