//! Property tests: the delegation-tree invariant, the seed caps, the quota bands.

use proptest::prelude::*;

use world::clock::Now;
use world::fact::{About, Audience, NewFact};
use world::params::{self, Ask, Quota};
use world::quota::{self, Band, CallKind, Decision, Usage};
use world::seed::{self, Limits, NoTopic, Occasion, Planned};
use world::task::{self, Harness};
use world::{Actor, FactKind, Mode, PersonId, Shape, Tier, World, bond, channel, message};

const T0: i64 = 1_773_144_000_000; // 2026-03-10 12:00 UTC
const HOUR: i64 = 3_600_000;

fn p(i: usize) -> PersonId {
    PersonId(format!("p{i}"))
}

#[derive(Debug, Clone)]
enum Op {
    /// Participant `from` (index into who is on the tree) pulls in a new colleague.
    Join { from: usize, turns: u32 },
    /// Participant spends one turn.
    Spend { who: usize },
    /// Participant reports.
    Report { who: usize },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..8, 0u32..6).prop_map(|(from, turns)| Op::Join { from, turns }),
        (0usize..8).prop_map(|who| Op::Spend { who }),
        (0usize..8).prop_map(|who| Op::Report { who }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever sequence of joins, spends and reports happens, the tree never holds more
    /// turns than the root was given, and a refused split changes nothing.
    #[test]
    fn delegation_tree_never_exceeds_its_grant(grant in 1u32..30, ops in prop::collection::vec(op(), 0..40)) {
        let w = World::in_memory().unwrap();
        let office = std::env::temp_dir().join(format!("world-prop-{}", std::process::id()));
        let root = task::ask(&w, &p(0), "q", Some(grant), None, Tier::Middle, &Ask::default(), T0).unwrap();
        let mut on: Vec<(PersonId, world::TaskId)> = vec![(p(0), root.task)];
        let mut next = 1;
        for op in ops {
            let before = task::tree_turns_left(&w, root.task).unwrap();
            match op {
                Op::Join { from, turns } => {
                    let (caller, _) = on[from % on.len()].clone();
                    let res = task::request_join(&w, root.channel, &caller, &p(next), Some(turns), T0);
                    match res {
                        Ok(j) => {
                            // A caller whose request is done pulls someone in outside
                            // any request: he answers once and the tree is untouched.
                            if let Some(t) = j.task {
                                on.push((p(next), t));
                            }
                            next += 1;
                            prop_assert_eq!(task::tree_turns_left(&w, root.task).unwrap(), before);
                        }
                        Err(_) => prop_assert_eq!(task::tree_turns_left(&w, root.task).unwrap(), before),
                    }
                }
                Op::Spend { who } => {
                    let (_, t) = on[who % on.len()].clone();
                    let h = task::spend_turn(&w, t).unwrap();
                    let left = task::get(&w, t).unwrap().turns_left;
                    prop_assert_eq!(h, task::harness(left));
                    if h == Harness::ForceClose {
                        prop_assert_eq!(left, 0);
                    }
                }
                Op::Report { who } => {
                    let (person, t) = on[who % on.len()].clone();
                    let _ = task::report(&w, &person, t, "r", &[], None, &office, T0);
                }
            }
            let sum = task::tree_turns_left(&w, root.task).unwrap();
            prop_assert!(sum <= grant, "sum {} > grant {}", sum, grant);
            prop_assert_eq!(task::tree_budget(&w, root.task).unwrap(), grant);
        }
    }

    /// Over any run of presence occasions, no one opens more than `seed.per_agent_day`
    /// times a local day, nobody more than `seed.daily_total` together, nothing opens in
    /// quiet hours or while the user is talking, and only enabled people open.
    #[test]
    fn seed_caps_hold(
        n_people in 1usize..6,
        enabled_mask in 0u32..64,
        per_agent in 0u32..3,
        total in 0u32..5,
        steps in prop::collection::vec((0i64..6 * HOUR, any::<bool>()), 1..30),
    ) {
        let w = World::in_memory().unwrap();
        let mut seed_p = params::Seed { per_agent_day: per_agent, daily_total: total, ..params::Seed::default() };
        seed_p.quiet_hours = params::QuietHours { from: "23:00".into(), to: "08:00".into() };
        let session_p = params::Session::default();
        let status = quota::status(&w, Tier::Middle, T0, &Quota::default()).unwrap();
        for i in 0..n_people {
            if enabled_mask & (1 << i) != 0 {
                bond::set_mode(&w, &p(i), Mode::Enabled).unwrap();
            }
            // Plenty of seeds: several memories about the user each person can see.
            let c = channel::direct(&w, &p(i)).unwrap();
            for k in 0..4 {
                let f = NewFact::new(Actor::Person(p(i)), Audience::Participants(c), FactKind::Fact, format!("m{i}-{k}"))
                    .about(About::User);
                world::fact::write(&w, &f, 0).unwrap();
            }
        }
        let other = channel::direct(&w, &PersonId::from("talker")).unwrap();
        let mut now = T0;
        let mut opened: Vec<(PersonId, i64)> = Vec::new();
        for (gap, user_talks) in steps {
            now += gap;
            if user_talks {
                message::append(&w, other, &Actor::User, "hey", None, None, now).unwrap();
            }
            let at = Now::utc(now);
            let limits = Limits { seed: &seed_p, session: &session_p, quota: &status };
            if let Planned::Open(o) = seed::plan(&w, &Occasion::Presence { words: &[] }, at, &limits, &NoTopic).unwrap() {
                prop_assert!(!world::clock::in_quiet_hours(&seed_p.quiet_hours, at));
                prop_assert!(!seed::in_conversation(&w, now, &session_p).unwrap());
                prop_assert_eq!(bond::mode(&w, &o.person).unwrap(), Mode::Enabled);
                let m = message::append(&w, o.channel, &Actor::Person(o.person.clone()), "hi", None, None, now).unwrap();
                seed::delivered(&w, &o, m, now).unwrap();
                // The opening itself is not the user talking; read it so it is not unread.
                channel::mark_read(&w, o.channel, m).unwrap();
                opened.push((o.person, at.local_day_start()));
            }
        }
        let mut days: std::collections::BTreeMap<i64, Vec<PersonId>> = Default::default();
        for (person, day) in opened {
            days.entry(day).or_default().push(person);
        }
        for people in days.values() {
            prop_assert!(people.len() as u32 <= total);
            for x in people {
                prop_assert!(people.iter().filter(|y| *y == x).count() as u32 <= per_agent);
            }
        }
    }

    /// The band is a function of the tighter window's use; user-initiated calls pass
    /// until u reaches 1; unprompted calls stop at `quota.quiet_at`; the tier without
    /// windows never blocks.
    #[test]
    fn quota_three_bands(
        quiet_at in 0.05f64..0.99,
        spends in prop::collection::vec((0i64..10 * HOUR, 0u64..400), 0..30),
        probe in 0i64..12 * HOUR,
    ) {
        let w = World::in_memory().unwrap();
        let mut q = Quota { quiet_at, ..Quota::default() };
        q.window_5h.low = 1000.0;
        q.window_7d.low = 3000.0;
        let cost = params::Cost::default();
        // Only the past: release times are checked later, with nothing recorded after now.
        let spends: Vec<_> = spends.into_iter().filter(|(at, _)| *at <= probe).collect();
        for (at, n) in &spends {
            quota::record_usage(&w, T0 + at, Shape::Direct, None, "m", Usage { uncached: *n, ..Usage::default() }, None, &cost).unwrap();
        }
        let now = T0 + probe;
        let st = quota::status(&w, Tier::Low, now, &q).unwrap();
        let sum = |span: i64| -> f64 {
            spends.iter().filter(|(at, _)| T0 + at > now - span && T0 + at <= now).map(|(_, n)| *n as f64).sum()
        };
        let u = (sum(quota::SPAN_5H_MS) / 1000.0).max(sum(quota::SPAN_7D_MS) / 3000.0);
        prop_assert!((st.u - u).abs() < 1e-9);
        let expected = if u >= 1.0 { Band::Exhausted } else if u >= quiet_at { Band::Quiet } else { Band::Open };
        prop_assert_eq!(st.band, expected);
        let user = quota::decide(&st, CallKind::UserInitiated);
        prop_assert_eq!(user.allowed(), u < 1.0);
        for kind in [CallKind::Opening, CallKind::Interjection, CallKind::HostLine] {
            prop_assert_eq!(quota::decide(&st, kind).allowed(), u < quiet_at);
        }
        if let Decision::Exhausted { release_at } = user {
            // Once released, the band is better than exhausted.
            let r = release_at.expect("exhausted has a release time");
            prop_assert!(r > now);
            prop_assert_ne!(quota::status(&w, Tier::Low, r, &q).unwrap().band, Band::Exhausted);
        }
        if st.band == Band::Quiet {
            let r = st.release_at.expect("quiet has a release time");
            prop_assert_eq!(quota::status(&w, Tier::Low, r, &q).unwrap().band, Band::Open);
        }
        let ultra = quota::status(&w, Tier::Ultra, now, &q).unwrap();
        prop_assert_eq!(ultra.band, Band::Open);
        prop_assert!(ultra.windows.is_empty());
    }
}
