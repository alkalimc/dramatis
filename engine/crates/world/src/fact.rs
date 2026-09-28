//! Memory: the one writable narrative state.
//!
//! Visibility is a hard filter applied when the candidate set is built: every query here
//! returns exactly the facts a reader may see, never a larger set to be trimmed later.
//! A retracted fact is gone from every reader but still deduplicates future writes, so a
//! deleted error does not come back; it can be restored.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::types::{Actor, ChannelId, Citation, FactId, FactKind, PersonId, TaskId};
use crate::{Error, Result, channel, not_found, trust};

/// Who may recall a fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Audience {
    /// Everyone (the user's conclusions).
    World,
    /// Whoever takes part in the channel, now or later.
    Participants(ChannelId),
    /// One person's own observation.
    Own(PersonId),
}

impl Audience {
    fn encode(&self) -> (&'static str, Option<String>) {
        match self {
            Self::World => ("world", None),
            Self::Participants(c) => ("participants", Some(c.0.to_string())),
            Self::Own(p) => ("self", Some(p.0.clone())),
        }
    }

    fn decode(kind: &str, reference: Option<String>) -> rusqlite::Result<Self> {
        let bad = || rusqlite::Error::InvalidColumnType(0, "audience".into(), rusqlite::types::Type::Text);
        Ok(match (kind, reference) {
            ("world", None) => Self::World,
            ("participants", Some(r)) => Self::Participants(ChannelId(r.parse().map_err(|_| bad())?)),
            ("self", Some(r)) => Self::Own(PersonId(r)),
            _ => return Err(bad()),
        })
    }
}

/// What a fact is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum About {
    Task(TaskId),
    Fact(FactId),
    Person(PersonId),
    User,
    Citation(Citation),
}

impl About {
    fn encode(&self) -> Result<(&'static str, Option<String>)> {
        Ok(match self {
            Self::Task(t) => ("task", Some(t.0.to_string())),
            Self::Fact(f) => ("fact", Some(f.0.to_string())),
            Self::Person(p) => ("person", Some(p.0.clone())),
            Self::User => ("user", None),
            Self::Citation(c) => ("citation", Some(serde_json::to_string(c)?)),
        })
    }

    fn decode(kind: Option<String>, reference: Option<String>) -> Option<Self> {
        Some(match (kind?.as_str(), reference) {
            ("task", Some(r)) => Self::Task(TaskId(r.parse().ok()?)),
            ("fact", Some(r)) => Self::Fact(FactId(r.parse().ok()?)),
            ("person", Some(r)) => Self::Person(PersonId(r)),
            ("user", None) => Self::User,
            ("citation", Some(r)) => Self::Citation(serde_json::from_str(&r).ok()?),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    pub id: FactId,
    pub author: Actor,
    pub audience: Audience,
    pub kind: FactKind,
    pub about: Option<About>,
    pub due: Option<i64>,
    pub delivered: bool,
    pub text: String,
    pub retracted: bool,
    pub created_at: i64,
}

/// A fact to write.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFact {
    pub author: Actor,
    pub audience: Audience,
    pub kind: FactKind,
    pub about: Option<About>,
    pub due: Option<i64>,
    pub text: String,
}

impl NewFact {
    pub fn new(author: Actor, audience: Audience, kind: FactKind, text: impl Into<String>) -> Self {
        Self {
            author,
            audience,
            kind,
            about: None,
            due: None,
            text: text.into(),
        }
    }

    pub fn about(mut self, about: About) -> Self {
        self.about = Some(about);
        self
    }

    pub fn due(mut self, due: i64) -> Self {
        self.due = Some(due);
        self
    }
}

/// The outcome of a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    New(FactId),
    /// Same kind, audience and text as an existing fact (live or retracted); nothing was
    /// written. A retracted duplicate stays retracted.
    Duplicate { existing: FactId, retracted: bool },
}

impl Written {
    pub fn id(self) -> FactId {
        match self {
            Self::New(id) | Self::Duplicate { existing: id, .. } => id,
        }
    }

