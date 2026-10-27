//! The four places a memory could leak to someone who was not there. Two are built in
//! `world` (the candidate set, the shared-log material); for the other two (filling
//! `as_person`, the rollover summary) `world` provides the primitive the agent uses, and
//! that primitive is what is tested here. The checks live in `world::leakcheck`, which the
//! maintainer CLI also runs to report how many fail.

use world::leakcheck;

fn held(point: &str) {
    let outcome = leakcheck::run()
        .into_iter()
        .find(|o| o.point == point)
        .expect("leak point exists");
    assert_eq!(outcome.failure, None);
}

#[test]
fn candidate_set_is_filtered_by_audience() {
    held("candidate_set_is_filtered_by_audience");
}

#[test]
fn as_person_comes_from_the_speaker() {
    held("as_person_comes_from_the_speaker");
}

#[test]
fn shared_log_carries_only_shared_material() {
    held("shared_log_carries_only_shared_material");
}

#[test]
fn rollover_summary_inherits_the_channel_audience() {
    held("rollover_summary_inherits_the_channel_audience");
}
