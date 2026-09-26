"""Archive bookkeeping: migrations on open, sync time, holdings, baselines, editor cache."""

from __future__ import annotations

import sqlite3

from dramatis_forge import baseline
from dramatis_forge.harvest import update as update_stage
from dramatis_forge.harvest.fetch import publish_holdings
from dramatis_forge.normalize.guards import HIGH, LEGACY_SEVERITY, LOW
from dramatis_forge.normalize.runner import run as normalize_run
from dramatis_forge.store.archive import Archive


def test_legacy_severity_values_are_rewritten_on_open(tmp_path):
    path = tmp_path / "t.archive"
    with Archive(path) as a:
        old = {v: k for k, v in LEGACY_SEVERITY.items()}
        old_high, old_low = old[HIGH], old[LOW]
        a.write_findings([("G1", old_high, None, "x"), ("G2", old_low, "p", "y")], stage="s")
    with Archive(path) as a:
        assert sorted(r["severity"] for r in a.db.execute("SELECT severity FROM guard_findings")) \
            == [HIGH, LOW]
        assert a.tally_from_table()["G1"] == (1, 0)
    with Archive(path) as a:  # idempotent
        assert a.count("guard_findings") == 2


def test_the_tally_lists_every_guard_including_silent_ones(archive):
    archive.write_findings([("G2", LOW, None, "x")], stage="s")
    tally = archive.tally_from_table()
    assert tally["G2"] == (0, 1) and tally["G4"] == (0, 0)


def test_stale_record_tables_are_rebuilt_but_inputs_survive(tmp_path):
    path = tmp_path / "t.archive"
    with Archive(path) as a:
        a.put_page("p", "body", 1, "now")
        a.write_seeds({"A": ["p"]})
    con = sqlite3.connect(path)
    con.execute("DROP TABLE scenes")
    con.execute("CREATE TABLE scenes (id TEXT PRIMARY KEY, old_column TEXT)")
    con.commit()
    con.close()
    with Archive(path) as a:
        cols = {r[1] for r in a.db.execute("PRAGMA table_info(scenes)")}
        assert "category" in cols and "old_column" not in cols
        assert a.page("p")["wikitext"] == "body" and a.seed("A") == ["p"]


def test_one_sync_time_whichever_path_ran(tmp_path):
    path = tmp_path / "t.archive"
    with Archive(path) as a:
        a.set_meta("fetched_at", "2020-01-01T00:00:00+00:00")
        a.set_meta("updated_at", "2021-01-01T00:00:00+00:00")
    with Archive(path) as a:
        assert a.get_meta("synced_at") == "2021-01-01T00:00:00+00:00"
        assert a.get_meta("fetched_at") is None
        stamped = a.mark_synced()
        assert a.get_meta("synced_at") == stamped


class _FakeWiki:
    requests = 0

    def __init__(self, bodies):
        self.bodies = bodies

    def recentchanges(self, namespaces, *, types="edit|new"):
        yield {"title": "s1", "rcid": 20}

    def content(self, titles):
        for t in titles:
            yield t, self.bodies.get(t), 2


def test_an_update_republishes_holdings_and_records_what_it_touched(archive, toy_pack):
    """`pages_held` once stayed at the previous run's count after an incremental update."""
    archive.write_seeds({"A": ["s1"], "B": ["p1"]})
    archive.put_page("p1", "text", 1, "now")
    archive.set_meta("watermark", 10)
    publish_holdings(archive, [])
    archive.commit()
    assert archive.get_meta("pages_held") == 1

    wiki = _FakeWiki({"s1": "x: hello\ny: there"})
    plan = update_stage.plan(wiki, archive, toy_pack, rescope=False)
    update_stage.apply(wiki, archive, toy_pack, plan)
    assert archive.get_meta("pages_held") == 2
    assert archive.get_meta("synced_at")
    assert archive.get_meta("last_update")["changed"] == ["s1"]


def test_baseline_accept_then_compare(archive, toy_pack):
    archive.write_seeds({"A": ["s1"], "B": ["p1"], "R": ["x"]})
    archive.put_page("x", "a person", 1, "now")
    archive.put_page("s1", "x: one\ny: two", 1, "now")
    archive.put_page("p1", "prose", 1, "now")
    archive.commit()
    assert baseline.load(archive.path) is None

    report = normalize_run(archive, toy_pack)
    no_file = [f for f in report.ledger.findings if f.guard == "G4"]
    assert [f.severity for f in no_file] == [LOW] and "no accepted baseline" in no_file[0].detail

    path, accepted = baseline.accept(archive)
    assert path.name == "BASELINES.json"
    assert accepted.seeds == {"A": 1, "B": 1, "R": 1}
    assert accepted.identity == {"persons": 1}
    assert baseline.load(archive.path).seeds == accepted.seeds

    report = normalize_run(archive, toy_pack)
    assert not [f for f in report.ledger.findings if f.guard == "G4"]

    path.write_text(path.read_text().replace('"persons": 1', '"persons": 3'))
    report = normalize_run(archive, toy_pack)
    assert any(f.guard == "G4" and f.severity == HIGH and "FELL" in f.detail
               for f in report.ledger.findings)


def test_editor_cache_is_keyed_by_revision(archive):
    archive.put_editors([("p", 5, "someone")])
    archive.put_editors([("p", 6, "someone else")])
    assert archive.editors() == {"p": (6, "someone else")}
