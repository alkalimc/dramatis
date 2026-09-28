"""Samples, figures, attribution and text fallback on the built toy corpus."""

from __future__ import annotations

import pytest
from conftest import build, sync

from dramatis_forge import text as text_mod
from dramatis_forge.archive import Archive
from dramatis_forge.folio import Folio
from dramatis_forge.report import attribution, figures, samples

# --------------------------------------------------------------------------- #
# samples
# --------------------------------------------------------------------------- #


def reasons(sel, route):
    return {title: [key for key, _f in why] for title, why in sel.routes[route].items()}


def test_boundary_selection_is_deterministic(built, pack):
    with Archive(built.archive, readonly=True) as a:
        first, second = samples.select(a, pack), samples.select(a, pack)
    assert first.routes == second.routes
    story = reasons(first, "story · dialogue")
    assert story["Chapter 1"] == ["first", "kind", "kind", "kind"]  # first choice/line/scene
    kinds = {f["kind"] for t, why in first.routes["story · dialogue"].items()
             for key, f in why if key == "kind"}
    assert kinds == {"choice", "line", "scene"}
    assert story["Chapter 2"] == ["largest", "most_records"]
    assert "smallest" in story["Interlude"] and "findings" in story["Interlude"]
    assert "empty" in story["Interlude"]             # first G3 by title, with its reason
    people = reasons(first, "people · prose")
    assert people["Alice"][:2] == ["first", "largest"] and "multi_form" in people["Alice"]
    assert "smallest" in people["Alice (Winter)"]
    assert "Bob" not in people  # no boundary reaches it


def test_samples_include_every_change_and_removed_snapshot(wiki, pack, paths):
    sync(wiki, pack, paths)
    build(pack, paths)
    wiki.edit("Chapter 1", "Alice: Rewritten line.\n", story=True)
    wiki.edit("Chapter 3", "Bob: A new chapter.\n", story=True)
    wiki.delete("Chapter 2")
    sync(wiki, pack, paths)
    build(pack, paths)
    out = paths.samples
    with Archive(paths.archive, readonly=True) as a:
        index, count = samples.write(a, pack, out)
    doc = index.read_text(encoding="utf-8")

    assert doc.startswith("# Toy samples")  # pack text overrides the framework default
    for heading in ("## Sync status", "## Formatting quality", "## This sync's changes",
                    "## Boundary samples", "### story · dialogue", "### people · prose"):
        assert heading in doc
    assert "1 changed · 1 added · 1 removed" in doc
    for row in ("| Chapter 1 | changed |", "| Chapter 3 | added |",
                "| Chapter 2 | removed (as it was before removal) |"):
        assert row in doc
    removed = out / "changes" / "Chapter_2.1-source.wikitext"
    assert "lighthouse" in removed.read_text(encoding="utf-8")
    assert '"lines"' in (out / "changes" / "Chapter_2.2-records.json").read_text(encoding="utf-8")
    assert count >= 3


def test_samples_start_from_an_empty_directory(built, pack, paths):
    stale = [paths.samples / "changes" / "Gone.1-source.wikitext", paths.samples / "old.md"]
    for path in stale:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("stale", encoding="utf-8")
    with Archive(built.archive, readonly=True) as a:
        samples.write(a, pack, paths.samples)
    assert not any(p.exists() for p in stale)
    assert (paths.samples / "INDEX.md").exists()


def test_residue_scan_finds_leftover_markup(built, pack):
    with Archive(built.archive) as a:
        assert samples.residue(a, pack) == []
        a.db.execute("INSERT INTO lines(scene,seq,text,kind) VALUES('X',0,'left {{over}}','s')")
        [hit] = samples.residue(a, pack)
    assert (hit.kind, hit.pattern, hit.hits) == ("line", "template", 1)


# --------------------------------------------------------------------------- #
# figures
# --------------------------------------------------------------------------- #


def test_figures_derive_values_and_judge_targets(built, pack):
    figs = {f.key: f for f in figures.resolve(pack, built)}
    assert figs["units"].value == 15 and figs["units"].passed is True
    assert figs["persons"].value == 2 and figs["forms.alt"].value == 1
    # The toy corpus deliberately has one unexplained empty page, so this target fails.
    assert figs["guards.high"].value == 1 and figs["guards.high"].passed is False
    md = figures.render(figs.values(), pack)
    assert "| `units` | 15 | ✅ > 0 |" in md and "❌ == 0" in md


