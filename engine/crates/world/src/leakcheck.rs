//! The four places a memory could leak to someone who was not there, as checks that run on
//! a synthetic world. Two are built in `world` (the candidate set, the shared-log
//! material); for the other two (filling `as_person`, the rollover summary) `world`
//! provides the primitive the agent uses, and that primitive is what is checked here.
//!
//! One definition serves the test suite and the maintainer CLI's count, so the number
//! reported is the number the tests enforce.

use std::fmt::Debug;

use crate::fact::{self, Audience, NewFact};
use crate::wrapup::{self, Args};
use crate::{Actor, ChannelId, FactId, FactKind, Origin, PersonId, World, channel, log, params};

/// One leak point and how it fared: `failure` is `None` when it held.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub point: &'static str,
    pub failure: Option<String>,
}

type Check = fn(&Scene) -> Result<(), String>;

/// Every leak point, run on a fresh synthetic world each.
pub fn run() -> Vec<Outcome> {
    let points: [(&'static str, Check); 4] = [
        ("candidate_set_is_filtered_by_audience", candidate_set),
        ("as_person_comes_from_the_speaker", as_person),
        ("shared_log_carries_only_shared_material", shared_log),
        (
            "rollover_summary_inherits_the_channel_audience",
            rollover_summary,
        ),
    ];
    points
        .into_iter()
        .map(|(point, check)| Outcome {
            point,
            failure: Scene::new().and_then(|s| check(&s)).err(),
        })
        .collect()
}

/// How many leak points failed.
pub fn leaks(outcomes: &[Outcome]) -> usize {
    outcomes.iter().filter(|o| o.failure.is_some()).count()
}

fn p(s: &str) -> PersonId {
    PersonId::from(s)
}

fn err(e: impl Debug) -> String {
    format!("{e:?}")
}

fn expect<T: PartialEq + Debug>(what: &str, got: T, want: T) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

fn require(what: &str, ok: bool) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

/// a and b share a group; a has a private observation and a direct-channel memory; c is
/// elsewhere.
struct Scene {
    w: World,
    group: ChannelId,
    direct_a: ChannelId,
    world_fact: FactId,
    group_fact: FactId,
    own_a: FactId,
    direct_fact: FactId,
}

impl Scene {
    fn new() -> Result<Self, String> {
        let w = World::in_memory().map_err(err)?;
        let group =
            channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).map_err(err)?;
        let direct_a = channel::direct(&w, &p("a")).map_err(err)?;
        let write = |aud: Audience, text: &str| {
            fact::write(&w, &NewFact::new(Actor::User, aud, FactKind::Fact, text), 1)
                .map(|f| f.id())
                .map_err(err)
        };
        let world_fact = write(Audience::World, "a conclusion")?;
        let group_fact = write(Audience::Participants(group), "said in the group")?;
        let own_a = write(Audience::Own(p("a")), "a noticed this")?;
        let direct_fact = write(Audience::Participants(direct_a), "said to a alone")?;
        Ok(Self {
            w,
            group,
            direct_a,
            world_fact,
            group_fact,
            own_a,
            direct_fact,
        })
    }

    fn visible(&self, who: Option<&PersonId>, ch: ChannelId) -> Result<Vec<FactId>, String> {
        let facts = fact::visible_to(&self.w, who, ch).map_err(err)?;
        Ok(facts.into_iter().map(|f| f.id).collect())
    }
}

fn ids(facts: Vec<fact::Fact>) -> Vec<FactId> {
    facts.into_iter().map(|f| f.id).collect()
}

/// Leak point 1: candidate construction. The set is filtered before ranking; c, who was
/// never there, gets only what the world knows, and a deleted memory reaches nobody.
fn candidate_set(s: &Scene) -> Result<(), String> {
    let c_dm = channel::direct(&s.w, &p("c")).map_err(err)?;
    expect(
        "c sees",
        s.visible(Some(&p("c")), c_dm)?,
        vec![s.world_fact],
    )?;
    expect(
        "a sees",
        s.visible(Some(&p("a")), s.direct_a)?,
        vec![s.world_fact, s.group_fact, s.own_a, s.direct_fact],
    )?;
    fact::retract(&s.w, s.group_fact).map_err(err)?;
    expect(
        "b sees after retraction",
        s.visible(Some(&p("b")), s.group)?,
        vec![s.world_fact],
    )?;
    // The maintainer face has no memories at all.
    expect("maintainer sees", s.visible(None, s.group)?, vec![])
}

