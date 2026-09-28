//! Who may have a session at all: a person with every persona slot in the corpus, or the
//! host with its in-world layer. Everyone else is treated like a disabled person: never
//! addressed, asked, pulled in, offered or opened with.

use std::collections::BTreeSet;

use folio::Folio;

use crate::error::Result;

/// The slots a person's persona is made of.
pub const PERSON_SLOTS: [&str; 4] = ["system", "tone", "fallback", "capability"];

/// The subject the host's prompts are stored under.
pub const HOST: &str = "host";

/// The persona holders of one corpus, read once when it is opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Personas {
    persons: BTreeSet<String>,
    host: bool,
}

impl Personas {
    pub fn load(folio: &Folio) -> Result<Self> {
        let mut stmt = folio.conn().prepare(
            "SELECT subject FROM prompts WHERE slot IN ('system', 'tone', 'fallback', 'capability')
               AND subject <> 'host' AND trim(body) <> ''
             GROUP BY subject HAVING count(DISTINCT slot) = 4 ORDER BY subject",
        )?;
        let persons = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        let host = folio
            .prompt(HOST, "in_world")?
            .is_some_and(|b| !b.trim().is_empty());
        Ok(Self { persons, host })
    }

    /// Whether this person may be given a session.
    pub fn has_persona(&self, person: &str) -> bool {
        self.persons.contains(person)
    }

    pub fn host(&self) -> bool {
        self.host
    }

    /// Every person who may have a session, by id.
    pub fn persons(&self) -> impl Iterator<Item = &str> {
        self.persons.iter().map(String::as_str)
    }

    /// `persons` without those who have no persona, order kept.
    pub fn filter(&self, persons: Vec<world::PersonId>) -> Vec<world::PersonId> {
        persons
            .into_iter()
            .filter(|p| self.has_persona(p.as_str()))
            .collect()
    }
}

/// One person's persona text as the corpus stores it (placeholders not yet replaced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Persona {
    pub system: String,
    pub tone: String,
    pub fallback: String,
}

pub fn person(folio: &Folio, person: &str) -> Result<Option<Persona>> {
    let get = |slot| folio.prompt(person, slot);
    Ok(match (get("system")?, get("tone")?, get("fallback")?) {
        (Some(system), Some(tone), Some(fallback)) => Some(Persona {
            system,
            tone,
            fallback,
        }),
        _ => None,
    })
}

/// The host's two layers. `meta` is only ever read into the host's own session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLayers {
    pub in_world: String,
    pub meta: String,
}

pub fn host(folio: &Folio) -> Result<HostLayers> {
    Ok(HostLayers {
        in_world: folio.prompt(HOST, "in_world")?.unwrap_or_default(),
        meta: folio.prompt(HOST, "meta")?.unwrap_or_default(),
    })
}
