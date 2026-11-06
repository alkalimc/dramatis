//! The adaptive cut behind what is offered to a user: evidence about the person alone, a
//! break in the scores, one person when one dominates.

use folio::fixture::{Builder, TempFolio, UnitSpec};
use index::params::{ConfidenceBands, FindPeople, Retrieve};
use index::{Cluster, Index, Level, cluster_len};

/// * `dan` has three units about a kiln of his own.
/// * `eve`, `fay`, `gus`, `hal` stand in one scene that mentions the kiln in the same words
///   as dan's first unit.
/// * `ivy` has a single passing line about it.
/// * `p1`..`p4` each have two identical lines about a quarry.
fn corpus() -> TempFolio {
    let mut b = Builder::new();
    let unit = |b: &mut Builder, id, page, text, persons: &[&str]| {
        b.unit(UnitSpec {
            id,
            template: "dialogue",
            page,
            text,
            persons,
            ..Default::default()
        });
    };
    let line = "the kiln fires at dawn and the kiln must stay hot";
    unit(&mut b, "d1", "yard", line, &["dan"]);
    unit(
        &mut b,
        "d2",
        "yard",
        "dan stacks the kiln shelves",
        &["dan"],
    );
    unit(&mut b, "d3", "yard", "bricks for the kiln", &["dan"]);
    unit(&mut b, "c1", "scene", line, &["eve", "fay", "gus", "hal"]);
    unit(&mut b, "i1", "road", "a kiln stood far away", &["ivy"]);
    for p in ["p1", "p2", "p3", "p4"] {
        for n in 0..2 {
            let id = format!("{p}-{n}");
            b.unit(UnitSpec {
                id: &id,
                template: "dialogue",
                page: p,
                text: "the quarry road is long",
                persons: &[p],
                ..Default::default()
            });
        }
    }
    for filler in 0..20 {
        let id = format!("f{filler}");
        let text = format!("filler text number {filler} about nothing");
        b.unit(UnitSpec {
            id: &id,
            template: "lore",
            page: "Filler",
            text: &text,
            ..Default::default()
        });
    }
    for p in [
        "dan", "eve", "fay", "gus", "hal", "ivy", "p1", "p2", "p3", "p4",
    ] {
        b.person(p, None);
    }
    b.finish()
}

fn open(file: &TempFolio, high: bool) -> Index {
    // With these bands every score in the small corpus is high, or none is.
    let confidence = if high {
        ConfidenceBands {
            low_entropy: 1.0,
            low_top1: 0.01,
            high_top1: 0.02,
            high_entropy: 1.0,
        }
    } else {
        ConfidenceBands::default()
    };
    Index::open(
        file.path(),
        Retrieve {
            confidence,
            ..Retrieve::default()
        },
    )
    .unwrap()
}

fn names(people: &index::People) -> Vec<&str> {
    people.persons.iter().map(|p| p.person.as_str()).collect()
}

#[test]
fn a_scene_counts_for_less_than_a_line_of_ones_own() {
    let file = corpus();
    let ix = open(&file, true);
    let p = FindPeople::default();
    let found = ix.ranked("kiln", None, &[], &p).unwrap();
    let dan = found.get("dan").unwrap();
    let eve = found.get("eve").unwrap();
    assert_eq!((dan.attributed, eve.attributed), (1, 4));
    assert_eq!((dan.specific, eve.specific), (3, 0));
    assert!(dan.fit > eve.fit, "same words, but the scene is shared");
    // The discount reorders; the unit's own score is untouched.
    assert!(eve.fit < eve.score);

    // Without it the fit is the score.
    let flat = FindPeople { crowd: 0.0, ..p };
    let found = ix.ranked("kiln", None, &[], &flat).unwrap();
    assert!(found.persons.iter().all(|m| m.fit == m.score));
}

#[test]
fn people_without_evidence_of_their_own_are_not_offered() {
    let file = corpus();
    let ix = open(&file, false);
    let found = ix
        .recommend("kiln", None, &[], &FindPeople::default())
        .unwrap();
    // Eve and the others only stand in a scene; ivy rests on one weak unit.
    assert_eq!(names(&found), ["dan"]);
    assert_eq!(found.persons[0].level, Level::Low);
}

#[test]
fn a_single_strong_unit_is_enough_but_a_single_weak_one_is_not() {
    let file = corpus();
    let p = FindPeople::default();
    let weak = open(&file, false)
        .ranked("kiln", Some(&["ivy".into()]), &[], &p)
        .unwrap();
    assert_eq!(weak.persons[0].matched, 1);
    assert!(
        open(&file, false)
            .recommend("kiln", Some(&["ivy".into()]), &[], &p)
            .unwrap()
            .persons
            .is_empty()
    );
    let strong = open(&file, true)
        .recommend("kiln", Some(&["ivy".into()]), &[], &p)
        .unwrap();
    assert_eq!(names(&strong), ["ivy"]);
}

#[test]
fn a_plateau_keeps_its_head_when_scores_are_high_and_one_when_they_are_not() {
    let file = corpus();
    let p = FindPeople::default();
    let high = open(&file, true)
        .recommend("quarry", None, &[], &p)
        .unwrap();
    assert_eq!(names(&high), ["p1", "p2", "p3"], "ties run in id order");
    let low = open(&file, false)
        .recommend("quarry", None, &[], &p)
        .unwrap();
    assert_eq!(names(&low), ["p1"]);
}

#[test]
fn the_safety_bound_limits_the_ranking_itself() {
    let file = corpus();
    let p = FindPeople {
        max: 2,
        ..FindPeople::default()
    };
    let found = open(&file, true).ranked("quarry", None, &[], &p).unwrap();
    assert_eq!(found.persons.len(), 2);
}

#[test]
fn the_cut_falls_at_the_largest_drop_in_the_window() {
    let p = FindPeople::default();
    // One person clearly ahead.
    assert_eq!(cluster_len(&[10.0, 7.0, 6.8, 6.5], &p), Cluster::Break(1));
    // A pair, then a fall.
    assert_eq!(cluster_len(&[10.0, 9.5, 6.0, 5.8], &p), Cluster::Break(2));
    // The larger of two drops wins, the earlier on a tie.
    assert_eq!(cluster_len(&[10.0, 8.5, 6.0, 5.0], &p), Cluster::Break(2));
    assert_eq!(cluster_len(&[10.0, 8.0, 6.4], &p), Cluster::Break(1));
    // Nothing stands out.
    assert_eq!(
        cluster_len(&[10.0, 9.6, 9.3, 9.1, 9.0], &p),
        Cluster::Plateau
    );
    // Drops beyond the window are not looked at.
    let p = FindPeople {
        window: 3,
        ..FindPeople::default()
    };
    assert_eq!(
        cluster_len(&[10.0, 9.8, 9.6, 4.0, 3.0], &p),
        Cluster::Plateau
    );
    // Nothing to compare with.
    assert_eq!(cluster_len(&[5.0], &p), Cluster::Break(1));
    assert_eq!(cluster_len(&[], &p), Cluster::Break(0));
}
