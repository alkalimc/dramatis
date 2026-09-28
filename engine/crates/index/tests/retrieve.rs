//! Retrieval behaviour on a synthetic corpus built in SQL.

use folio::fixture::{Builder, TempFolio, UnitSpec};
use index::params::{ConfidenceBands, Retrieve};
use index::{Error, Filters, Index, Item, Level, RerankPolicy, SearchMode, SearchRequest, Source};

/// Two scenes and some lore.
///
/// * `harbor` scene: ann speaks `h1`, ben speaks `h2`, `h3` belongs to nobody.
/// * `market` scene: cid speaks `m1` and `m2`.
/// * lore: `l1` about lighthouses (inside ann's topics), `l2` about lanterns (nobody's).
///
/// "lantern" occurs in every one of `h2`, `m1`, `l1`, `l2`, so a query for it ranks units
/// from all four sources for ann.
fn corpus() -> TempFolio {
    let mut b = Builder::new();
    let unit = |b: &mut Builder, id, template, page, text, persons: &[&str]| {
        b.unit(UnitSpec {
            id,
            template,
            page,
            text,
            persons,
            ..Default::default()
        });
    };
    unit(
        &mut b,
        "h1",
        "dialogue",
        "harbor",
        "the boats come home at dusk",
        &["ann"],
    );
    unit(
        &mut b,
        "h2",
        "dialogue",
        "harbor",
        "light the lantern for the boats",
        &["ben"],
    );
    unit(
        &mut b,
        "h3",
        "dialogue",
        "harbor",
        "the tide turns quietly",
        &[],
    );
    unit(
        &mut b,
        "m1",
        "dialogue",
        "market",
        "a lantern costs two coins",
        &["cid"],
    );
    unit(
        &mut b,
        "m2",
        "dialogue",
        "market",
        "cid counts the coins twice",
        &["cid"],
    );
    unit(
        &mut b,
        "l1",
        "lore",
        "Lighthouse",
        "the lighthouse lantern burns oil",
        &[],
    );
    unit(
        &mut b,
        "l2",
        "lore",
        "Lantern",
        "a lantern is a portable lamp lantern",
        &[],
    );
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
    b.person("ann", None)
        .person("ben", None)
        .person("cid", None);
    b.scope("ann", "h1", "self")
        .scope("ann", "h2", "lived")
        .scope("ann", "h3", "lived");
    b.topic("ann", "lighthouse");
    b.alias("Annie", "ann", "redirect");
    b.finish()
}

fn params() -> Retrieve {
    Retrieve {
        top_k: 4,
        neighbours: 0,
        ..Retrieve::default()
    }
}

fn open(file: &TempFolio, params: Retrieve) -> Index {
    Index::open(file.path(), params).unwrap()
}

fn ids(hits: &[index::Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.item.id()).collect()
}

#[test]
fn without_a_person_nothing_is_weighted() {
    let file = corpus();
    let index = open(&file, params());
    let response = index.search(&SearchRequest::new("lantern")).unwrap();
    assert_eq!(response.hits.len(), 4);
    assert!(response.hits.iter().all(|h| h.source.is_none()));
    assert!(response.hits.iter().all(|h| h.score == h.unweighted));
    // Repeating the term wins on BM25.
    assert_eq!(response.hits[0].item.id(), "l2");
}

#[test]
fn every_hit_is_tagged_by_where_it_sits_for_the_person() {
    let file = corpus();
    let index = open(&file, params());
    let request = SearchRequest {
        as_person: Some("ann".into()),
        top_k: 10,
        ..SearchRequest::new("lantern boats")
    };
    let response = index.search(&request).unwrap();
    let source = |id: &str| {
        response
            .hits
            .iter()
            .find(|h| h.item.id() == id)
            .and_then(|h| h.source)
    };
    assert_eq!(source("h1"), Some(Source::Own));
    assert_eq!(source("h2"), Some(Source::Lived));
    assert_eq!(source("l1"), Some(Source::InScope));
    assert_eq!(source("l2"), Some(Source::OutOfScope));
    assert_eq!(source("m1"), Some(Source::OutOfScope));
}

