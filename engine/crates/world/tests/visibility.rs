//! The four places a memory could leak to someone who was not there. Two are built in
//! `world` (the candidate set, the shared-log material); for the other two (filling
//! `as_person`, the rollover summary) `world` provides the primitive the agent uses, and
//! that primitive is what is tested here.

use world::fact::{self, Audience, NewFact};
use world::log;
use world::wrapup::{self, Args};
use world::{Actor, ChannelId, FactKind, Origin, PersonId, World, channel, params};

fn p(s: &str) -> PersonId {
    PersonId::from(s)
}

/// a and b share a group; a has a private observation and a direct-channel memory; c is
/// elsewhere. Returns (group, a's direct channel, the facts by name).
struct Scene {
    w: World,
    group: ChannelId,
    direct_a: ChannelId,
    world_fact: world::FactId,
    group_fact: world::FactId,
    own_a: world::FactId,
    direct_fact: world::FactId,
}

fn scene() -> Scene {
    let w = World::in_memory().unwrap();
    let group = channel::create_group(&w, &[p("a"), p("b")], None, Origin::User).unwrap();
    let direct_a = channel::direct(&w, &p("a")).unwrap();
    let write = |aud: Audience, text: &str| {
        fact::write(&w, &NewFact::new(Actor::User, aud, FactKind::Fact, text), 1)
            .unwrap()
            .id()
    };
    let world_fact = write(Audience::World, "a conclusion");
    let group_fact = write(Audience::Participants(group), "said in the group");
    let own_a = write(Audience::Own(p("a")), "a noticed this");
    let direct_fact = write(Audience::Participants(direct_a), "said to a alone");
    Scene {
        w,
        group,
        direct_a,
        world_fact,
        group_fact,
        own_a,
        direct_fact,
    }
}

fn ids(facts: Vec<fact::Fact>) -> Vec<world::FactId> {
    facts.into_iter().map(|f| f.id).collect()
}

/// Leak point 1: candidate construction. The set is filtered before ranking; c, who was
/// never there, gets only what the world knows, and a deleted memory reaches nobody.
#[test]
fn candidate_set_is_filtered_by_audience() {
    let s = scene();
    let c_dm = channel::direct(&s.w, &p("c")).unwrap();
    assert_eq!(
        ids(fact::visible_to(&s.w, Some(&p("c")), c_dm).unwrap()),
        [s.world_fact]
    );
    assert_eq!(
        ids(fact::visible_to(&s.w, Some(&p("a")), s.direct_a).unwrap()),
        [s.world_fact, s.group_fact, s.own_a, s.direct_fact]
    );
    fact::retract(&s.w, s.group_fact).unwrap();
    assert_eq!(
        ids(fact::visible_to(&s.w, Some(&p("b")), s.group).unwrap()),
        [s.world_fact]
    );
    // The maintainer face has no memories at all.
    assert!(fact::visible_to(&s.w, None, s.group).unwrap().is_empty());
}

/// Leak point 2: `as_person` is filled by the harness from who is speaking; a person
/// outside the channel cannot be made to search as if he were in it, and the host never
/// searches as a person.
#[test]
fn as_person_comes_from_the_speaker() {
    let s = scene();
    assert_eq!(
        channel::as_person(&s.w, s.group, &Actor::Person(p("b"))).unwrap(),
        Some(p("b"))
    );
    assert_eq!(
        channel::as_person(&s.w, s.group, &Actor::Host).unwrap(),
        None
    );
    assert!(channel::as_person(&s.w, s.group, &Actor::Person(p("c"))).is_err());
    assert!(fact::visible_to(&s.w, Some(&p("c")), s.group).is_err());
}

/// Leak point 3: shared-log material. In the group, even a's own retrieval yields only
/// what both a and b may recall: his private observation and his direct-channel memory
/// never enter the log b reads.
#[test]
fn shared_log_carries_only_shared_material() {
    let s = scene();
    let shared = ids(fact::shared_visible(&s.w, s.group).unwrap());
    assert_eq!(shared, [s.world_fact, s.group_fact]);
    assert_eq!(
        ids(fact::visible_to(&s.w, Some(&p("a")), s.group).unwrap()),
        shared
    );
    assert_eq!(
        ids(log::opening_memories(&s.w, s.group, Some(&p("a"))).unwrap()),
        shared
    );
    // A direct channel with a colleague pulled in is shared too.
    channel::add_participant(&s.w, s.direct_a, &Actor::Person(p("c"))).unwrap();
    let shared_dm = ids(fact::shared_visible(&s.w, s.direct_a).unwrap());
    assert_eq!(shared_dm, [s.world_fact, s.direct_fact]);
    assert!(!shared_dm.contains(&s.own_a));
}

/// Leak point 4: rollover summary inheritance. The summary carries its channel's
/// audience and can only open that channel's next segment.
#[test]
fn rollover_summary_inherits_the_channel_audience() {
    let s = scene();
    let out = wrapup::apply(
        &s.w,
        s.direct_a,
        &Actor::Person(p("a")),
        &Args {
            summary: Some("what a and the user said".into()),
            ..Args::default()
        },
        world::clock::Now::utc(1),
        &params::Wrapup::default(),
        &params::Trust::default(),
    )
    .unwrap();
    let summary = out.summary.unwrap();
    assert_eq!(summary.audience, Audience::Participants(s.direct_a));
    assert!(log::open_segment(&s.w, s.group, [b"A", b"B", b"C"], "{}", Some(&summary), 2).is_err());
    log::open_segment(
        &s.w,
        s.direct_a,
        [b"A", b"B", b"C"],
        "{}",
        Some(&summary),
        2,
    )
    .unwrap();
    // Wrap-up memories are scoped the same way: b cannot recall them.
    let out = wrapup::apply(
        &s.w,
        s.direct_a,
        &Actor::Person(p("a")),
        &Args {
            facts: vec![wrapup::Memory {
                kind: wrapup::MemoryKind::Fact,
                text: "the user likes rain".into(),
                due: None,
            }],
            ..Args::default()
        },
        world::clock::Now::utc(3),
        &params::Wrapup::default(),
        &params::Trust::default(),
    )
    .unwrap();
    assert!(!fact::can_see(&s.w, &p("b"), out.facts[0]).unwrap());
    assert!(fact::can_see(&s.w, &p("a"), out.facts[0]).unwrap());
}
