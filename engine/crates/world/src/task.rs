//! Requests: one person looks into a question within a turn budget.
//!
//! Turns can only be split, never created: a request's tree (it plus the colleagues it
//! pulled in, recursively) never holds more turns than the request was given. A pinned
//! question is a task too; requests filed under it are trees of their own.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::params::Ask;
use crate::types::{Actor, ChannelId, Citation, MessageId, PersonId, TaskId, TaskStatus, Tier};
use crate::{Error, Result, bond, channel, message, not_found, session};

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: TaskId,
    pub question: String,
    /// Who is on it; a pinned question may have nobody yet.
    pub assignee: Option<PersonId>,
    pub parent: Option<TaskId>,
    pub turns_left: u32,
    /// The turns the user gave it; `None` on a child split off by `request_join`.
    pub granted: Option<u32>,
    pub status: TaskStatus,
    pub pinned: bool,
    pub cites: Vec<Citation>,
    /// Relative to the office directory.
    pub note_path: Option<String>,
    pub channel: Option<ChannelId>,
    pub reply: Option<MessageId>,
    pub created_at: Option<i64>,
}

impl Task {
    /// The root of its delegation tree: it holds a grant (or predates grants).
    fn is_tree_root(&self) -> bool {
        self.granted.is_some() || self.parent.is_none()
    }
}

const COLUMNS: &str = "id, question, assignee, parent, turns_left, granted, status, pinned, cites, \
                       note_path, channel, reply, created_at";

fn read(row: &Row<'_>) -> rusqlite::Result<Task> {
    let cites: String = row.get("cites")?;
    Ok(Task {
        id: row.get("id")?,
        question: row.get("question")?,
        assignee: row.get("assignee")?,
        parent: row.get("parent")?,
        turns_left: row.get("turns_left")?,
        granted: row.get("granted")?,
        status: row.get("status")?,
        pinned: row.get("pinned")?,
        cites: serde_json::from_str(&cites).unwrap_or_default(),
        note_path: row.get("note_path")?,
        channel: row.get("channel")?,
        reply: row.get("reply")?,
        created_at: row.get("created_at")?,
    })
}

pub fn get(conn: &Connection, id: TaskId) -> Result<Task> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM task WHERE id = ?1"), [id], read)
        .optional()?
        .ok_or_else(|| not_found("task", id))
}

fn query(conn: &Connection, filter: &str, params: impl rusqlite::Params) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM task WHERE {filter} ORDER BY id"))?;
    let rows = stmt.query_map(params, read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn list(conn: &Connection) -> Result<Vec<Task>> {
    query(conn, "1", [])
}

pub fn children(conn: &Connection, id: TaskId) -> Result<Vec<Task>> {
    query(conn, "parent = ?1", [id])
}

/// The root of `id`'s delegation tree.
pub fn tree_root(conn: &Connection, id: TaskId) -> Result<Task> {
    let mut t = get(conn, id)?;
    while !t.is_tree_root() {
        t = get(conn, t.parent.expect("non-roots have a parent"))?;
    }
    Ok(t)
}

/// Every task in the tree rooted at `root`, root first.
pub fn tree(conn: &Connection, root: TaskId) -> Result<Vec<Task>> {
    let mut out = vec![get(conn, root)?];
    let mut i = 0;
    while i < out.len() {
        for c in children(conn, out[i].id)? {
            if !c.is_tree_root() {
                out.push(c);
            }
        }
        i += 1;
    }
    Ok(out)
}

/// What the tree containing `id` was given: the invariant's bound.
pub fn tree_budget(conn: &Connection, id: TaskId) -> Result<u32> {
    let root = tree_root(conn, id)?;
    Ok(root.granted.unwrap_or(root.turns_left))
}

/// Sum of turns_left over the tree containing `id`.
pub fn tree_turns_left(conn: &Connection, id: TaskId) -> Result<u32> {
    let root = tree_root(conn, id)?;
    Ok(tree(conn, root.id)?.iter().map(|t| t.turns_left).sum())
}

fn insert(
    conn: &Connection,
    question: &str,
    assignee: Option<&PersonId>,
    parent: Option<TaskId>,
    turns: u32,
    granted: Option<u32>,
    pinned: bool,
    channel: Option<ChannelId>,
    now: i64,
) -> Result<TaskId> {
    conn.execute(
        "INSERT INTO task(question, assignee, parent, turns_left, granted, pinned, channel, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![question, assignee, parent, turns, granted, pinned, channel, now],
    )?;
    Ok(TaskId(conn.last_insert_rowid()))
}

/// What `ask` did: the request, and the channel its harness turn goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    pub task: TaskId,
    pub channel: ChannelId,
}