#[test]
fn weighting_reorders_but_confidence_reads_unweighted_scores() {
    let file = corpus();
    let heavy = Retrieve {
        scope_weight: index::params::ScopeWeights {
            in_scope: 10.0,
            ..Default::default()
        },
        ..params()
    };
    let index = open(&file, heavy);
    let plain = index.search(&SearchRequest::new("lantern")).unwrap();
    let request = SearchRequest {
        as_person: Some("ann".into()),
        ..SearchRequest::new("lantern")
    };
    let weighted = index.search(&request).unwrap();
    assert_eq!(weighted.hits[0].item.id(), "l1", "in-scope lore is lifted");
    assert_ne!(plain.hits[0].item.id(), "l1");
    assert_eq!(weighted.confidence, plain.confidence);
    let l2 = weighted.hits.iter().find(|h| h.item.id() == "l2");
    assert!(l2.is_some(), "weighting never removes a hit");
}

#[test]
fn filters_apply_before_the_cut() {
    let file = corpus();
    let index = open(
        &file,
        Retrieve {
            candidates: 1,
            ..params()
        },
    );
    for filters in [
        Filters {
            persons: vec!["cid".into()],
            ..Default::default()
        },
        Filters {
            pages: vec!["market".into()],
            ..Default::default()
        },
        Filters {
            templates: vec!["dialogue".into()],
            pages: vec!["market".into()],
            ..Default::default()
        },
    ] {
        let request = SearchRequest {
            filters: filters.clone(),
            ..SearchRequest::new("lantern")
        };
        // With a single candidate, filtering afterwards would return nothing.
        let hits = index.search(&request).unwrap().hits;
        assert_eq!(ids(&hits), ["m1"], "{filters:?}");
    }
}

#[test]
fn excluded_units_keep_their_place_but_are_listed_by_id() {
    let file = corpus();
    let index = open(
        &file,
        Retrieve {
            top_k: 2,
            ..params()
        },
    );
    let full = index.search(&SearchRequest::new("lantern")).unwrap();
    let first = full.hits[0].item.id().to_string();
    let request = SearchRequest {
        exclude: vec![first.clone(), "not-in-the-window".into()],
        ..SearchRequest::new("lantern")
    };
    let response = index.search(&request).unwrap();
    assert_eq!(ids(&response.hits), [full.hits[1].item.id()], "no refill");
    assert_eq!(response.excluded, [first]);
    assert_eq!(response.confidence, full.confidence);
}

#[test]
fn neighbours_are_fetched_for_returned_hits_only() {
    let file = corpus();
    let index = open(
        &file,
        Retrieve {
            neighbours: 1,
            ..params()
        },
    );
    let request = SearchRequest {
        top_k: 1,
        filters: Filters {
            persons: vec!["ben".into()],
            ..Default::default()
        },
        ..SearchRequest::new("lantern")
    };
    let hits = index.search(&request).unwrap().hits;
    assert_eq!(ids(&hits), ["h2"]);
    let around: Vec<&str> = hits[0].neighbours.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(around, ["h1", "h3"]);

    // A neighbour that is itself a hit, or already in the session, is not sent again.
    let request = SearchRequest {
        top_k: 2,
        exclude: vec!["h3".into()],
        filters: Filters {
            pages: vec!["harbor".into()],
            ..Default::default()
        },
        ..SearchRequest::new("boats")
    };
    let hits = index.search(&request).unwrap().hits;
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.neighbours.is_empty()));
}

#[test]
fn only_the_lexical_path_exists() {
    let file = corpus();
    let index = open(&file, params());
    for mode in [SearchMode::Hybrid, SearchMode::Dense] {
        let request = SearchRequest {
            mode,
            ..SearchRequest::new("lantern")
        };
        assert!(matches!(index.search(&request), Err(Error::DenseUnavailable(m)) if m == mode));
    }
    for mode in [SearchMode::Auto, SearchMode::Lexical] {
        let request = SearchRequest {
            mode,
            ..SearchRequest::new("lantern")
        };
        assert!(index.search(&request).is_ok());
    }
    let always = SearchRequest {
        rerank: RerankPolicy::Always,
        ..SearchRequest::new("lantern")
    };
    assert!(matches!(
        index.search(&always),
        Err(Error::RerankUnavailable)
    ));
}