    pub fn is_new(self) -> bool {
        matches!(self, Self::New(_))
    }
}

const COLUMNS: &str = "id, author, audience_kind, audience_ref, kind, about_kind, about_ref, \
                       due, delivered, text, retracted, created_at";

fn read(row: &Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get("id")?,
        author: row.get("author")?,
        audience: Audience::decode(&row.get::<_, String>("audience_kind")?, row.get("audience_ref")?)?,
        kind: row.get("kind")?,
        about: About::decode(row.get("about_kind")?, row.get("about_ref")?),
        due: row.get("due")?,
        delivered: row.get("delivered")?,
        text: row.get("text")?,
        retracted: row.get("retracted")?,
        created_at: row.get("created_at")?,
    })
}

fn query(conn: &Connection, filter: &str, params: impl rusqlite::Params) -> Result<Vec<Fact>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM fact WHERE {filter} ORDER BY id"))?;
    let rows = stmt.query_map(params, read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The dedupe form of a text: letters and digits of any script, case-folded. Spacing and
/// punctuation differences do not make a new memory.
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// An existing fact (live or retracted) the new one would repeat.
pub fn find_duplicate(conn: &Connection, new: &NewFact) -> Result<Option<Fact>> {
    let key = normalize(&new.text);
    let (kind, reference) = new.audience.encode();
    let same = query(
        conn,
        "kind = ?1 AND audience_kind = ?2 AND audience_ref IS ?3",
        (new.kind, kind, reference),
    )?;
    Ok(same.into_iter().find(|f| normalize(&f.text) == key))
}

/// Write a fact unless it repeats an existing one, retracted ones included.
pub fn write(conn: &Connection, new: &NewFact, now: i64) -> Result<Written> {
    if normalize(&new.text).is_empty() {
        return Err(Error::Invalid("a memory needs some text".into()));
    }
    if let Audience::Participants(c) = new.audience {
        channel::get(conn, c)?;
    }
    if let Some(existing) = find_duplicate(conn, new)? {
        return Ok(Written::Duplicate {
            existing: existing.id,
            retracted: existing.retracted,
        });
    }
    let (audience_kind, audience_ref) = new.audience.encode();
    let (about_kind, about_ref) = match &new.about {
        Some(a) => {
            let (k, r) = a.encode()?;
            (Some(k), r)
        }
        None => (None, None),
    };
    conn.execute(
        "INSERT INTO fact(author, audience_kind, audience_ref, kind, about_kind, about_ref,
                          due, text, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            new.author,
            audience_kind,
            audience_ref,
            new.kind,
            about_kind,
            about_ref,
            new.due,
            new.text.trim(),
            now
        ],
    )?;
    Ok(Written::New(FactId(conn.last_insert_rowid())))
}

pub fn get(conn: &Connection, id: FactId) -> Result<Fact> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM fact WHERE id = ?1"), [id], read)
        .optional()?
        .ok_or_else(|| not_found("fact", id))
}

/// Delete: gone from retrieval, seeds and every reader; restorable. Retracting a hurt
/// gives back the trust it took.
pub fn retract(conn: &Connection, id: FactId) -> Result<()> {
    let f = get(conn, id)?;
    if f.retracted {
        return Ok(());
    }
    conn.execute("UPDATE fact SET retracted = 1 WHERE id = ?1", [id])?;
    if f.kind == FactKind::Hurt {
        trust::undo_hurt(conn, &f)?;
    }
    Ok(())
}

/// Undo a delete. Restoring a hurt takes its trust again.
pub fn restore(conn: &Connection, id: FactId) -> Result<()> {
    let f = get(conn, id)?;
    if !f.retracted {
        return Ok(());
    }
    conn.execute("UPDATE fact SET retracted = 0 WHERE id = ?1", [id])?;
    if f.kind == FactKind::Hurt {
        trust::redo_hurt(conn, &f)?;
    }
    Ok(())
}

/// Edit = retract + a new fact by the user with the same kind, audience, subject and due.
/// When the new text repeats an existing fact, that fact is returned (restored if it was
/// deleted: the user wrote it again on purpose).
pub fn edit(conn: &Connection, id: FactId, text: &str, now: i64) -> Result<FactId> {
    let old = get(conn, id)?;
    let new = NewFact {
        author: Actor::User,
        audience: old.audience.clone(),
        kind: old.kind,
        about: old.about.clone(),
        due: old.due,
        text: text.to_owned(),
    };
    if normalize(text).is_empty() {
        return Err(Error::Invalid("a memory needs some text".into()));
    }
    retract(conn, id)?;
    match write(conn, &new, now)? {
        Written::New(n) => Ok(n),
        Written::Duplicate { existing, .. } => {
            restore(conn, existing)?;
            Ok(existing)
        }
    }
}

