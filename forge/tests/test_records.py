"""Record storage: round-trip, text-inclusive uniqueness, insert-never-replace, and the
produced-vs-stored reconciliation that makes a lost row a finding."""

from __future__ import annotations

import json

from dramatis_forge import guards
from dramatis_forge.archive import Archive
from dramatis_forge.guards import HIGH, LOW, Reconciliation
from dramatis_forge.records import Line, Lore, Scene


def test_severity_values():
    assert (HIGH, LOW) == ("high", "low")
    assert list(guards.GUARDS) == ["G1", "G2", "G3", "G4", "G5"]


def test_round_trip_and_raw_attachment(tmp_path):
    path = tmp_path / "t.archive"
    with Archive(path) as a:
        a.put_page("P", "body", 7, "now")
        a.insert_records([Scene(page="P", category="c", group="g", revid=7)])
        a.insert_records([Line(scene="P", seq=0, text="hi", speaker="Ann")])
        a.set_meta("k", {"x": 1})
    assert path.with_suffix(".rawcache").exists()
    with Archive(path, readonly=True) as a:
        assert tuple(a.db.execute("SELECT id,category,grp,revid FROM scenes").fetchone()) == (
            "P", "c", "g", 7)
        assert tuple(a.db.execute("SELECT speaker,text,kind FROM lines").fetchone()) == (
            "Ann", "hi", "speech")
        assert a.page("P")["wikitext"] == "body" and a.get_meta("k") == {"x": 1}


def test_uniqueness_includes_text_and_duplicates_are_counted(tmp_path):
    with Archive(tmp_path / "t.archive") as a:
        same = Lore(page="P", path=("A",), text="one")
        stored, ignored = a.insert_records([same, Lore(page="P", path=("A",), text="two"), same])
        assert (stored, ignored) == (2, 1)
        assert json.loads(a.db.execute("SELECT path FROM lore LIMIT 1").fetchone()[0]) == ["A"]


def test_insert_never_replaces(tmp_path):
    with Archive(tmp_path / "t.archive") as a:
        a.insert_records([Line(scene="S", seq=0, text="original")])
        assert a.insert_records([Line(scene="S", seq=0, text="impostor")]) == (0, 1)
        assert a.scalar("SELECT text FROM lines") == "original"


def test_reconciliation_explains_or_fires():
    r = Reconciliation()
    r.note("lore", produced=3, stored=2, ignored=1)
    [finding] = r.check()
    assert finding.severity == LOW and "1 exact duplicates" in finding.detail
    r.note("line", produced=5, stored=3, ignored=0)
    lost = [f for f in r.check() if f.severity == HIGH]
    assert len(lost) == 1 and "2 rows unaccounted" in lost[0].detail


def test_findings_replaced_per_stage_and_tallied(tmp_path):
    with Archive(tmp_path / "t.archive") as a:
        a.write_findings([("G1", HIGH, None, "x")], stage="scope")
        a.write_findings([("G2", LOW, "P", "y"), ("G2", LOW, "P", "z")], stage="normalize")
        a.write_findings([("G2", LOW, "P", "only")], stage="normalize")
        tally = a.tally()
    assert tally["G1"] == (1, 0) and tally["G2"] == (0, 1) and tally["G5"] == (0, 0)


def test_stale_record_table_is_rebuilt_but_inputs_kept(tmp_path):
    path = tmp_path / "t.archive"
    with Archive(path) as a:
        a.put_page("P", "body", 1, "now")
        a.db.execute("DROP TABLE lore")
        a.db.execute("CREATE TABLE lore (page TEXT, text TEXT)")
        a.db.execute("INSERT INTO lore VALUES ('P','old shape')")
    with Archive(path) as a:
        assert a.count("lore") == 0 and "sig" in {r[1] for r in a.db.execute("PRAGMA table_info(lore)")}
        assert a.page("P")["wikitext"] == "body"
