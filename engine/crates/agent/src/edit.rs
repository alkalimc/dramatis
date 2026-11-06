//! Taking one message out of a conversation, and writing a reply again.
//!
//! A deleted message is gone from the channel and from what the model is shown next: the
//! current segment's log entries that carried it are dropped or rewritten, and where the
//! message sits in the opening block of the segment (it was one of the last messages of
//! the one before) the line goes from the block. What the log held before the cut keeps its
//! cached prefix; what follows pays once. Memories and summaries already written from the
//! conversation stay: a wrap-up covers a whole conversation and names no message, and the
//! user removes a memory from the memory screen.
//!
//! Regenerating the latest reply of a person or the host cuts the log back to the entry
//! that asked for it, and asks again. The old reply leaves the channel at once; if the new
//! one fails it comes back (as the latest message, with its log entries), so a failed try
//! loses nothing.

use api::events::{MessageAdded, MessageRemoved, WorldChange};
use llm::Part;
use world::log::{self, Role};
use world::quota::{CallKind, Decision};
use world::{Actor, ChannelId, ChannelKind, MessageId, SegmentId, Shape, channel, message};

use crate::agent::{Agent, Event};
use crate::assemble;
use crate::error::{Error, Result};
use crate::request;
use crate::session::Turn;
use crate::view;

/// A regeneration begun: the channel held, the old reply taken out, the new turn ready.
pub struct Regeneration {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    taken: Taken,
    turn: Turn,
}

/// What a regeneration took out, to put back if the new reply fails.
struct Taken {
    message: message::Message,
    entries: Vec<log::Entry>,
    seeds: Vec<i64>,
    segment: SegmentId,
    /// The segment's opening block as it was, when the question had to be taken from it.
    block: Option<Vec<u8>>,
}

impl Agent {
    /// Delete one message of `ch`, the user's or a person's or the host's. Refused while a
    /// turn is running in the channel. `MessageRemoved` follows.
    pub fn delete_message(&self, ch: ChannelId, id: MessageId) -> Result<()> {
        let _turn = self.try_lock_channel(ch).ok_or(Error::Busy)?;
        let m = message::get(&self.db(), id)?;
        if m.channel != ch {
            return Err(Error::Invalid("the message is in another channel".into()));
        }
        self.scrub(&m)?;
        message::remove(&self.db(), id)?;
        self.emit(Event::MessageRemoved(MessageRemoved {
            channel: view::channel_id(ch),
            message: view::message_id(id),
        }));
        self.changed(vec![WorldChange::Digest]);
        Ok(())
    }

    /// Take `m` out of what the model is shown: its entries in the current segment, or its
    /// line in that segment's opening block.
    fn scrub(&self, m: &message::Message) -> Result<()> {
        let conn = self.db();
        let Some(seg) = log::current(&conn, m.channel)? else {
            return Ok(());
        };
        let entries = log::entries(&conn, seg.id)?;
        let mut found = false;
        match &m.author {
            Actor::User => {
                let tag = assemble::message_tag(m.id);
                for e in entries.iter().filter(|e| e.role == Role::User) {
                    let Some(item) = request::parse(&e.bytes) else {
                        continue;
                    };
                    let carries = item
                        .parts
                        .iter()
                        .any(|p| matches!(p, Part::Text(t) if t.contains(&tag)));
                    if carries {
                        log::replace_entry(&conn, seg.id, e.seq, None)?;
                        found = true;
                    }
                }
            }
            _ => {
                // The turn's entries lie between the message's start and the next message.
                let until: i64 = conn.query_row(
                    "SELECT coalesce(min(at), ?3) FROM message WHERE channel = ?1 AND id > ?2",
                    (m.channel, m.id, i64::MAX),
                    |r| r.get(0),
                )?;
                let marks = assemble::closing_marks(&self.corpus.wording, true, true).list();
                for e in entries
                    .iter()
                    .filter(|e| e.role == Role::Assistant && e.at >= m.at && e.at <= until)
                {
                    let Some(mut item) = request::parse(&e.bytes) else {
                        continue;
                    };
                    let text: Vec<&str> = item
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !text.iter().any(|t| owns(&m.text, t, &marks)) {
                        continue;
                    }
                    item.parts.retain(|p| !matches!(p, Part::Text(_)));
                    let bytes = (!item.parts.is_empty()).then(|| request::to_bytes(&item));
                    log::replace_entry(&conn, seg.id, e.seq, bytes.as_deref())?;
                    found = true;
                }
            }
        }
        if !found {
            let block = String::from_utf8_lossy(&seg.prefix_c).into_owned();
            if let Some(cut) = without_recent(&block, &m.text) {
                log::replace_prefix_c(&conn, seg.id, cut.as_bytes())?;
            }
        }
        Ok(())
    }

    /// Write the latest reply of a person or the host again, in place of the old one (a
    /// bad first line of an opening, say). Refused while a turn runs in the channel, for
    /// a message that is not the channel's latest, and for the user's own. The turn runs to
    /// its end here; events carry what it writes.
    pub async fn regenerate(&self, ch: ChannelId, id: MessageId) -> Result<()> {
        let started = self.begin_regenerate(ch, id)?;
        self.finish_regenerate(started).await
    }