def test_v2_figures(built, pack):
    figs = {f.key: f for f in figures.resolve(pack, built)}
    # Every guard is listed, the silent ones too.
    assert set(figs["guards.by_guard"].value) == {"G1", "G2", "G3", "G4", "G5"}
    assert figs["guards.by_guard"].value["G5"] == {"high": 0, "low": 0}
    assert "G2 0/0" in figs["guards.by_guard"].shown
    # Low findings: no-baseline notes (G1, G4) and the scriptless interlude (G3), which
    # the pack's note explains.
    assert sum(v["low"] for v in figs["guards.by_guard"].value.values()) == 3
    assert figs["guards.unattributed"].value == 2
    assert figs["cooccur.pairs"].passed and figs["birthdays"].passed
    assert figs["units.attributed"].value == 10
    assert figs["cooccur.scenes_per_person"].value == 2.0
    assert 0 < figs["stopword.top_df"].value <= 1 and "(the)" in figs["stopword.top_df"].shown
    assert "pipeline.hours" not in figs  # no timings stamped by this build helper
    with Archive(built.archive) as a:
        a.set_meta("timings", {"build": 1800, "sync_full": 5400})
    figs = {f.key: f for f in figures.resolve(pack, built)}
    assert figs["pipeline.hours"].value == 2.0


def test_meets_rejects_unreadable_target():
    assert figures.meets(3, "< 5") is True and figures.meets("x", "< 5") is None
    with pytest.raises(ValueError):
        figures.meets(3, "about five")


def test_check_reports_dangling_link_undefined_id_unknown_key(tmp_path, pack):
    docs = tmp_path / "docs"
    docs.mkdir()
    (docs / "a.md").write_text(
        "| D10 | a decision |\n\nSee [b](b.md), [gone](missing.md), D10 and D99.\n"
        "Measured: `units`, `units.later`, `persons`, `units.bogus`, `forms.{alt}`.\n",
        encoding="utf-8")
    (docs / "b.md").write_text("Clean: `units`, D10.\n", encoding="utf-8")
    hits = [(h.path.name, h.what, h.detail) for h in figures.scan(docs, pack).hits]
    assert hits == [
        ("a.md", "dangling link", "missing.md"),
        ("a.md", "undefined id", "D99 is referenced but never defined"),
        ("a.md", "unknown key", "`units.bogus` is neither computed nor planned"),
    ]
    (docs / "a.md").unlink()
    (docs / "b.md").write_text("| D10 | x |\nClean: `units` and D10.\n", encoding="utf-8")
    assert figures.scan(docs, pack).hits == []


# --------------------------------------------------------------------------- #
# attribution and text
# --------------------------------------------------------------------------- #


def test_attribution_refuses_when_a_unit_lacks_revid(built, pack, tmp_path):
    template = tmp_path / "NOTICE.md"
    template.write_text("{SITE} {PAGE_COUNT} {UNIT_COUNT}\n{PAGE_ROWS}\n", encoding="utf-8")
    with Archive(built.archive, readonly=True) as a, Folio(built.folio, readonly=True) as f:
        data = attribution.collect(a, f)
    assert data.complete
    [out] = attribution.render(data, pack, [template], tmp_path / "out")
    assert out.read_text(encoding="utf-8").startswith("https://toy.invalid 5 15")

    with Folio(built.folio) as f:
        f.db.execute("UPDATE chunks SET revid=NULL WHERE ord=0")
    with Archive(built.archive, readonly=True) as a, Folio(built.folio, readonly=True) as f:
        data = attribution.collect(a, f)
    assert not data.complete and data.units_without_revid == 1
    with pytest.raises(ValueError, match="no source revision"):
        attribution.render(data, pack, [template], tmp_path / "out")


def test_say_prefers_pack_text_then_framework_default(pack):
    assert pack.say("record.lore") == "prose sections"
    assert pack.say("record.line") == text_mod.TEXT["record.line"]
    assert pack.say("samples.delta", changed=1, added=2, removed=3) == (
        "- This sync: 1 changed · 2 added · 3 removed")
    with pytest.raises(KeyError):
        pack.say("no.such.key")


def test_term_columns_follow_every_record_order():
    from dramatis_forge.report.inspect import _merge_orders
    assert _merge_orders([["b", "d"], ["a", "b", "c", "d"], ["e"]]) == ["a", "b", "c", "d", "e"]