/// The user (or the host for the user) asks a person to look into a question. `turns`
/// absent: the tier's default. `parent`: file it under a pinned question. Opens the
/// person's triggered session on this request (thawing him if frozen). The caller then
/// appends the harness turn to his direct channel.
pub fn ask(
    conn: &Connection,
    person: &PersonId,
    question: &str,
    turns: Option<u32>,
    parent: Option<TaskId>,
    tier: Tier,
    params: &Ask,
    now: i64,
) -> Result<Asked> {
    bond::ensure_available(conn, person)?;
    if question.trim().is_empty() {
        return Err(Error::Invalid("a request needs a question".into()));
    }
    let turns = turns.unwrap_or_else(|| params.turns.get(tier));
    if turns == 0 {
        return Err(Error::Invalid("a request needs at least one turn".into()));
    }
    if let Some(p) = parent {
        let parent = get(conn, p)?;
        if !parent.pinned {
            return Err(Error::Invalid(format!("request {p} is not a pinned question")));
        }
    }
    let ch = channel::direct(conn, person)?;
    let id = insert(conn, question.trim(), Some(person), parent, turns, Some(turns), false, Some(ch), now)?;
    session::open(conn, ch, person, session::Budget::Task(id), session::Cause::Ask, now)?;
    Ok(Asked { task: id, channel: ch })
}

/// A pinned question nobody is on yet.
pub fn open_case(conn: &Connection, question: &str, now: i64) -> Result<TaskId> {
    if question.trim().is_empty() {
        return Err(Error::Invalid("a question needs text".into()));
    }
    insert(conn, question.trim(), None, None, 0, Some(0), true, None, now)
}

pub fn set_pinned(conn: &Connection, id: TaskId, pinned: bool) -> Result<()> {
    get(conn, id)?;
    conn.execute("UPDATE task SET pinned = ?2 WHERE id = ?1", (id, pinned))?;
    Ok(())
}

pub fn pinned(conn: &Connection) -> Result<Vec<Task>> {
    query(conn, "pinned = 1", [])
}

/// The active request a person is working on in a channel, if any (newest first).
pub fn active_for(conn: &Connection, person: &PersonId, channel: ChannelId) -> Result<Option<Task>> {
    Ok(query(
        conn,
        "assignee = ?1 AND channel = ?2 AND status = 'active'",
        (person, channel),
    )?
    .pop())
}

/// What `request_join` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub person: PersonId,
    /// The child request when the caller was on one; `None` in an ordinary conversation,
    /// where the colleague answers once.
    pub task: Option<TaskId>,
    pub turns: u32,
}

