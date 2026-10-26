//! Short exact-match aliases: whole word, case sensitive, inside longer queries.

use folio::fixture::{Builder, TempFolio, UnitSpec};
use index::params::Retrieve;
use index::{Ambiguity, How, Index, SearchRequest};

fn corpus() -> TempFolio {
    let mut b = Builder::new();
    for (id, text, persons) in [
        ("u1", "the lantern burns all night", &["wen"][..]),
        ("u2", "a doctor arrives at the harbor", &["doc"][..]),
        ("u3", "the lantern burns low", &[][..]),
    ] {
        b.unit(UnitSpec {
            id,
            template: "dialogue",
            page: "p",
            text,
            persons,
            ..Default::default()
        });
    }
    b.person("wen", None)
        .person("doc", None)
        .person("ace", None);
    b.alias("Ben", "wen", "term_exact")
        .alias("Dr.", "doc", "term_exact")
        .alias("Ace", "ace", "term_exact")
        .alias("Oni", "wen", "term_exact")
        .alias("Oni", "doc", "term_exact")
        .alias("Lantern", "wen", "term")
        .alias("Brand", "wen", "redirect");
    b.finish()
}

fn resolve(query: &str) -> (Option<index::Resolved>, Option<Ambiguity>) {
    let file = corpus();
    let index = Index::open(file.path(), Retrieve::default()).unwrap();
    let r = index.search(&SearchRequest::new(query)).unwrap();
    (r.resolved, r.ambiguous)
}

fn target(query: &str) -> Option<String> {
    resolve(query).0.map(|r| r.target)
}

#[test]
fn a_whole_word_in_a_sentence_resolves() {
    for q in ["who is Ben?", "Ben", "is Ben here", "(Ben)", "Ben, please"] {
        assert_eq!(target(q).as_deref(), Some("wen"), "{q}");
    }
    let (resolved, _) = resolve("who is Ben in the story");
    let r = resolved.unwrap();
    assert_eq!((r.alias.as_str(), r.how), ("Ben", How::Word));
    assert_eq!(r.person.as_deref(), Some("wen"));
}

#[test]
fn latin_neighbours_block_the_match() {
    for q in [
        "what is Bend like",
        "Benjamin said",
        "the Ben2 unit",
        "xBen",
        "9Ben here",
    ] {
        assert_eq!(target(q), None, "{q}");
    }
}

#[test]
fn cjk_and_punctuation_neighbours_are_boundaries() {
    for q in [
        "\u{6587}Ben\u{6587}",
        "Ben\u{3002}",
        "a-Ben-b",
        "who/Ben/x",
        "Ben's",
    ] {
        assert_eq!(target(q).as_deref(), Some("wen"), "{q}");
    }
}

#[test]
fn matching_is_case_sensitive() {
    for q in ["who is ben", "who is BEN", "ben", "a ace of spades"] {
        assert_eq!(target(q), None, "{q}");
    }
    assert_eq!(target("the Ace card").as_deref(), Some("ace"));
}

#[test]
fn the_dot_is_part_of_the_alias() {
    assert_eq!(target("ask Dr. Smith").as_deref(), Some("doc"));
    assert_eq!(target("ask Dr Smith"), None);
    assert_eq!(target("ask Dr.Smith"), None);
}

#[test]
fn an_alias_with_several_targets_is_ambiguous() {
    let (resolved, ambiguous) = resolve("what is Oni about");
    assert!(resolved.is_none());
    let a = ambiguous.unwrap();
    assert_eq!(a.alias, "Oni");
    assert_eq!(a.candidates, ["doc", "wen"]);
}

#[test]
fn two_different_exact_aliases_apply_nothing() {
    assert_eq!(target("Ben and Ace"), None);
}

#[test]
fn other_kinds_keep_their_matching() {
    // Whole-query lookup only; no substring or word-level matching for other kinds.
    assert_eq!(target("Lantern").as_deref(), Some("wen"));
    assert_eq!(target("the Lantern"), None);
    assert_eq!(target("lantern"), None);
    assert_eq!(target("Brand").as_deref(), Some("wen"));
    assert_eq!(target("is Brand here"), None);
    let r = resolve("Lantern").0.unwrap();
    assert_eq!(r.how, How::Unique);
}

#[test]
fn lexical_retrieval_of_the_query_text_is_still_case_insensitive() {
    let file = corpus();
    let index = Index::open(file.path(), Retrieve::default()).unwrap();
    for q in ["lantern", "LANTERN", "Lantern"] {
        let r = index.search(&SearchRequest::new(q)).unwrap();
        assert!(!r.hits.is_empty(), "{q}");
    }
}
