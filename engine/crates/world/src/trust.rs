//! Trust: grows with shared turns and finished requests (less above 100), drops only by
//! a hurt, never with absence. Only its tone band reaches a session.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::bond::{self, Bond};
use crate::fact::{Audience, Fact};
use crate::params::Trust;
use crate::types::{Actor, ChannelId, ChannelKind, PersonId};
use crate::{Error, Result, channel};

/// The one trust signal a session sees, in its opening block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Guarded,
    Normal,
    Close,
    Deep,
}

impl Tone {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Guarded => "guarded",
            Self::Normal => "normal",
            Self::Close => "close",
            Self::Deep => "deep",
        }
    }
}

/// `trust.bands` are the lower bounds of normal, close and deep.
pub fn tone(trust: u8, params: &Trust) -> Tone {
    let [normal, close, deep] = params.bands;
    if trust >= deep {
        Tone::Deep
    } else if trust >= close {
        Tone::Close
    } else if trust >= normal {
        Tone::Normal
    } else {
        Tone::Guarded
    }
}

/// The diminishing factor: full below 100, shrinking linearly to nothing at 200.
pub fn f(trust: f64) -> f64 {
    if trust < 100.0 {
        1.0
    } else {
        ((200.0 - trust) / 100.0).max(0.0)
    }
}

/// One growth step from `trust` with gain `g`.
pub fn grow(trust: f64, g: f64) -> f64 {
    (trust + g * f(trust)).clamp(0.0, 200.0)
}

fn apply(conn: &Connection, b: &Bond, exact: f64) -> Result<()> {
    bond::store_trust(conn, &b.a, &b.b, exact)
}