/// Pull a colleague into `channel`. `candidate` has already been chosen by the caller
/// from the same retrieval as `find_people` (disabled people filtered out); here it is
/// checked again. Inside a request the colleague's turns are split off the caller's:
/// the caller keeps at least the turn he is spending now, and asking for more than
/// that leaves is an explicit error. Outside one the colleague answers once, paid by the
/// user's turn.
pub fn request_join(
    conn: &Connection,
    channel: ChannelId,
    caller: &PersonId,
    candidate: &PersonId,
    turns: Option<u32>,
    now: i64,
) -> Result<Joined> {
    bond::ensure_available(conn, candidate)?;
    if caller == candidate {
        return Err(Error::Invalid("cannot pull oneself in".into()));
    }
    let ch = channel::get(conn, channel)?;
    if !ch.has(&Actor::Person(caller.clone())) {
        return Err(Error::NotParticipant {
            person: caller.clone(),
            channel,
        });
    }
    let asked = turns.unwrap_or(1);
    if asked == 0 {
        return Err(Error::Invalid("hand over at least one turn".into()));
    }
    let task = match active_for(conn, caller, channel)? {
        Some(mine) => {
            // Keep the turn being spent on this call.
            let spare = mine.turns_left.saturating_sub(1);
            if asked > spare {
                return Err(Error::NotEnoughTurns { asked, spare });
            }
            conn.execute(
                "UPDATE task SET turns_left = turns_left - ?2 WHERE id = ?1",
                (mine.id, asked),
            )?;
            let child = insert(
                conn,
                &mine.question,
                Some(candidate),
                Some(mine.id),
                asked,
                None,
                false,
                Some(channel),
                now,
            )?;
            Some(child)
        }
        None => None,
    };
    channel::add_participant(conn, channel, &Actor::Person(candidate.clone()))?;
    let budget = match task {
        Some(t) => session::Budget::Task(t),
        None => session::Budget::Replies(1),
    };
    session::open(conn, channel, candidate, budget, session::Cause::Join, now)?;
    Ok(Joined {
        person: candidate.clone(),
        task,
        turns: if task.is_some() { asked } else { 1 },
    })
}

/// The harness behaviour for the next turn of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    /// More than one turn left.
    Normal,
    /// One left: append the line saying this turn must `report()`.
    MustReportThisTurn,
    /// None left: send what was found as the reply and close the request
    /// ([`force_close`]).
    ForceClose,
}

pub fn harness(turns_left: u32) -> Harness {
    match turns_left {
        0 => Harness::ForceClose,
        1 => Harness::MustReportThisTurn,
        _ => Harness::Normal,
    }
}

/// Spend one turn of a request (a model call made on it) and say what the harness does
/// next. A finished request spends nothing and says [`Harness::ForceClose`].
pub fn spend_turn(conn: &Connection, id: TaskId) -> Result<Harness> {
    let t = get(conn, id)?;
    if t.status == TaskStatus::Done {
        return Ok(Harness::ForceClose);
    }
    conn.execute(
        "UPDATE task SET turns_left = max(turns_left - 1, 0) WHERE id = ?1",
        [id],
    )?;
    Ok(harness(t.turns_left.saturating_sub(1)))
}

/// Where a request's note is written: always under the office directory, named by the
/// harness from the request id. The model never supplies a path.
pub fn note_rel_path(id: TaskId) -> String {
    format!("requests/{}.md", id.0)
}

/// What `report` stores.
#[derive(Debug, Clone, PartialEq)]
pub struct Reported {
    pub message: MessageId,
    pub note: Option<PathBuf>,
}

/// Answer a request: the reply goes into the request's channel as the person's message,
/// its citations are stored on the task, an optional note is written under `office`, and
/// the request is done. Someone with no such active request gets [`Error::NoTask`].
/// Closing a request also closes its session; the caller applies trust growth for a
/// request the user gave ([`crate::trust::task_done`]) when `granted` is set.
pub fn report(
    conn: &Connection,
    person: &PersonId,
    id: TaskId,
    text: &str,
    cites: &[Citation],
    note: Option<&str>,
    office: &Path,
    now: i64,
) -> Result<Reported> {
    let t = get(conn, id)?;
    let (Some(ch), true, true) = (
        t.channel,
        t.assignee.as_ref() == Some(person),
        t.status == TaskStatus::Active,
    ) else {
        return Err(Error::NoTask {
            person: person.clone(),
            task: id,
        });
    };
    let note_path = match note.filter(|n| !n.trim().is_empty()) {
        Some(body) => {
            let rel = note_rel_path(id);
            let full = office.join(&rel);
            if let Some(dir) = full.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&full, body)?;
            Some((rel, full))
        }
        None => None,
    };
    let msg = message::append(conn, ch, &Actor::Person(person.clone()), text, None, None, now)?;
    conn.execute(
        "UPDATE task SET status = 'done', cites = ?2, note_path = ?3, reply = ?4, turns_left = 0
         WHERE id = ?1",
        (
            id,
            serde_json::to_string(cites)?,
            note_path.as_ref().map(|(rel, _)| rel.as_str()),
            msg,
        ),
    )?;
    session::close_for_task(conn, id, now)?;
    Ok(Reported {
        message: msg,
        note: note_path.map(|(_, full)| full),
    })
}