/// Every fact, oldest first; retracted ones only when asked for (the drawer's view).
pub fn all(conn: &Connection, include_retracted: bool) -> Result<Vec<Fact>> {
    if include_retracted {
        query(conn, "1", [])
    } else {
        query(conn, "retracted = 0", [])
    }
}

/// The persons an audience reaches; `None` = everyone.
pub fn audience_persons(conn: &Connection, audience: &Audience) -> Result<Option<Vec<PersonId>>> {
    Ok(match audience {
        Audience::World => None,
        Audience::Own(p) => Some(vec![p.clone()]),
        Audience::Participants(c) => Some(channel::get(conn, *c)?.persons()),
    })
}

/// SQL condition: fact visible to person `?1`.
const VISIBLE_TO_PERSON: &str = "retracted = 0 AND (
    audience_kind = 'world'
    OR (audience_kind = 'self' AND audience_ref = ?1)
    OR (audience_kind = 'participants' AND EXISTS (
        SELECT 1 FROM channel_participant cp
        WHERE cp.channel = CAST(fact.audience_ref AS INTEGER) AND cp.participant = ?1)))";

/// Everything a person may recall, anywhere.
pub fn visible_to_person(conn: &Connection, person: &PersonId) -> Result<Vec<Fact>> {
    query(conn, VISIBLE_TO_PERSON, [person])
}