/// A shared turn: a direct-channel turn with the user, or a person answering in a group.
/// In a group every other person there who has spoken becomes (or stays) an acquaintance
/// of the speaker: the pair bond is created at 0 on the first shared turn and grows like
/// any other.
pub fn shared_turn(
    conn: &Connection,
    channel: ChannelId,
    speaker: &PersonId,
    params: &Trust,
) -> Result<()> {
    let ch = channel::get(conn, channel)?;
    if !ch.has(&Actor::Person(speaker.clone())) {
        return Err(Error::NotParticipant {
            person: speaker.clone(),
            channel,
        });
    }
    bond::ensure_user_bond(conn, speaker)?;
    let b = bond::user_bond(conn, speaker)?;
    apply(conn, &b, grow(b.trust_exact, params.g1))?;
    if ch.kind == ChannelKind::Group || ch.persons().len() > 1 {
        let spoke: Vec<PersonId> = {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT author FROM message WHERE channel = ?1
                 AND author NOT IN ('user', 'host') AND author <> ?2",
            )?;
            let rows = stmt.query_map((channel, speaker), |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let members = ch.persons();
        for other in spoke.into_iter().filter(|o| members.contains(o)) {
            let existed = bond::pair_bond(conn, speaker, &other)?.is_some();
            bond::ensure_pair_bond(conn, speaker, &other)?;
            if existed {
                let pb = bond::pair_bond(conn, speaker, &other)?.expect("just ensured");
                apply(conn, &pb, grow(pb.trust_exact, params.g1))?;
            }
        }
    }
    Ok(())
}

/// A request the user gave this person is done.
pub fn task_done(conn: &Connection, person: &PersonId, params: &Trust) -> Result<()> {
    bond::ensure_user_bond(conn, person)?;
    let b = bond::user_bond(conn, person)?;
    apply(conn, &b, grow(b.trust_exact, params.g2))
}

/// The person a hurt was aimed at: its subject, else the owner of the direct channel
/// it was said in.
fn hurt_target(conn: &Connection, fact: &Fact) -> Result<Option<PersonId>> {
    if let Some(crate::fact::About::Person(p)) = &fact.about {
        return Ok(Some(p.clone()));
    }
    Ok(match &fact.audience {
        Audience::Participants(c) => channel::get(conn, *c)?.person().cloned(),
        Audience::Own(p) => Some(p.clone()),
        Audience::World => None,
    })
}

/// Apply a newly written hurt: take `trust.h`, clamped at 0, and remember exactly what
/// was taken on the fact so a delete can give it back.
pub(crate) fn take_hurt(conn: &Connection, fact: &Fact, params: &Trust) -> Result<()> {
    let Some(p) = hurt_target(conn, fact)? else {
        return Ok(());
    };
    bond::ensure_user_bond(conn, &p)?;
    let b = bond::user_bond(conn, &p)?;
    let after = (b.trust_exact - params.h).max(0.0);
    apply(conn, &b, after)?;
    conn.execute(
        "UPDATE fact SET trust_delta = ?2 WHERE id = ?1",
        (fact.id, after - b.trust_exact),
    )?;
    Ok(())
}

fn delta(conn: &Connection, fact: &Fact) -> Result<f64> {
    Ok(conn
        .query_row(
            "SELECT trust_delta FROM fact WHERE id = ?1",
            [fact.id],
            |r| r.get::<_, Option<f64>>(0),
        )?
        .unwrap_or(0.0))
}

/// A hurt was deleted: give back what it took.
pub(crate) fn undo_hurt(conn: &Connection, fact: &Fact) -> Result<()> {
    let Some(p) = hurt_target(conn, fact)? else {
        return Ok(());
    };
    let b = bond::user_bond(conn, &p)?;
    apply(conn, &b, b.trust_exact - delta(conn, fact)?)
}

/// A deleted hurt was restored: take it again (clamped at 0, recorded afresh).
pub(crate) fn redo_hurt(conn: &Connection, fact: &Fact) -> Result<()> {
    let Some(p) = hurt_target(conn, fact)? else {
        return Ok(());
    };
    let b = bond::user_bond(conn, &p)?;
    let taken = -delta(conn, fact)?;
    let after = (b.trust_exact - taken).max(0.0);
    apply(conn, &b, after)?;
    conn.execute(
        "UPDATE fact SET trust_delta = ?2 WHERE id = ?1",
        (fact.id, after - b.trust_exact),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{self, About, NewFact};
    use crate::message;
    use crate::testing::*;
    use crate::types::{FactKind, Origin};

    #[test]
    fn bands() {
        let t = Trust::default();
        assert_eq!(tone(0, &t), Tone::Guarded);
        assert_eq!(tone(59, &t), Tone::Guarded);
        assert_eq!(tone(60, &t), Tone::Normal);
        assert_eq!(tone(100, &t), Tone::Close);
        assert_eq!(tone(150, &t), Tone::Deep);
        assert_eq!(tone(200, &t), Tone::Deep);
    }

    #[test]
    fn growth_diminishes_above_100_and_never_passes_200() {
        assert_eq!(grow(50.0, 2.0), 52.0);
        assert_eq!(grow(100.0, 2.0), 102.0);
        assert_eq!(grow(150.0, 2.0), 151.0);
        assert_eq!(grow(200.0, 2.0), 200.0);
        let mut t = 100.0;
        for _ in 0..100_000 {
            t = grow(t, 2.0);
        }
        assert!(t <= 200.0 && t > 199.0);
    }

    #[test]
    fn shared_turns_accumulate_fractions() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let params = Trust::default();
        for _ in 0..4 {
            shared_turn(&w, c, &p("a"), &params).unwrap();
        }
        let b = bond::user_bond(&w, &p("a")).unwrap();
        // 0.5 per turn with f shrinking a little above 100.
        assert!((b.trust_exact - 101.99).abs() < 0.01, "{}", b.trust_exact);
        assert_eq!(b.trust, 102);
        task_done(&w, &p("a"), &params).unwrap();
        assert!(bond::user_bond(&w, &p("a")).unwrap().trust_exact > 103.9);
    }

    #[test]
    fn pair_bonds_start_at_zero_after_a_shared_turn() {
        let w = world();
        let params = Trust::default();
        let g = channel::create_group(&w, &[p("a"), p("b"), p("c")], None, Origin::User).unwrap();
        // Being in the same room is not a shared turn.
        shared_turn(&w, g, &p("a"), &params).unwrap();
        assert_eq!(bond::pair_bond(&w, &p("a"), &p("b")).unwrap(), None);
        message::append(&w, g, &Actor::Person(p("a")), "x", None, None, 1).unwrap();
        shared_turn(&w, g, &p("b"), &params).unwrap();
        let pb = bond::pair_bond(&w, &p("a"), &p("b")).unwrap().unwrap();
        assert_eq!(pb.trust, 0, "starts at 0");
        assert_eq!(bond::pair_bond(&w, &p("a"), &p("c")).unwrap(), None);
        shared_turn(&w, g, &p("b"), &params).unwrap();
        assert_eq!(
            bond::pair_bond(&w, &p("a"), &p("b"))
                .unwrap()
                .unwrap()
                .trust_exact,
            0.5
        );
    }

    #[test]
    fn hurt_takes_and_retraction_gives_back_exactly() {
        let w = world();
        let params = Trust {
            h: 15.0,
            ..Trust::default()
        };
        let c = channel::direct(&w, &p("a")).unwrap();
        let hurt = |text: &str| {
            let f = fact::write(
                &w,
                &NewFact::new(Actor::User, Audience::Participants(c), FactKind::Hurt, text)
                    .about(About::Person(p("a"))),
                1,
            )
            .unwrap()
            .id();
            take_hurt(&w, &fact::get(&w, f).unwrap(), &params).unwrap();
            f
        };
        let trust = || bond::user_bond(&w, &p("a")).unwrap().trust_exact;
        let h1 = hurt("one");
        assert_eq!(trust(), 85.0);
        for i in 0..6 {
            hurt(&format!("more {i}"));
        }
        assert_eq!(trust(), 0.0, "clamped at 0");
        fact::retract(&w, h1).unwrap();
        assert_eq!(trust(), 15.0, "gives back what it took");
        fact::restore(&w, h1).unwrap();
        assert_eq!(trust(), 0.0);
        fact::retract(&w, h1).unwrap();
        fact::retract(&w, h1).unwrap();
        assert_eq!(trust(), 15.0, "retracting twice gives back once");
    }
}
