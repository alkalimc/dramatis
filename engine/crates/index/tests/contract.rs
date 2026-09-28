//! Forge → engine contract, retrieval side: lexical search over the toy folio.
//!
//! See `crates/folio/tests/contract.rs` for how the folio is supplied and when this skips.

use std::path::PathBuf;

use index::params::Retrieve;
use index::{Index, Normalise, SearchRequest, Source};

/// A word that occurs in exactly one toy dialogue line.
const KNOWN_TERM: &str = "zephyrine";

fn contract_folio() -> Option<PathBuf> {
    match std::env::var_os("DRAMATIS_CONTRACT_FOLIO") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var("DRAMATIS_REQUIRE_CONTRACT").as_deref() == Ok("1") => {
            panic!("DRAMATIS_REQUIRE_CONTRACT=1 but DRAMATIS_CONTRACT_FOLIO is not set")
        }
        None => {
            eprintln!("skipped: set DRAMATIS_CONTRACT_FOLIO (or run `make contract` in forge/)");
            None
        }
    }
}

#[test]
fn lexical_search_finds_the_known_unit_and_expands_it() -> anyhow::Result<()> {
    let Some(path) = contract_folio() else {
        return Ok(());
    };
    let mut index = Index::open(&path, Retrieve::default())?;
    assert_eq!(index.segmenter().name(), "char-bigram");

    let response = index.search(&SearchRequest::new(KNOWN_TERM))?;
    let top = response.hits.first().expect("the known term is indexed");
    let unit = top.item.unit().expect("a corpus unit");
    assert_eq!(unit.page, "Chapter 2");
    assert!(unit.text.contains(KNOWN_TERM));
    assert_eq!(top.source, Source::Unscoped);
    assert_eq!(top.neighbours.len(), 2, "one neighbour on each side");
    assert!(top.neighbours.iter().all(|n| n.adjacent_to(unit)));
    assert!(response.confidence.top1 > 0.0);

    // As a person: attribution and the materialised scope decide the tag, and weighting
    // never changes confidence.
    let request = SearchRequest {
        as_person: Some("Alice".into()),
        ..SearchRequest::new(KNOWN_TERM)
    };
    let scoped = index.search(&request)?;
    assert_eq!(scoped.confidence, response.confidence);
    for hit in &scoped.hits {
        let unit = hit.item.unit().expect("a corpus unit");
        if unit.persons.iter().any(|p| p == "Alice") {
            assert_eq!(hit.source, Source::Own, "{}", unit.id);
        } else {
            assert_ne!(hit.source, Source::Unscoped, "{}", unit.id);
        }
    }

    // An alias query is resolved to its person before searching. Expansion only: the
    // person filter depends on attribution the toy corpus need not carry.
    index.set_normalise(Normalise::Expand);
    let response = index.search(&SearchRequest::new("Alice (Winter)"))?;
    let resolved = response.resolved.expect("alternate form is an alias");
    assert_eq!(resolved.target, "Alice");
    assert_eq!(resolved.person.as_deref(), Some("Alice"));
    assert!(!response.hits.is_empty());

    // People are found by their own units: every reason is attributed to its person.
    let people = index.find_people("lighthouse", None, &[], 5)?;
    for found in &people.persons {
        assert!(found.reason.persons.contains(&found.person), "{found:?}");
    }
    Ok(())
}