pub fn can_see(conn: &Connection, person: &PersonId, id: FactId) -> Result<bool> {
    Ok(conn
        .query_row(
            &format!("SELECT 1 FROM fact WHERE id = ?2 AND {VISIBLE_TO_PERSON}"),
            (person, id),
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The material a shared log may carry: facts every person in the channel may recall.
/// A speaker's own observations and memories from rooms the others were not in are not
/// here, so they never enter a log the others read. The host takes no part: it has no
/// memories. A channel with no persons shares only what the world knows.
pub fn shared_visible(conn: &Connection, channel: ChannelId) -> Result<Vec<Fact>> {
    let persons = channel::get(conn, channel)?.persons();
    let Some((first, rest)) = persons.split_first() else {
        return query(conn, "retracted = 0 AND audience_kind = 'world'", []);
    };
    let mut keep: BTreeSet<FactId> = visible_to_person(conn, first)?.iter().map(|f| f.id).collect();
    for p in rest {
        let theirs: BTreeSet<FactId> = visible_to_person(conn, p)?.iter().map(|f| f.id).collect();
        keep.retain(|id| theirs.contains(id));
    }
    Ok(visible_to_person(conn, first)?
        .into_iter()
        .filter(|f| keep.contains(&f.id))
        .collect())
}

/// The memory candidates for a speaker's retrieval in a channel, filtered before any
/// ranking. `None` is the maintainer face (the host's search): no memories at all. A
/// person alone with the user sees all he may recall; in a channel others read he gets
/// only [`shared_visible`], because what he retrieves lands in the shared log.
pub fn visible_to(conn: &Connection, as_person: Option<&PersonId>, channel: ChannelId) -> Result<Vec<Fact>> {
    let Some(person) = as_person else {
        return Ok(Vec::new());
    };
    let ch = channel::get(conn, channel)?;
    if !ch.has(&Actor::Person(person.clone())) {
        return Err(Error::NotParticipant {
            person: person.clone(),
            channel,
        });
    }
    if ch.persons() == [person.clone()] {
        visible_to_person(conn, person)
    } else {
        shared_visible(conn, channel)
    }
}

/// The audience the harness gives a memory written in a channel (`remember`, wrap-up):
/// whoever takes part there. Never chosen by the model.
pub fn channel_audience(channel: ChannelId) -> Audience {
    Audience::Participants(channel)
}

/// Commitments due by `now` and not yet brought up.
pub fn due_commitments(conn: &Connection, now: i64) -> Result<Vec<Fact>> {
    query(
        conn,
        "kind = 'commitment' AND retracted = 0 AND delivered = 0 AND due IS NOT NULL AND due <= ?1",
        [now],
    )
}

pub fn mark_delivered(conn: &Connection, id: FactId) -> Result<()> {
    conn.execute("UPDATE fact SET delivered = 1 WHERE id = ?1", [id])?;
    Ok(())
}

/// The user's conclusion on a question, if any (live).
pub fn conclusion_of(conn: &Connection, task: TaskId) -> Result<Option<Fact>> {
    Ok(query(
        conn,
        "kind = 'conclusion' AND retracted = 0 AND about_kind = 'task' AND about_ref = ?1",
        [task.0.to_string()],
    )?
    .pop())
}

/// Write or replace the user's conclusion on a question: visible to everyone; replacing
/// retracts the old one.
pub fn write_conclusion(conn: &Connection, task: TaskId, text: &str, now: i64) -> Result<FactId> {
    crate::task::get(conn, task)?;
    let old = conclusion_of(conn, task)?;
    let new = NewFact::new(Actor::User, Audience::World, FactKind::Conclusion, text).about(About::Task(task));
    if let Some(old) = &old {
        if normalize(&old.text) == normalize(text) {
            return Ok(old.id);
        }
        retract(conn, old.id)?;
    }
    match write(conn, &new, now)? {
        Written::New(id) => Ok(id),
        Written::Duplicate { existing, .. } => {
            restore(conn, existing)?;
            Ok(existing)
        }
    }
}

/// The drawer's filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    World,
    Own,
    Commitment,
    Hurt,
    Deleted,
}

/// The drawer: all facts (live, or only deleted ones under [`Scope::Deleted`]) matching
/// the scope, optionally only those about or recallable by a person, or about a request.
pub fn list(conn: &Connection, scope: Scope, person: Option<&PersonId>, task: Option<TaskId>) -> Result<Vec<Fact>> {
    let mut facts = all(conn, true)?;
    facts.retain(|f| match scope {
        Scope::Deleted => f.retracted,
        _ => !f.retracted,
    });
    facts.retain(|f| match scope {
        Scope::All | Scope::Deleted => true,
        Scope::World => f.audience == Audience::World,
        Scope::Own => matches!(f.audience, Audience::Own(_)),
        Scope::Commitment => f.kind == FactKind::Commitment,
        Scope::Hurt => f.kind == FactKind::Hurt,
    });
    if let Some(t) = task {
        facts.retain(|f| f.about == Some(About::Task(t)));
    }
    if let Some(p) = person {
        let mut keep = Vec::with_capacity(facts.len());
        for f in facts {
            let about = f.about == Some(About::Person(p.clone()));
            let reach = match audience_persons(conn, &f.audience)? {
                None => true,
                Some(ps) => ps.contains(p),
            };
            if about || reach {
                keep.push(f);
            }
        }
        facts = keep;
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use crate::types::Origin;

    fn note(audience: Audience, text: &str) -> NewFact {
        NewFact::new(Actor::User, audience, FactKind::Fact, text)
    }

    #[test]
    fn dedupes_against_everything_including_deleted() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let aud = Audience::Participants(c);
        let first = write(&w, &note(aud.clone(), "Likes tea."), 1).unwrap();
        assert!(first.is_new());
        assert_eq!(
            write(&w, &note(aud.clone(), "  likes TEA "), 2).unwrap(),
            Written::Duplicate { existing: first.id(), retracted: false }
        );
        retract(&w, first.id()).unwrap();
        assert_eq!(
            write(&w, &note(aud.clone(), "likes tea"), 3).unwrap(),
            Written::Duplicate { existing: first.id(), retracted: true }
        );
        assert!(get(&w, first.id()).unwrap().retracted, "a deleted error does not come back");
        // Another audience or kind is another memory.
        assert!(write(&w, &note(Audience::World, "likes tea"), 4).unwrap().is_new());
        restore(&w, first.id()).unwrap();
        assert!(!get(&w, first.id()).unwrap().retracted);
        assert!(write(&w, &note(aud, " . "), 5).is_err());
    }

    #[test]
    fn edit_is_retract_plus_write() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let f = write(&w, &note(Audience::Participants(c), "old").about(About::User), 1)
            .unwrap()
            .id();
        let n = edit(&w, f, "new", 2).unwrap();
        assert_ne!(n, f);
        assert!(get(&w, f).unwrap().retracted);
        let nf = get(&w, n).unwrap();
        assert_eq!((nf.text.as_str(), nf.about, nf.author), ("new", Some(About::User), Actor::User));
        // Editing back to the deleted text restores it instead of duplicating.
        assert_eq!(edit(&w, n, "old", 3).unwrap(), f);
        assert!(!get(&w, f).unwrap().retracted);
    }

    #[test]
    fn visibility_by_audience() {
        let w = world();
        let g = channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        let da = channel::direct(&w, &p("a")).unwrap();
        let world_f = write(&w, &note(Audience::World, "w"), 1).unwrap().id();
        let group_f = write(&w, &note(Audience::Participants(g), "g"), 1).unwrap().id();
        let own_a = write(&w, &note(Audience::Own(p("a")), "own"), 1).unwrap().id();
        let direct_a = write(&w, &note(Audience::Participants(da), "d"), 1).unwrap().id();
        let ids = |fs: Vec<Fact>| fs.into_iter().map(|f| f.id).collect::<Vec<_>>();

        assert_eq!(ids(visible_to_person(&w, &p("a")).unwrap()), [world_f, group_f, own_a, direct_a]);
        assert_eq!(ids(visible_to_person(&w, &p("b")).unwrap()), [world_f, group_f]);
        assert_eq!(ids(visible_to_person(&w, &p("c")).unwrap()), [world_f]);
        assert!(can_see(&w, &p("b"), group_f).unwrap());
        assert!(!can_see(&w, &p("b"), own_a).unwrap());

        // Alone with the user he has all of his; in the group only the shared part.
        assert_eq!(ids(visible_to(&w, Some(&p("a")), da).unwrap()), [world_f, group_f, own_a, direct_a]);
        assert_eq!(ids(visible_to(&w, Some(&p("a")), g).unwrap()), [world_f, group_f]);
        assert!(visible_to(&w, None, g).unwrap().is_empty());

        // Joining later is having been brought in: he now recalls the room's memories.
        channel::add_participant(&w, g, &Actor::Person(p("c"))).unwrap();
        assert!(can_see(&w, &p("c"), group_f).unwrap());

        retract(&w, group_f).unwrap();
        assert!(!can_see(&w, &p("a"), group_f).unwrap());
        assert_eq!(audience_persons(&w, &Audience::Participants(g)).unwrap(), Some(vec![p("a"), p("b"), p("c")]));
    }

    #[test]
    fn conclusions_replace_and_drawer_filters() {
        let w = world();
        let t = crate::task::open_case(&w, "q", 1).unwrap();
        let c1 = write_conclusion(&w, t, "first", 2).unwrap();
        assert_eq!(write_conclusion(&w, t, "First!", 3).unwrap(), c1);
        let c2 = write_conclusion(&w, t, "second", 4).unwrap();
        assert!(get(&w, c1).unwrap().retracted);
        assert_eq!(conclusion_of(&w, t).unwrap().unwrap().id, c2);
        assert_eq!(get(&w, c2).unwrap().audience, Audience::World);
        assert_eq!(list(&w, Scope::Deleted, None, None).unwrap().len(), 1);
        assert_eq!(list(&w, Scope::World, None, Some(t)).unwrap().len(), 1);
        assert_eq!(list(&w, Scope::All, Some(&p("x")), None).unwrap().len(), 1);
        assert!(list(&w, Scope::Own, None, None).unwrap().is_empty());
    }

    #[test]
    fn due_commitments_until_delivered() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let f = write(
            &w,
            &NewFact::new(Actor::User, Audience::Participants(c), FactKind::Commitment, "call").due(10),
            0,
        )
        .unwrap()
        .id();
        assert!(due_commitments(&w, 9).unwrap().is_empty());
        assert_eq!(due_commitments(&w, 10).unwrap()[0].id, f);
        mark_delivered(&w, f).unwrap();
        assert!(due_commitments(&w, 99).unwrap().is_empty());
    }
}