    /// The checks and the cut of [`Agent::regenerate`]: after this the old reply is out
    /// of the channel and the log, and the channel is held for the new one. Apart from
    /// `finish_regenerate` so a command can answer its refusals at once and run the rest in
    /// the background.
    pub fn begin_regenerate(&self, ch: ChannelId, id: MessageId) -> Result<Regeneration> {
        let guard = self.try_lock_channel(ch).ok_or(Error::Busy)?;
        let (m, before, kind) = {
            let conn = self.db();
            let m = message::get(&conn, id)?;
            let c = channel::get(&conn, ch)?;
            if m.channel != ch || c.is_favorites() {
                return Err(Error::Invalid("not a reply of this channel".into()));
            }
            if m.author == Actor::User
                || api::meta::MessageMeta::of(m.attachment.as_ref()).service
                || message::last(&conn, ch)?.is_none_or(|l| l.id != id)
            {
                return Err(Error::NotRegenerable("regenerate_latest"));
            }
            let before = message::history(&conn, ch, Some(id), 1)?.pop();
            (m, before, c.kind)
        };
        // An opening carries the seed it was delivered for; its words answer no one.
        let seeds = self.seeds_of(id)?;
        let answers = seeds.is_empty() && before.as_ref().is_some_and(|b| b.author == Actor::User);
        let host = self.log_kind(&self.db(), ch)? == crate::session::LogKind::Host;
        let shape = match (host, kind, answers) {
            (true, _, _) => Shape::Host,
            (_, ChannelKind::Group, true) => Shape::Group,
            (_, ChannelKind::Group, false) => Shape::Interject,
            (_, _, true) => Shape::Direct,
            (_, _, false) => Shape::Opening,
        };
        if let Decision::Exhausted { release_at } = self.decide(CallKind::UserInitiated)? {
            return Err(Error::QuotaExhausted { release_at });
        }
        let taken = self.take(&m, before.as_ref(), seeds)?;
        self.emit(Event::MessageRemoved(MessageRemoved {
            channel: view::channel_id(ch),
            message: view::message_id(id),
        }));
        let mut turn = Turn::new(ch, m.author.clone(), shape);
        turn.authorized = shape == Shape::Host && answers;
        Ok(Regeneration {
            _guard: guard,
            taken,
            turn,
        })
    }

