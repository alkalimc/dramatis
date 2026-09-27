"""Guards and baselines: drift has three outcomes, a missing baseline is one finding, and an
empty page is either explained (low) or a failure (high)."""

from __future__ import annotations

import json

from conftest import build, sync

from dramatis_forge import baseline
from dramatis_forge.archive import Archive
from dramatis_forge.guards import HIGH, LOW, check_drift


def test_drift_fell_is_high_grew_is_low_equal_is_silent():
    found = check_drift({"a": 4, "b": 6, "c": 5}, {"a": 5, "b": 5, "c": 5})
    assert [(f.severity, f.detail.split(":")[0]) for f in found] == [
        (HIGH, "a count FELL"), (LOW, "b count grew")]


def test_no_baseline_is_a_single_low_finding():
    [finding] = check_drift({"a": 1, "b": 2}, None, guard="G4")
    assert (finding.guard, finding.severity) == ("G4", LOW)


def test_unbaselined_key_is_reported_low():
    [finding] = check_drift({"new": 3}, {})
    assert finding.severity == LOW and "new has no accepted baseline" in finding.detail


def findings(paths, guard):
    with Archive(paths.archive, readonly=True) as a:
        return [tuple(r) for r in a.db.execute(
            "SELECT severity, page, detail FROM guard_findings WHERE guard=? ORDER BY page",
            (guard,))]


def test_g3_empty_with_reason_is_low_without_is_high(built):
    by_page = {page: (sev, detail) for sev, page, detail in findings(built, "G3")}
    assert by_page["Interlude"] == (LOW, "marked as having no script")
    assert by_page["Silent"][0] == HIGH and "gave no reason" in by_page["Silent"][1]


def test_g2_reports_unknown_macro_with_its_page(wiki, pack, paths):
    wiki.edit("Chapter 3", "Bob: Hello ${ghost}.\n", story=True, log=False)
    sync(wiki, pack, paths)
    build(pack, paths)
    assert (LOW, "Chapter 3", "unknown engine macro: ${ghost}") in findings(paths, "G2")


def test_baseline_accept_show_round_trip(built, pack, wiki):
    with Archive(built.archive) as a:
        assert baseline.load(a.path) is None
        path, accepted = baseline.accept(a)
        measured = baseline.measure(a)
    assert path == built.baselines
    data = json.loads(path.read_text(encoding="utf-8"))
    assert data["seeds"] == {"people": 3, "story": 4}
    assert data["identity"] == {"persons": 2, "form:alt": 1}
    loaded = baseline.load(built.archive)
    assert (loaded.seeds, loaded.identity, loaded.watermark) == (
        measured.seeds, measured.identity, accepted.watermark)

    # With a baseline accepted, the "none accepted" findings go and a shrink is high.
    wiki.delete("Chapter 2")
    sync(wiki, pack, built)
    build(pack, built)
    g1 = findings(built, "G1")
    assert not any("no accepted baseline" in d for _s, _p, d in g1)
    assert any(s == HIGH and "story count FELL: 4 → 3" in d for s, _p, d in g1)
    assert not any("no accepted baseline" in d for _s, _p, d in findings(built, "G4"))
