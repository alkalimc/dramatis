"""Samples, figures, attribution and text fallback on the built toy corpus."""

from __future__ import annotations

import dataclasses
import json

import pytest
from conftest import build, sync

from dramatis_forge import corpus, evals
from dramatis_forge import text as text_mod
from dramatis_forge.archive import Archive
from dramatis_forge.folio import Folio
from dramatis_forge.pack import FigureSpec, Route
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
    assert story["Chapter 1"] == ["first", "smallest", "kind", "kind", "kind"]
    kinds = {f["kind"] for t, why in first.routes["story · dialogue"].items()
             for key, f in why if key == "kind"}
    assert kinds == {"choice", "line", "scene"}
    assert story["Chapter 2"] == ["largest", "most_records"]
    # pages that produce nothing are never sampled; they are listed by reason instead
    assert "Interlude" not in story and "Silent" not in story
    listed = {t for titles in first.silent.values() for t in titles}
    assert {"Interlude", "Silent"} <= listed
    assert ("empty", (("reason", "marked as having no script"),)) in first.silent
    people = reasons(first, "people · prose")
    assert people["Alice"][:2] == ["first", "largest"] and "multi_form" in people["Alice"]
    assert "smallest" in people["Alice (Winter)"]
    assert "Bob" not in people  # no boundary reaches it


def test_silent_pages_are_listed_by_reason_and_never_sampled(built, pack):
    out = built.samples
    with Archive(built.archive, readonly=True) as a:
        index, _ = samples.write(a, pack, out, built.folio)
    doc = index.read_text(encoding="utf-8")
    assert "## Pages not sampled" in doc
    assert "### No unit: marked as having no script\n\n- Interlude\n" in doc
    assert "- Silent\n" in doc
    assert not list(out.rglob("Interlude.*")) and not list(out.rglob("Silent.*"))
    assert not any(p.name.endswith("excluded") for p in out.iterdir())


def test_a_changed_page_with_no_units_gets_one_line_unless_it_lost_its_units(wiki, pack, paths):
    sync(wiki, pack, paths)
    build(pack, paths)
    wiki.edit("Interlude", "{{NoScript}}\nstill nothing\n", story=True)
    wiki.edit("Chapter 1", "Alice: Rewritten line.\n", story=True)
    sync(wiki, pack, paths)
    build(pack, paths)
    with Archive(paths.archive, readonly=True) as a, Folio(paths.folio, readonly=True) as f:
        producing = samples.unit_pages(a, f)
    out = paths.samples

    def doc_of(lost: list[str]) -> str:
        with Archive(paths.archive) as a:
            a.set_meta("unit_pages_previous", lost)
            a.commit()
            index, _ = samples.write(a, pack, out, paths.folio)
        return index.read_text(encoding="utf-8")

    doc = doc_of([])
    assert "| Interlude | changed; no units, so no files | — |" in doc
    assert not list((out / "changes").glob("Interlude.*"))
    assert list((out / "changes").glob("Chapter_1.*")) and "Chapter 1" in producing
    # a page that had units at the last sync and has none now keeps its files
    doc = doc_of(["Interlude"])
    assert "changed; had units before the last sync, has none now" in doc
    assert list((out / "changes").glob("Interlude.*"))


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


def test_samples_leave_nothing_of_an_earlier_run(built, pack, paths):
    (paths.samples / "Empty 2").mkdir(parents=True, exist_ok=True)
    stale = [paths.samples / "changes" / "Gone.1-source.wikitext", paths.samples / "old.md"]
    for path in stale:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("stale", encoding="utf-8")
    with Archive(built.archive, readonly=True) as a:
        samples.write(a, pack, paths.samples)
    assert not any(p.exists() for p in stale)
    assert not (paths.samples / "Empty 2").exists()
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
    assert figs["units.engine_meta"].value == 0 and figs["units.engine_meta"].passed
    assert figs["units.attributed"].value == 10
    assert figs["cooccur.scenes_per_person"].value == 2.0
    assert 0 < figs["stopword.top_df"].value <= 1 and "(the)" in figs["stopword.top_df"].shown
    assert "pipeline.hours" not in figs  # no timings stamped by this build helper
    with Archive(built.archive) as a:
        a.set_meta("timings", {"build": 1800, "sync_full": 5400})
    figs = {f.key: f for f in figures.resolve(pack, built)}
    assert figs["pipeline.hours"].value == 2.0