/// Turns ran out: the caller sends what was found as the reply (`text`, possibly the
/// plain "found nothing" line) and the request is done. Same effects as [`report`]
/// without a note.
pub fn force_close(
    conn: &Connection,
    id: TaskId,
    text: &str,
    cites: &[Citation],
    office: &Path,
    now: i64,
) -> Result<Reported> {
    let t = get(conn, id)?;
    let person = t.assignee.ok_or_else(|| Error::Invalid(format!("request {id} has nobody on it")))?;
    report(conn, &person, id, text, cites, None, office, now)
}

/// Requests done since `since` whose answer the user has not read: "replied" in the
/// digest.
pub fn replied_unread(conn: &Connection) -> Result<Vec<Task>> {
    query(
        conn,
        "status = 'done' AND reply IS NOT NULL AND granted IS NOT NULL AND EXISTS (
             SELECT 1 FROM channel c WHERE c.id = task.channel
             AND (c.read_up_to IS NULL OR c.read_up_to < task.reply))",
        [],
    )
}

/// How many requests a person has finished.
pub fn done_count(conn: &Connection, person: &PersonId) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM task WHERE assignee = ?1 AND status = 'done'",
        [person],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use crate::types::Mode;

    fn office() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "world-office-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn ask_uses_the_tier_default_and_refuses_disabled() {
        let w = world();
        let a = ask(&w, &p("a"), "why?", None, None, Tier::High, &Ask::default(), T0).unwrap();
        let t = get(&w, a.task).unwrap();
        assert_eq!((t.turns_left, t.granted), (16, Some(16)));
        assert_eq!(t.channel, Some(a.channel));
        assert_eq!(tree_budget(&w, a.task).unwrap(), 16);
        bond::set_mode(&w, &p("off"), Mode::Disabled).unwrap();
        assert!(matches!(
            ask(&w, &p("off"), "q", Some(3), None, Tier::Low, &Ask::default(), T0),
            Err(Error::Disabled(_))
        ));
        assert!(ask(&w, &p("a"), "q", Some(0), None, Tier::Low, &Ask::default(), T0).is_err());
    }

    #[test]
    fn request_join_splits_and_refuses_what_it_cannot_give() {
        let w = world();
        let a = ask(&w, &p("a"), "q", Some(4), None, Tier::Middle, &Ask::default(), T0).unwrap();
        let j = request_join(&w, a.channel, &p("a"), &p("b"), Some(2), T0).unwrap();
        let child = j.task.unwrap();
        assert_eq!(get(&w, a.task).unwrap().turns_left, 2);
        assert_eq!(get(&w, child).unwrap().turns_left, 2);
        assert_eq!(tree_turns_left(&w, child).unwrap(), 4);
        assert!(matches!(
            request_join(&w, a.channel, &p("a"), &p("c"), Some(2), T0),
            Err(Error::NotEnoughTurns { asked: 2, spare: 1 })
        ));
        // The colleague may split his own share further.
        let g = request_join(&w, a.channel, &p("b"), &p("c"), Some(1), T0).unwrap();
        assert_eq!(tree_root(&w, g.task.unwrap()).unwrap().id, a.task);
        assert_eq!(tree_turns_left(&w, a.task).unwrap(), 4);
        assert!(matches!(
            request_join(&w, a.channel, &p("c"), &p("d"), None, T0),
            Err(Error::NotEnoughTurns { asked: 1, spare: 0 })
        ));
        assert!(channel::get(&w, a.channel).unwrap().has(&Actor::Person(p("c"))));
    }

    #[test]
    fn request_join_outside_a_request_answers_once() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let j = request_join(&w, c, &p("a"), &p("b"), Some(5), T0).unwrap();
        assert_eq!((j.task, j.turns), (None, 1));
        bond::set_mode(&w, &p("off"), Mode::Disabled).unwrap();
        assert!(matches!(
            request_join(&w, c, &p("a"), &p("off"), None, T0),
            Err(Error::Disabled(_))
        ));
        assert!(matches!(
            request_join(&w, c, &p("zz"), &p("b"), None, T0),
            Err(Error::NotParticipant { .. })
        ));
    }

    #[test]
    fn turns_left_three_bands() {
        let w = world();
        let a = ask(&w, &p("a"), "q", Some(3), None, Tier::Middle, &Ask::default(), T0).unwrap();
        assert_eq!(harness(3), Harness::Normal);
        assert_eq!(spend_turn(&w, a.task).unwrap(), Harness::Normal);
        assert_eq!(spend_turn(&w, a.task).unwrap(), Harness::MustReportThisTurn);
        assert_eq!(spend_turn(&w, a.task).unwrap(), Harness::ForceClose);
        assert_eq!(spend_turn(&w, a.task).unwrap(), Harness::ForceClose);
        assert_eq!(get(&w, a.task).unwrap().turns_left, 0);
        let dir = office();
        force_close(&w, a.task, "what I found", &[], &dir, T0 + 1).unwrap();
        let t = get(&w, a.task).unwrap();
        assert_eq!(t.status, TaskStatus::Done);
        assert!(t.reply.is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn report_stores_reply_cites_and_note_under_office() {
        let w = world();
        let a = ask(&w, &p("a"), "q", Some(3), None, Tier::Middle, &Ask::default(), T0).unwrap();
        let dir = office();
        let cite = Citation {
            page: "P".into(),
            revid: 1,
            span_from: 0,
            span_to: 2,
            chunk_id: "c1".into(),
        };
        assert!(matches!(
            report(&w, &p("b"), a.task, "x", &[], None, &dir, T0),
            Err(Error::NoTask { .. })
        ));
        let r = report(&w, &p("a"), a.task, "found it", &[cite.clone()], Some("# Note"), &dir, T0 + 5).unwrap();
        let t = get(&w, a.task).unwrap();
        assert_eq!(t.cites, vec![cite]);
        assert_eq!(t.note_path.as_deref(), Some(note_rel_path(a.task).as_str()));
        let note = r.note.unwrap();
        assert!(note.starts_with(&dir));
        assert_eq!(fs::read_to_string(&note).unwrap(), "# Note");
        assert_eq!(message::get(&w, r.message).unwrap().text, "found it");
        assert_eq!(replied_unread(&w).unwrap().len(), 1);
        channel::mark_read(&w, a.channel, r.message).unwrap();
        assert!(replied_unread(&w).unwrap().is_empty());
        assert!(matches!(
            report(&w, &p("a"), a.task, "again", &[], None, &dir, T0),
            Err(Error::NoTask { .. })
        ));
        assert_eq!(done_count(&w, &p("a")).unwrap(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pinned_questions_hold_requests_as_separate_trees() {
        let w = world();
        let case = open_case(&w, "the big one", T0).unwrap();
        let a = ask(&w, &p("a"), "part", Some(5), Some(case), Tier::Middle, &Ask::default(), T0).unwrap();
        assert_eq!(get(&w, a.task).unwrap().parent, Some(case));
        assert_eq!(tree_root(&w, a.task).unwrap().id, a.task);
        assert_eq!(tree_budget(&w, case).unwrap(), 0);
        assert_eq!(pinned(&w).unwrap().len(), 1);
        assert!(ask(&w, &p("a"), "x", Some(1), Some(a.task), Tier::Middle, &Ask::default(), T0).is_err());
    }
}