#[test]
fn memories_rank_with_the_corpus() {
    let file = corpus();
    let index = open(&file, params());
    let request = SearchRequest {
        as_person: Some("ann".into()),
        ..SearchRequest::new("lantern coins")
    };
    let memories = [
        (
            "fact:1",
            "the user lost a lantern and two coins at the market",
        ),
        ("fact:2", "the user prefers tea"),
    ];
    let response = index.search_with_memories(&request, &memories).unwrap();
    let memory: Vec<&index::Hit> = response
        .hits
        .iter()
        .filter(|h| h.source == Some(Source::Memory))
        .collect();
    assert_eq!(memory.len(), 1, "a memory that does not match is not a hit");
    assert!(matches!(&memory[0].item, Item::Memory { id, .. } if id == "fact:1"));
    assert!(memory[0].unweighted > 0.0);

    // Memories are the person's; the maintainer surface has none to rank.
    assert!(matches!(
        index.search_with_memories(&SearchRequest::new("lantern"), &memories),
        Err(Error::MemoryWithoutPerson)
    ));
    // Memory ids already in the session are excluded like units.
    let request = SearchRequest {
        exclude: vec!["fact:1".into()],
        ..request
    };
    let response = index.search_with_memories(&request, &memories).unwrap();
    assert!(
        response
            .hits
            .iter()
            .all(|h| h.source != Some(Source::Memory))
    );
    assert_eq!(response.excluded, ["fact:1"]);
}

#[test]
fn an_alias_query_reaches_its_person() {
    let file = corpus();
    let index = open(&file, params());
    let response = index.search(&SearchRequest::new("Annie")).unwrap();
    let resolved = response.resolved.expect("redirect");
    assert_eq!(resolved.person.as_deref(), Some("ann"));
}

#[test]
fn find_people_aggregates_own_units_by_person() {
    let file = corpus();
    let index = open(&file, params());
    let people = index.find_people("lantern coins", None, 5, &[]).unwrap();
    let order: Vec<&str> = people.persons.iter().map(|p| p.person.as_str()).collect();
    assert_eq!(order, ["cid", "ben"], "ann has no own unit about it");
    let cid = people.get("cid").unwrap();
    assert_eq!(cid.matched, 2);
    assert!(["m1", "m2"].contains(&cid.reason.id.as_str()));
    assert!(cid.score >= people.get("ben").unwrap().score);

    // Excluded people are removed before the cut.
    let people = index
        .find_people("lantern coins", None, 1, &["cid".into()])
        .unwrap();
    assert_eq!(people.persons.len(), 1);
    assert_eq!(people.persons[0].person, "ben");

    // A scope restricts the candidates; an empty scope finds nobody.
    let people = index
        .find_people("lantern", Some(&["ann".into(), "ben".into()]), 5, &[])
        .unwrap();
    assert_eq!(people.persons.len(), 1);
    assert!(
        index
            .find_people("lantern", Some(&[]), 5, &[])
            .unwrap()
            .persons
            .is_empty()
    );
    assert!(
        index
            .find_people("", None, 5, &[])
            .unwrap()
            .persons
            .is_empty()
    );
}

#[test]
fn participants_are_ranked_with_their_levels() {
    let file = corpus();
    let bands = ConfidenceBands {
        low_top1: 0.1,
        high_top1: 0.5,
        high_entropy: 1.0,
    };
    let index = open(
        &file,
        Retrieve {
            confidence: bands,
            ..params()
        },
    );
    let group = ["ann".to_string(), "ben".into(), "cid".into()];
    let ranked = index.rank_participants("coins", &group).unwrap();
    assert_eq!(ranked.persons[0].person, "cid", "addressed");
    assert_eq!(ranked.persons[0].level, Level::High);
    assert!(ranked.get("ann").is_none(), "no own unit matched");
}

#[test]
fn a_memory_scores_exactly_as_the_unit_it_copies() {
    // Same text, same corpus statistics: the hand-rolled BM25 must agree with FTS5's.
    let file = corpus();
    let index = open(&file, params());
    let request = SearchRequest {
        as_person: Some("cid".into()),
        top_k: 20,
        ..SearchRequest::new("lantern coins")
    };
    let memories = [("fact:copy", "a lantern costs two coins")];
    let response = index.search_with_memories(&request, &memories).unwrap();
    let unweighted = |id: &str| {
        response
            .hits
            .iter()
            .find(|h| h.item.id() == id)
            .map(|h| h.unweighted)
            .unwrap()
    };
    let (unit, memory) = (unweighted("m1"), unweighted("fact:copy"));
    assert!(
        (unit - memory).abs() < 1e-6 * unit,
        "unit {unit} vs memory {memory}"
    );
}
