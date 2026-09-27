//! Forge → engine contract, retrieval side: lexical search over the toy folio.
//!
//! See `crates/folio/tests/contract.rs` for how the folio is supplied and when this skips.

use std::path::PathBuf;

use folio::Folio;
use index::{Index, Mode, Normalise, Request};

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
    let folio = Folio::open(&path)?;
    let index = Index::new(&folio);
    assert_eq!(index.segmenter().name(), "char-bigram");

    let request = Request {
        query: KNOWN_TERM.to_string(),
        mode: Mode::Lexical,
        expand: 1,
        ..Request::default()
    };
    let response = index.search(&request, None)?;
    let top = response.hits.first().expect("the known term is indexed");
    assert_eq!(top.unit.page, "Chapter 2");
    assert!(top.unit.text.contains(KNOWN_TERM));
    assert!(top.lexical_rank == Some(0));
    assert_eq!(top.context.len(), 2);
    assert!(top.context.iter().all(|n| n.adjacent_to(&top.unit)));

    // An alias query is resolved to its person before searching.
    let request = Request {
        query: "Alice (Winter)".to_string(),
        mode: Mode::Lexical,
        normalise: Normalise::Expand,
        ..Request::default()
    };
    let response = index.search(&request, None)?;
    let resolved = response.resolved.expect("alternate form is an alias");
    assert_eq!(resolved.target, "Alice");
    assert_eq!(resolved.person.as_deref(), Some("Alice"));
    assert!(!response.hits.is_empty());
    Ok(())
}
