//! Wrap-up: store what the forced `wrapup` call extracted from a conversation. At most
//! `wrapup.max_facts` memories (a fact, or a commitment with a due time), at most one
//! hurt, and on a rollover a summary. Everything is deduplicated against every memory,
//! deleted ones included, and scoped to the channel's participants.

use rusqlite::Connection;

use crate::clock::{self, Now};
use crate::fact::{self, About, Audience, NewFact, Written};
use crate::params::{Trust, Wrapup as WrapupParams};
use crate::types::{Actor, ChannelId, FactId, FactKind, PersonId};
use crate::{Error, Result, bond, channel, trust};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Fact,
    Commitment,
}

/// One memory as the model wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    pub kind: MemoryKind,
    pub text: String,
    /// ISO 8601 date or date-time; required for a commitment.
    pub due: Option<String>,
}

/// A hurt: the user's words, quoted, aimed at one person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hurt {
    /// Needed in a group; in a direct channel it is the channel's person.
    pub person: Option<PersonId>,
    pub quote: String,
}

/// The `wrapup` tool's arguments, as the agent parsed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub facts: Vec<Memory>,
    pub hurt: Option<Hurt>,
    pub summary: Option<String>,
}

/// Why part of the arguments was not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// Beyond `wrapup.max_facts`.
    OverLimit { text: String },
    /// A commitment without a usable due time.
    NoDue { text: String },
    /// Repeats an existing memory (live or deleted).
    Duplicate { text: String, existing: FactId },
    Empty,
    /// The hurt names nobody who is here (or someone disabled).
    NoTarget { quote: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub facts: Vec<FactId>,
    pub hurt: Option<FactId>,
    /// The summary for the next segment's opening block. It inherits the channel's
    /// audience: only this channel's next segment may carry it.
    pub summary: Option<Summary>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub channel: ChannelId,
    pub audience: Audience,
    pub text: String,
}

/// Store a wrap-up of `channel`. `author` is the person whose session ran it (the
/// channel's person in a direct channel; the last speaker in a group).
pub fn apply(
    conn: &Connection,
    channel: ChannelId,
    author: &Actor,
    args: &Args,
    now: Now,
    params: &WrapupParams,
    trust_params: &Trust,
) -> Result<Outcome> {
    let ch = channel::get(conn, channel)?;
    let audience = fact::channel_audience(channel);
    let mut out = Outcome {
        facts: Vec::new(),
        hurt: None,
        summary: None,
        skipped: Vec::new(),
    };

    for m in &args.facts {
        let text = m.text.trim();
        if fact::normalize(text).is_empty() {
            out.skipped.push(Skipped::Empty);
            continue;
        }
        if out.facts.len() as u32 >= params.max_facts {
            out.skipped.push(Skipped::OverLimit { text: text.into() });
            continue;
        }
        let new = match m.kind {
            MemoryKind::Fact => NewFact::new(author.clone(), audience.clone(), FactKind::Fact, text).about(About::User),
            MemoryKind::Commitment => {
                let Some(due) = m.due.as_deref().and_then(|d| clock::parse_due(d, now)) else {
                    out.skipped.push(Skipped::NoDue { text: text.into() });
                    continue;
                };
                NewFact::new(author.clone(), audience.clone(), FactKind::Commitment, text).due(due)
            }
        };
        match fact::write(conn, &new, now.ms)? {
            Written::New(id) => out.facts.push(id),
            Written::Duplicate { existing, .. } => out.skipped.push(Skipped::Duplicate {
                text: text.into(),
                existing,
            }),
        }
    }

    if let Some(h) = &args.hurt {
        let quote = h.quote.trim();
        let target = h.person.clone().or_else(|| ch.person().cloned());
        let valid = match &target {
            Some(p) => ch.has(&Actor::Person(p.clone())) && !bond::is_disabled(conn, p)?,
            None => false,
        };
        if fact::normalize(quote).is_empty() {
            out.skipped.push(Skipped::Empty);
        } else if !valid {
            out.skipped.push(Skipped::NoTarget { quote: quote.into() });
        } else {
            let person = target.expect("valid");
            let new = NewFact::new(Actor::User, audience.clone(), FactKind::Hurt, quote).about(About::Person(person));
            match fact::write(conn, &new, now.ms)? {
                Written::New(id) => {
                    trust::take_hurt(conn, &fact::get(conn, id)?, trust_params)?;
                    out.hurt = Some(id);
                }
                Written::Duplicate { existing, .. } => out.skipped.push(Skipped::Duplicate {
                    text: quote.into(),
                    existing,
                }),
            }
        }
    }

    out.summary = args
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|text| Summary {
            channel,
            audience,
            text: text.into(),
        });
    Ok(out)
}