/// Leak point 2: `as_person` is filled by the harness from who is speaking; a person
/// outside the channel cannot be made to search as if he were in it, and the host never
/// searches as a person.
fn as_person(s: &Scene) -> Result<(), String> {
    expect(
        "as_person for b",
        channel::as_person(&s.w, s.group, &Actor::Person(p("b"))).map_err(err)?,
        Some(p("b")),
    )?;
    expect(
        "as_person for the host",
        channel::as_person(&s.w, s.group, &Actor::Host).map_err(err)?,
        None,
    )?;
    require(
        "an outsider was allowed to speak as a person",
        channel::as_person(&s.w, s.group, &Actor::Person(p("c"))).is_err(),
    )?;
    require(
        "an outsider was allowed to search the group",
        fact::visible_to(&s.w, Some(&p("c")), s.group).is_err(),
    )
}

/// Leak point 3: shared-log material. In the group, even a's own retrieval yields only
/// what both a and b may recall: his private observation and his direct-channel memory
/// never enter the log b reads.
fn shared_log(s: &Scene) -> Result<(), String> {
    let shared = ids(fact::shared_visible(&s.w, s.group).map_err(err)?);
    expect(
        "shared in the group",
        shared.clone(),
        vec![s.world_fact, s.group_fact],
    )?;
    expect(
        "a's retrieval in the group",
        s.visible(Some(&p("a")), s.group)?,
        shared.clone(),
    )?;
    expect(
        "opening memories",
        ids(log::opening_memories(&s.w, s.group, Some(&p("a"))).map_err(err)?),
        shared,
    )?;
    // A direct channel with a colleague pulled in is shared too.
    channel::add_participant(&s.w, s.direct_a, &Actor::Person(p("c"))).map_err(err)?;
    let shared_dm = ids(fact::shared_visible(&s.w, s.direct_a).map_err(err)?);
    expect(
        "shared in the direct channel",
        shared_dm.clone(),
        vec![s.world_fact, s.direct_fact],
    )?;
    require(
        "a private observation reached the shared log",
        !shared_dm.contains(&s.own_a),
    )
}

/// Leak point 4: rollover summary inheritance. The summary carries its channel's
/// audience and can only open that channel's next segment.
fn rollover_summary(s: &Scene) -> Result<(), String> {
    let apply = |args: Args, at: i64| {
        wrapup::apply(
            &s.w,
            s.direct_a,
            &Actor::Person(p("a")),
            &args,
            crate::clock::Now::utc(at),
            &params::Wrapup::default(),
            &params::Trust::default(),
        )
        .map_err(err)
    };
    let summary = apply(
        Args {
            summary: Some("what a and the user said".into()),
            ..Args::default()
        },
        1,
    )?
    .summary
    .ok_or("no summary came back")?;
    expect(
        "summary audience",
        summary.audience.clone(),
        Audience::Participants(s.direct_a),
    )?;
    require(
        "the summary opened another channel's segment",
        log::open_segment(&s.w, s.group, [b"A", b"B", b"C"], "{}", Some(&summary), 2).is_err(),
    )?;
    log::open_segment(
        &s.w,
        s.direct_a,
        [b"A", b"B", b"C"],
        "{}",
        Some(&summary),
        2,
    )
    .map_err(err)?;
    // Wrap-up memories are scoped the same way: b cannot recall them.
    let out = apply(
        Args {
            facts: vec![wrapup::Memory {
                kind: wrapup::MemoryKind::Fact,
                text: "the user likes rain".into(),
                due: None,
            }],
            ..Args::default()
        },
        3,
    )?;
    let memory = out.facts[0];
    require(
        "b can recall a's wrap-up memory",
        !fact::can_see(&s.w, &p("b"), memory).map_err(err)?,
    )?;
    require(
        "a cannot recall his own wrap-up memory",
        fact::can_see(&s.w, &p("a"), memory).map_err(err)?,
    )
}
