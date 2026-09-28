//! Format-2 reading on a synthetic corpus built in SQL.

use folio::fixture::{Builder, UnitSpec};
use folio::{Error, Folio, Scope};

fn corpus() -> folio::fixture::TempFolio {
    let mut b = Builder::new();
    for (id, text, persons) in [
        ("d1", "alpha speaks first", &["ann", "ben"][..]),
        ("d2", "then beta answers", &["ben"][..]),
        ("d3", "gamma closes", &[][..]),
    ] {
        b.unit(UnitSpec {
            id,
            template: "dialogue",
            page: "Scene",
            text,
            persons,
            ..Default::default()
        });
    }
    b.person("ann", Some("03-14")).person("ben", None);
    b.scope("ann", "d1", "self").scope("ann", "d2", "lived");
    b.topic("ann", "lighthouse");
    b.cooccur("ben", "ann", 4);
    b.alias("Annie", "ann", "redirect");
    b.finish()
}

#[test]
fn reads_the_per_person_tables() -> anyhow::Result<()> {
    let file = corpus();
    let folio = Folio::open(file.path())?;

    let units = folio.units_by_ord(&[2, 0])?;
    assert_eq!(
        units.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        ["d3", "d1"],
        "ranked order is preserved"
    );
    assert_eq!(
        units[1].persons,
        ["ann", "ben"],
        "attribution is many-valued"
    );
    assert!(units[0].persons.is_empty());
    assert_eq!(folio.ords_for_ids(&["d2".into(), "missing".into()])?, [1]);

    let ann = folio.person("ann")?.unwrap();
    assert_eq!(ann.birthday.as_deref(), Some("03-14"));
    assert_eq!(folio.persons()?.len(), 2);
    assert_eq!(folio.persons_born_on("03-14")?, ["ann"]);

    assert_eq!(folio.cooccur("ann")?, [("ben".to_string(), 4)]);
    assert_eq!(folio.cooccur("ben")?, [("ann".to_string(), 4)]);
    assert_eq!(folio.cooccur_pair("ben", "ann")?, 4);
    assert_eq!(folio.cooccur_pair("ann", "nobody")?, 0);

    let ids: Vec<String> = ["d1", "d2", "d3"].map(String::from).into();
    let scope = folio.knowledge_scope("ann", &ids)?;
    assert_eq!(scope.get("d1"), Some(&Scope::Own));
    assert_eq!(scope.get("d2"), Some(&Scope::Lived));
    assert_eq!(scope.get("d3"), None);
    assert_eq!(folio.topic_terms("ann")?, ["lighthouse"]);
    assert_eq!(folio.persons_for_alias("Annie")?, ["ann"]);
    Ok(())
}

#[test]
fn neighbours_stay_inside_their_sequence() -> anyhow::Result<()> {
    let file = corpus();
    let folio = Folio::open(file.path())?;
    let [middle] = &folio.units_by_ord(&[1])?[..] else {
        panic!("one unit")
    };
    let around = folio.neighbours(middle, 1, 1)?;
    assert_eq!(
        around.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        ["d1", "d3"]
    );
    Ok(())
}

#[test]
fn refuses_a_version_one_corpus_and_an_unknown_requirement() {
    let old = Builder::new().manifest("format_version", "1").finish();
    assert!(matches!(
        Folio::open(old.path()),
        Err(Error::UnknownFormat { found: 1, .. })
    ));
    let greedy = Builder::new()
        .manifest("requires", r#"["neighbor_expand","span_merge"]"#)
        .finish();
    assert!(matches!(
        Folio::open(greedy.path()),
        Err(Error::UnmetRequirement { requirement }) if requirement == "span_merge"
    ));
}
