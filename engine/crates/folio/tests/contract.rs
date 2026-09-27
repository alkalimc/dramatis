//! Forge → engine contract: open a folio the forge built from its toy test pack.
//!
//! The folio path comes from `DRAMATIS_CONTRACT_FOLIO` (`make contract` in `forge/` builds
//! one and runs this). Unset, the test is skipped with a note, unless
//! `DRAMATIS_REQUIRE_CONTRACT=1`, in which case a missing folio is a failure.
//!
//! The expectations mirror the toy corpus in `forge/tests/conftest.py`.

use std::path::PathBuf;

use folio::Folio;
use folio::manifest::IMPLEMENTED;

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
fn toy_folio_honours_the_contract() -> anyhow::Result<()> {
    let Some(path) = contract_folio() else {
        return Ok(());
    };
    let folio = Folio::open(&path)?;
    let manifest = folio.manifest();

    assert!(
        manifest
            .requires
            .iter()
            .all(|r| IMPLEMENTED.contains(&r.as_str())),
        "requires {:?} not all implemented",
        manifest.requires
    );
    assert!(manifest.requires.iter().any(|r| r == "neighbor_expand"));
    assert_eq!(manifest.pack, "toy");
    assert_eq!(manifest.segmenter_name(), "char-bigram");

    let units = folio.unit_count()?;
    assert!(units > 0);
    assert_eq!(units, manifest.unit_count);

    // Lexical: the FTS rowid is the unit ordinal.
    let ord: i64 = folio.conn().query_row(
        "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1",
        [KNOWN_TERM],
        |row| row.get(0),
    )?;
    let [hit] = &folio.units_by_ord(&[ord])?[..] else {
        panic!("one unit expected for ordinal {ord}");
    };
    assert_eq!(hit.page, "Chapter 2");
    assert!(hit.text.contains(KNOWN_TERM));
    assert!(
        hit.revid.is_some(),
        "every unit carries its source revision"
    );

    // Neighbour expansion returns the units on either side, and only adjacent ones.
    let context = folio.neighbours(hit, 1, 1)?;
    assert_eq!(context.len(), 2, "one neighbour on each side");
    assert!(context.iter().all(|n| n.adjacent_to(hit)));

    // Alias resolution: an alternate form reaches the one person.
    assert_eq!(
        folio.resolve_alias("Alice (Winter)")?,
        vec!["Alice".to_string()]
    );
    assert_eq!(
        folio.persons_for_alias("Alice (Winter)")?,
        vec!["Alice".to_string()]
    );
    let person = folio.person("Alice")?.expect("toy person on the roster");
    assert!(person.forms.iter().any(|f| f.page == "Alice (Winter)"));
    Ok(())
}