def test_figures_read_other_tools_output_and_say_why_one_is_missing(built, pack):
    specs = (
        FigureSpec("bench.p50", "report:bench.json", target="< 5"),
        FigureSpec("eval.family.*.ndcg", "report:results.json"),
        FigureSpec("persona.meta_leak", "persona:report", target="== 0"),
        FigureSpec("old.number", "report:stale.json"),
        FigureSpec("absent.number", "report:nowhere.json"),
    )
    pack = dataclasses.replace(pack, figures=pack.figures + specs)
    built.reports.mkdir(parents=True, exist_ok=True)
    with Folio(built.folio) as folio:
        current = folio.get_meta("build_fingerprint")
    (built.reports / "bench.json").write_text(json.dumps(
        {"folio_fingerprint": current, "keys": {"bench.p50": 2.5}}))
    (built.reports / "results.json").write_text(json.dumps(
        {"folio_fingerprint": current,
         "keys": {"eval.family.a.ndcg": 0.5, "eval.family.b.ndcg": 21322.0, "eval.questions": 1}}))
    (built.reports / "stale.json").write_text(json.dumps(
        {"folio_fingerprint": "sha256:other", "keys": {"old.number": 1}}))
    (built.pack_dir / "persona").mkdir()
    (built.pack_dir / "persona" / "probe-report.json").write_text(
        json.dumps({"figures": {"persona.meta_leak": 0}}))
    figs = {f.key: f for f in figures.resolve(pack, built)}
    assert figs["bench.p50"].value == 2.5 and figs["bench.p50"].passed is True
    assert figs["eval.family.a.ndcg"].shown == "0.5" and figs["eval.family.b.ndcg"].shown == "21,322"
    assert "eval.questions" not in figs
    assert figs["persona.meta_leak"].passed is True and "probe run" in figs["persona.meta_leak"].note
    assert figs["old.number"].shown == "\u2b1c" and "another folio" in figs["old.number"].note
    assert figs["absent.number"].value is None and "no reports/nowhere.json" in figs["absent.number"].note


def test_meets_rejects_unreadable_target():
    assert figures.meets(3, "< 5") is True and figures.meets("x", "< 5") is None
    with pytest.raises(ValueError):
        figures.meets(3, "about five")


def test_check_reports_dangling_link_undefined_id_unknown_key(tmp_path, pack):
    docs = tmp_path / "docs"
    docs.mkdir()
    (docs / "a.md").write_text(
        "| R10 | a decision |\n\nSee [b](b.md), [gone](missing.md), R10 and R99.\n"
        "Measured: `units`, `units.later`, `persons`, `units.bogus`, `forms.{alt}`.\n",
        encoding="utf-8")
    (docs / "b.md").write_text("Clean: `units`, R10.\n", encoding="utf-8")
    hits = [(h.path.name, h.what, h.detail) for h in figures.scan(docs, pack).hits]
    assert hits == [
        ("a.md", "dangling link", "missing.md"),
        ("a.md", "undefined id", "R99 is referenced but never defined"),
        ("a.md", "unknown key", "`units.bogus` is neither computed nor planned"),
    ]
    (docs / "a.md").unlink()
    (docs / "b.md").write_text("| R10 | x |\nClean: `units` and R10.\n", encoding="utf-8")
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


