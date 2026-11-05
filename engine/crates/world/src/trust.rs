//! Trust: grows with shared turns and, in bigger steps, with a talk that left something
//! worth remembering; both shrink towards the top of the range. It drops only by a hurt
//! (heavier for a closer person), never with absence. Only its tone band reaches a session.

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

/// The diminishing factor: full up to the start (100), then shrinking along a parabola to
/// nothing at 200, so the last stretch to the top is slow. Below the start growth is
/// full: a pair of colleagues that begins at 0, or a person who has been hurt, builds back
/// at the ordinary rate.
pub fn f(trust: f64) -> f64 {
    if trust <= 100.0 {
        1.0
    } else {
        let u = (trust - 100.0) / 100.0;
        (1.0 - u * u).max(0.0)
    }
}

/// One growth step from `trust` with gain `g`.
pub fn grow(trust: f64, g: f64) -> f64 {
    (trust + g * f(trust)).clamp(0.0, 200.0)
}

/// How much heavier or lighter a hurt weighs at this trust: half the base where nobody
/// has anything to lose, one and a half at the top, exactly the base at the start.
fn hurt_weight(trust: f64) -> f64 {
    0.5 + trust.clamp(0.0, 200.0) / 200.0
}

/// A talk that left a memory worth keeping (a promise, a change between the two): a step of
/// `trust.g2`, once per wrap-up whatever the number of memories. Rare, so it is large next
/// to a turn's.
pub fn moment(conn: &Connection, person: &PersonId, params: &Trust) -> Result<()> {
    bond::ensure_user_bond(conn, person)?;
    let b = bond::user_bond(conn, person)?;
    apply(conn, &b, grow(b.trust_exact, params.g2))
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
    let after = (b.trust_exact - params.h * hurt_weight(b.trust_exact)).max(0.0);
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
    fn growth_diminishes_towards_the_top_and_never_passes_200() {
        assert_eq!(f(100.0), 1.0);
        assert_eq!(
            f(0.0),
            1.0,
            "a fallen person is won back at the ordinary rate"
        );
        assert!(f(150.0) < f(120.0) && f(190.0) < f(150.0));
        assert_eq!(f(200.0), 0.0);
        assert_eq!(f(150.0), 0.75, "gentle at first, steep at the end");
        assert_eq!(grow(200.0, 2.0), 200.0);
        let mut t = 100.0;
        for _ in 0..100_000 {
            t = grow(t, 2.0);
        }
        assert!(t <= 200.0 && t > 199.0);
        // The same turn is worth less the closer to the top.
        assert!(grow(180.0, 1.0) - 180.0 < grow(120.0, 1.0) - 120.0);
    }

    #[test]
    fn a_moment_is_larger_than_a_turn_and_also_shrinks() {
        let w = world();
        let params = Trust::default();
        let c = channel::direct(&w, &p("a")).unwrap();
        shared_turn(&w, c, &p("a"), &params).unwrap();
        let after_turn = bond::user_bond(&w, &p("a")).unwrap().trust_exact;
        moment(&w, &p("a"), &params).unwrap();
        let after_moment = bond::user_bond(&w, &p("a")).unwrap().trust_exact;
        assert!(after_moment - after_turn > 5.0 * (after_turn - 100.0));
        for _ in 0..200 {
            moment(&w, &p("a"), &params).unwrap();
        }
        assert!(bond::user_bond(&w, &p("a")).unwrap().trust_exact < 200.0);
    }

    #[test]
    fn a_hurt_weighs_more_the_closer_the_person() {
        assert_eq!(hurt_weight(100.0), 1.0);
        assert!(hurt_weight(180.0) > hurt_weight(100.0));
        assert!(hurt_weight(20.0) < hurt_weight(100.0));
        assert_eq!(hurt_weight(0.0), 0.5);
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
        // 0.5 per turn with f shrinking a little away from 100.
        assert!((b.trust_exact - 102.0).abs() < 0.01, "{}", b.trust_exact);
        assert_eq!(b.trust, 102);
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
        assert_eq!(trust(), 85.0, "the base at the start");
        for i in 0..20 {
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
