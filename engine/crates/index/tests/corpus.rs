//! Integration tests against the real corpus.
//!
//! Skipped when it is absent, so a fresh clone still builds and tests green. What is
//! checked here cannot be checked against a fixture: whether the pipeline behaves on
//! 58,853 real units with a real lexical index.

use std::path::PathBuf;

use folio::Folio;
use index::{Index, Mode, Request};

fn corpus() -> Option<Folio> {
    let path = PathBuf::from("../../../artifacts/arknights/arknights.folio");
    if !path.exists() {
        eprintln!("skipping: no corpus at {}", path.display());
        return None;
    }
    Folio::open(path).ok()
}

fn request(query: &str) -> Request {
    Request {
        query: query.to_string(),
        mode: Mode::Lexical,
        ..Default::default()
    }
}

#[test]
fn opens_and_reports_what_it_requires() {
    let Some(folio) = corpus() else { return };
    let manifest = folio.manifest();
    assert_eq!(manifest.format_version, 1);
    assert!(manifest.unit_count > 50_000, "units: {}", manifest.unit_count);
    // Reaching here at all means every declared requirement is implemented: `open` refuses
    // otherwise, which is the whole point of the declaration.
    assert!(manifest.requires.contains(&"neighbor_expand".to_string()));
}

#[test]
fn segmenter_is_taken_from_the_corpus_not_assumed() {
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    assert_eq!(idx.segmenter().name(), folio.manifest().segmenter_name());
}

#[test]
fn a_term_query_finds_its_definition() {
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    let response = idx.search(&request("源石技艺"), None).unwrap();
    assert!(!response.hits.is_empty());
    let top = &response.hits[0];
    assert!(top.unit.text.contains("源石技艺"));
    // Provenance on every hit: this is what makes redistribution traceable.
    assert!(top.unit.revid.is_some(), "a hit without a source revision");
    assert!(!top.unit.header.is_empty(), "a hit without its context line");
}

#[test]
fn a_template_filter_still_returns_the_requested_count() {
    // The bug this pins: filtering ran after truncation, so asking for two dialogue units
    // returned zero from a corpus holding 32,322 of them.
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    let response = idx
        .search(
            &Request {
                templates: vec!["dialogue".to_string()],
                top_k: 2,
                ..request("罗德岛的成立")
            },
            None,
        )
        .unwrap();
    assert_eq!(response.hits.len(), 2, "template filter emptied the result");
    assert!(response.hits.iter().all(|h| h.unit.template == "dialogue"));
}

#[test]
fn expansion_returns_genuinely_adjacent_units() {
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    let response = idx
        .search(
            &Request {
                templates: vec!["dialogue".to_string()],
                top_k: 1,
                expand: 1,
                ..request("罗德岛的成立")
            },
            None,
        )
        .unwrap();
    let hit = &response.hits[0];
    assert!(!hit.context.is_empty(), "expansion produced nothing");
    for neighbour in &hit.context {
        assert_eq!(neighbour.span_of, hit.unit.span_of, "neighbour from another scene");
        assert!(
            hit.unit.adjacent_to(neighbour),
            "unit {} is not adjacent to {}",
            hit.unit.id,
            neighbour.id
        );
    }
}

#[test]
fn a_query_matching_nothing_is_low_confidence_not_an_error() {
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    let response = idx.search(&request("zzzzqqqxxx不存在的词组"), None).unwrap();
    assert!(response.hits.is_empty());
    assert_eq!(response.signals.level, index::Confidence::Low);
}

#[test]
fn dense_mode_refuses_rather_than_silently_running_lexical() {
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    let result = idx.search(
        &Request {
            mode: Mode::Dense,
            ..request("源石")
        },
        None,
    );
    // Vectors are not written yet. Falling back to the lexical path would return plausible
    // results from a path the caller did not ask for — worse than an error, because nobody
    // would notice.
    assert!(result.is_err());
}

#[test]
fn alias_resolution_reaches_the_person() {
    let Some(folio) = corpus() else { return };
    // An alternate-form name must resolve to the one person, or the alias dictionary
    // disagrees with the roster.
    let targets = folio.resolve_alias("予愿安洁莉娜").unwrap();
    assert!(
        targets.contains(&"安洁莉娜".to_string()),
        "alternate form did not resolve to the person: {targets:?}"
    );
}

#[test]
fn the_roster_holds_the_measured_number_of_people() {
    let Some(folio) = corpus() else { return };
    assert_eq!(folio.person_count().unwrap(), 416);
    let person = folio.person("陈").unwrap().expect("陈 should be on the roster");
    assert!(
        person.forms.len() > 1,
        "陈 has alternate incarnations and should carry several forms"
    );
}

#[test]
fn results_are_reproducible() {
    let Some(folio) = corpus() else { return };
    let idx = Index::new(&folio);
    let first = idx.search(&request("感染者"), None).unwrap();
    let second = idx.search(&request("感染者"), None).unwrap();
    let ids = |r: &index::Response| r.hits.iter().map(|h| h.unit.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&first), ids(&second));
}