/// The summary may open a new segment of `channel` only. Any other use is a leak.
pub fn check_summary_target(summary: &Summary, channel: ChannelId) -> Result<()> {
    if summary.channel != channel {
        return Err(Error::Invalid(format!(
            "a summary of channel {} cannot open channel {channel}",
            summary.channel
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;
    use crate::types::{Mode, Origin};

    fn mem(kind: MemoryKind, text: &str, due: Option<&str>) -> Memory {
        Memory {
            kind,
            text: text.into(),
            due: due.map(Into::into),
        }
    }

    #[test]
    fn stores_at_most_max_facts_with_due_and_channel_audience() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let args = Args {
            facts: vec![
                mem(MemoryKind::Commitment, "no time", None),
                mem(MemoryKind::Commitment, "call on friday", Some("2026-03-13T18:00")),
                mem(MemoryKind::Fact, "likes rain", None),
                mem(MemoryKind::Fact, "third", None),
            ],
            hurt: None,
            summary: Some("  they talked  ".into()),
        };
        let out = apply(&w, c, &Actor::Person(p("a")), &args, at(T0), &WrapupParams::default(), &Trust::default()).unwrap();
        assert_eq!(out.facts.len(), 2);
        assert_eq!(
            out.skipped,
            vec![Skipped::NoDue { text: "no time".into() }, Skipped::OverLimit { text: "third".into() }]
        );
        let commitment = fact::get(&w, out.facts[0]).unwrap();
        assert_eq!(commitment.kind, FactKind::Commitment);
        assert!(commitment.due.is_some());
        assert_eq!(commitment.audience, Audience::Participants(c));
        assert_eq!(fact::get(&w, out.facts[1]).unwrap().about, Some(About::User));
        let s = out.summary.unwrap();
        assert_eq!((s.text.as_str(), s.audience.clone()), ("they talked", Audience::Participants(c)));
        check_summary_target(&s, c).unwrap();
        assert!(check_summary_target(&s, ChannelId(c.0 + 1)).is_err());
    }

    #[test]
    fn dedupes_against_deleted_memories() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let args = Args {
            facts: vec![mem(MemoryKind::Fact, "likes rain", None)],
            ..Args::default()
        };
        let author = Actor::Person(p("a"));
        let first = apply(&w, c, &author, &args, at(T0), &WrapupParams::default(), &Trust::default()).unwrap();
        fact::retract(&w, first.facts[0]).unwrap();
        let again = apply(&w, c, &author, &args, at(T0), &WrapupParams::default(), &Trust::default()).unwrap();
        assert!(again.facts.is_empty());
        assert!(matches!(again.skipped[0], Skipped::Duplicate { .. }));
    }

    #[test]
    fn one_hurt_aimed_at_someone_present() {
        let w = world();
        let g = channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
        let author = Actor::Person(p("a"));
        let hurt = |person: Option<&str>, quote: &str| Args {
            hurt: Some(Hurt {
                person: person.map(p),
                quote: quote.into(),
            }),
            ..Args::default()
        };
        let params = (WrapupParams::default(), Trust::default());
        let out = apply(&w, g, &author, &hurt(None, "you are useless"), at(T0), &params.0, &params.1).unwrap();
        assert!(matches!(out.skipped[0], Skipped::NoTarget { .. }), "a group needs a person");
        let out = apply(&w, g, &author, &hurt(Some("zz"), "x"), at(T0), &params.0, &params.1).unwrap();
        assert!(matches!(out.skipped[0], Skipped::NoTarget { .. }));
        let out = apply(&w, g, &author, &hurt(Some("b"), "you are useless"), at(T0), &params.0, &params.1).unwrap();
        let h = fact::get(&w, out.hurt.unwrap()).unwrap();
        assert_eq!((h.kind, h.author, h.about), (FactKind::Hurt, Actor::User, Some(About::Person(p("b")))));
        assert_eq!(bond::user_bond(&w, &p("b")).unwrap().trust, 85);
        fact::retract(&w, h.id).unwrap();
        assert_eq!(bond::user_bond(&w, &p("b")).unwrap().trust, 100);
        // In a direct channel the target is its person.
        let c = channel::direct(&w, &p("c")).unwrap();
        let out = apply(&w, c, &Actor::Person(p("c")), &hurt(None, "cruel"), at(T0), &params.0, &params.1).unwrap();
        assert!(out.hurt.is_some());
        bond::set_mode(&w, &p("c"), Mode::Disabled).unwrap();
        let out = apply(&w, c, &Actor::Person(p("c")), &hurt(None, "again"), at(T0), &params.0, &params.1).unwrap();
        assert!(out.hurt.is_none());
    }
}