def test_attribution_lists_pages_that_only_supply_names(built, pack):
    named = dataclasses.replace(pack, routes=(
        *pack.routes, Route(pack.routes[0].seed, pack.routes[0].parser,
                            title="Interlude", names=True)))
    with Archive(built.archive, readonly=True) as a, Folio(built.folio, readonly=True) as f:
        plain = attribution.collect(a, f, pack)
        data = attribution.collect(a, f, named)
    assert "Interlude" not in {p["page"] for p in plain.pages}
    [entry] = [p for p in data.pages if p["page"] == "Interlude"]
    assert entry["units"] == 0 and entry["revid"]


def test_eval_target_without_page_or_person_resolves_to_a_lore_title(built):
    with Folio(built.folio, readonly=True) as f:
        index = evals._Index(f)
        title, = {r["title"] for r in f.db.execute(
            "SELECT title FROM chunks WHERE template='lore' AND title LIKE '%Early life'")}
    assert title.rsplit(" › ", 1)[-1] == "Early life"
    assert index.chunks_for("Early life") and index.chunks_for(title) == index.chunks_for("Early life")
    assert not index.chunks_for("No such entry")


def test_lore_units_never_share_a_place(built):
    with Folio(built.folio, readonly=True) as f:
        shared = f.db.execute(
            "SELECT span_of, span_from FROM chunks WHERE template='lore' "
            "GROUP BY span_of, span_from HAVING COUNT(*) > 1").fetchall()
    assert not shared


def test_name_alias_to_an_unmentioned_target_is_dropped(built):
    rows = [("a", "Alice", "term"), ("b", "lighthouse", "term"), ("c", "Nowhere Land", "term"),
            ("d", "Nowhere Land", "redirect"), ("e", "Nowhere Land", "realname"),
            ("f", "Chapter 1", "term")]
    with Folio(built.folio, readonly=True) as f:
        kept = corpus._drop_unreached_names(corpus.CorpusReport(), f, rows)
    assert kept == rows[:2] + [rows[3], rows[5]]


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


def test_wording_command_refreshes_the_folio_in_place(built, pack):
    from typer.testing import CliRunner

    from dramatis_forge.cli import app

    with Folio(built.folio) as folio:
        folio.set_meta("wording", {"stale.key": "x"})
        before = folio.get_meta("build_fingerprint")
        folio.db.execute("INSERT INTO prompts(subject,slot,body,generator_version) VALUES('p','system','keep',1)")
    out = CliRunner().invoke(app, ["wording", "--pack", "toy", "--home", str(built.home)])
    assert out.exit_code == 0, out.output
    with Folio(built.folio, readonly=True) as folio:
        assert folio.get_meta("wording") == dict(pack.wording)
        assert folio.get_meta("build_fingerprint") == before
        assert folio.db.execute("SELECT body FROM prompts WHERE subject='p'").fetchone()[0] == "keep"


def test_credits_go_into_the_folio_without_moving_the_fingerprint(built, pack):
    from dataclasses import replace

    from typer.testing import CliRunner

    import dramatis_forge.cli as cli
    from dramatis_forge.pack import Publication

    credited = replace(pack, publication=Publication(credit_by="someone", credit_url="https://example.org/someone",
                                                     project_url="https://example.org/tool", licence="CC BY-SA 4.0"))
    expected = {"by": "someone", "url": "https://example.org/someone", "project": "https://example.org/tool",
                "licence": "CC BY-SA 4.0"}
    assert credited.publication.credits() == expected
    assert Publication(credit_by="a").credits() == {"by": "a"}
    with Folio(built.folio) as folio:
        before = folio.get_meta("build_fingerprint")
    real = cli._resolve
    cli._resolve = lambda p, h: (credited, real(p, h)[1])
    try:
        out = CliRunner().invoke(cli.app, ["wording", "--pack", "toy", "--home", str(built.home)])
    finally:
        cli._resolve = real
    assert out.exit_code == 0, out.output
    with Folio(built.folio, readonly=True) as folio:
        assert folio.get_meta("credits") == expected
        # Who is credited is not part of what the corpus is.
        assert folio.get_meta("build_fingerprint") == before