    /// The new reply of a regeneration begun; the old one comes back if there is none.
    pub async fn finish_regenerate(&self, started: Regeneration) -> Result<()> {
        let Regeneration {
            _guard,
            taken,
            mut turn,
        } = started;
        let result = self
            .drive(&mut turn, self.config.agent.turn_loop.max_steps)
            .await;
        let made = turn.message.filter(|_| result.is_ok());
        match made {
            Some(new) => {
                let conn = self.db();
                for s in &taken.seeds {
                    conn.execute("UPDATE seed SET message = ?2 WHERE id = ?1", (s, new.0))?;
                }
                drop(conn);
                if let Actor::Person(p) = &taken.message.author {
                    self.replied(taken.message.channel, p)?;
                }
                self.changed(vec![WorldChange::Digest]);
                Ok(())
            }
            None => {
                self.put_back(taken)?;
                match result {
                    Ok(()) => Err(Error::Invalid("the reply came out empty".into())),
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// The seeds (planned openings) delivered as message `id`.
    fn seeds_of(&self, id: MessageId) -> Result<Vec<i64>> {
        let conn = self.db();
        let mut stmt = conn.prepare("SELECT id FROM seed WHERE message = ?1")?;
        Ok(stmt
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Cut `m` and the log entries after the entry that asked for it out of the channel.
    fn take(
        &self,
        m: &message::Message,
        before: Option<&message::Message>,
        seeds: Vec<i64>,
    ) -> Result<Taken> {
        let (seg, entries) = {
            let conn = self.db();
            let seg = log::current(&conn, m.channel)?
                .ok_or_else(|| Error::Invalid("nothing to answer".into()))?
                .id;
            (seg, log::entries(&conn, seg)?)
        };
        // A wrap-up request an older build left behind is not the question; it goes with
        // the old reply.
        let mut asked = None;
        for (i, e) in entries.iter().enumerate().rev() {
            if e.role == Role::User && !self.is_wrapup_request(seg, &e.bytes)? {
                asked = Some(i);
                break;
            }
        }
        let Some(asked) = asked else {
            return self.take_from_block(m, before, seg, seeds);
        };
        let removed: Vec<log::Entry> = entries[asked + 1..].to_vec();
        let conn = self.db();
        if let Some(first) = removed.first() {
            log::truncate(&conn, seg, first.seq)?;
        }
        message::remove(&conn, m.id)?;
        Ok(Taken {
            message: m.clone(),
            entries: removed,
            seeds,
            segment: seg,
            block: None,
        })
    }

    /// The reply sits in the opening block of a segment that has no turn of its own yet
    /// (the log rolled over after it was written): both it and the user's words it
    /// answered leave the block, and those words are asked again as the segment's first
    /// entry.
    fn take_from_block(
        &self,
        m: &message::Message,
        before: Option<&message::Message>,
        seg: SegmentId,
        seeds: Vec<i64>,
    ) -> Result<Taken> {
        let asked = before
            .filter(|b| b.author == Actor::User)
            .ok_or(Error::NotRegenerable("regenerate_question"))?;
        let block = log::current(&self.db(), m.channel)?
            .map(|s| s.prefix_c)
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&block).into_owned();
        let cut = without_recent(&text, &m.text)
            .and_then(|t| without_recent(&t, &asked.text))
            .ok_or(Error::NotRegenerable("regenerate_question"))?;
        log::replace_prefix_c(&self.db(), seg, cut.as_bytes())?;
        let posted = crate::flows::Posted {
            channel: m.channel,
            message: asked.id,
            text: asked.text.clone(),
            image: None,
            authorized: false,
            reply_to: asked.reply_to,
        };
        if let Err(e) = self.user_entry_with(seg, &posted, &[], &[]) {
            log::replace_prefix_c(&self.db(), seg, &block)?;
            return Err(e);
        }
        message::remove(&self.db(), m.id)?;
        Ok(Taken {
            message: m.clone(),
            entries: Vec::new(),
            seeds,
            segment: seg,
            block: Some(block),
        })
    }

    /// The old reply again, after the new one failed: its log entries at the end of the
    /// segment, the message as the channel's latest, announced like any new message.
    fn put_back(&self, taken: Taken) -> Result<()> {
        let m = &taken.message;
        let conn = self.db();
        if log::current(&conn, m.channel)?.is_some_and(|s| s.id == taken.segment) {
            if let Some(block) = &taken.block {
                log::truncate(&conn, taken.segment, 0)?;
                log::replace_prefix_c(&conn, taken.segment, block)?;
            }
            for e in &taken.entries {
                log::append(&conn, taken.segment, e.role, &e.bytes, e.at)?;
            }
        }
        let id = message::append(
            &conn,
            m.channel,
            &m.author,
            &m.text,
            m.tool_calls.as_ref(),
            m.attachment.as_ref(),
            m.at,
        )?;
        if let Some(r) = m.reply_to {
            let _ = message::set_reply_to(&conn, id, r);
        }
        for s in &taken.seeds {
            conn.execute("UPDATE seed SET message = ?2 WHERE id = ?1", (s, id.0))?;
        }
        let stored = message::get(&conn, id)?;
        let shown = view::message(&conn, &stored, self.now().offset_min);
        drop(conn);
        self.emit(Event::MessageAdded(MessageAdded { message: shown }));
        Ok(())
    }
}

/// Whether `entry` (the text of one reply step in the log) is the words of the message
/// `text`: at least half of its lines of speech occur in it. Closing lines and citation
/// lines are not speech.
fn owns(text: &str, entry: &str, marks: &[&str]) -> bool {
    let text = api::cites::strip(text);
    let lines: Vec<String> = api::cites::strip(entry)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !marks.iter().any(|m| !m.is_empty() && l.starts_with(m)))
        .map(str::to_owned)
        .collect();
    if lines.is_empty() {
        return false;
    }
    let inside = lines.iter().filter(|l| text.contains(l.as_str())).count();
    inside * 2 >= lines.len()
}

/// The opening block without the line `who: text` of a message.
fn without_recent(block: &str, text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let at = block.find(text)?;
    let start = block[..at].rfind('\n').map_or(0, |i| i + 1);
    // Only a line that opens with a speaker's name and a colon is a recent message.
    if !block[start..at].ends_with(": ") {
        return None;
    }
    let mut end = at + text.len();
    if block[end..].starts_with('\n') {
        end += 1;
    }
    Some(format!("{}{}", &block[..start], &block[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_found_by_its_lines_not_by_closing_lines() {
        let said = "(looks up)\nThe water wants to be cooler.";
        let marks = ["\u{21d2}"];
        assert!(owns(
            said,
            "(looks up)\nThe water wants to be cooler.\n\u{21d2} f: bo",
            &marks
        ));
        assert!(!owns(said, "Something else entirely.\nNo overlap.", &marks));
        assert!(!owns(said, "\u{21d2} f: bo", &marks));
        // A reply that echoed the line before it still counts: half its lines match.
        assert!(owns(
            said,
            "An echo of the line before.\nThe water wants to be cooler.",
            &marks
        ));
    }

    #[test]
    fn a_recent_line_leaves_the_opening_block() {
        let block = "Earlier:\nAnn: first\nsecond line\nBo: hello there\nAnn: last";
        assert_eq!(
            without_recent(block, "hello there").as_deref(),
            Some("Earlier:\nAnn: first\nsecond line\nAnn: last")
        );
        assert_eq!(
            without_recent(block, "first\nsecond line").as_deref(),
            Some("Earlier:\nBo: hello there\nAnn: last")
        );
        // Text that is not a message line stays put.
        assert_eq!(without_recent(block, "Earlier"), None);
        assert_eq!(without_recent(block, "absent"), None);
    }
}
