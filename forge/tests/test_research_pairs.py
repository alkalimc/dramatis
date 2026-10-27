"""`forge research pairs`: every family, the kinds of negatives, the split and its leak check."""

from __future__ import annotations

from pathlib import Path

import pytest
from research_support import build, pid
from typer.testing import CliRunner

from dramatis_forge.research import cli as research_cli
from dramatis_forge.research import export, pairs, split, source
from dramatis_forge.folio import Folio
from dramatis_forge.config import Paths
import typer

SHARE = 0.4


@pytest.fixture
def folio(tmp_path: Path) -> Path:
    return build(tmp_path / "toy.folio")


@pytest.fixture
def out(folio: Path, tmp_path: Path):
    return export.run(folio, tmp_path / "research", share=SHARE)


def corpus(folio: Path) -> source.Corpus:
    with Folio(folio, readonly=True) as f:
        return source.load(f)


def rows(path: Path) -> list[dict]:
    return export.read_lines(path)


def by_family(summary, family: str, side: str | None = None):
    return [p for p in summary.pairs if p.family == family and (side is None or p.split == side)]


def test_every_family_makes_pairs_on_both_sides(out):
    for fam in pairs.FAMILIES:
        assert by_family(out, fam), fam
    assert any(p.split == "train" for p in out.pairs)
    assert any(p.split == "test" for p in out.pairs)


def test_files_and_pair_format(out):
    d = out.outdir
    for name in (export.UNITS, export.TRAIN, export.TEST, export.EVAL, export.SPLIT,
                 export.MANIFEST):
        assert (d / name).exists(), name
    units = {u["id"]: u for u in rows(d / export.UNITS)}
    assert set(next(iter(units.values()))) == {"id", "header", "text", "template", "page",
                                               "persons"}
    for side in (export.TRAIN, export.TEST):
        for p in rows(d / side):
            assert set(p) == {"query", "positive", "negatives", "family", "split", "page",
                              "persons"}
            assert p["positive"] in units and all(n in units for n in p["negatives"])
            assert p["page"] == units[p["positive"]]["page"]
            assert p["positive"] not in p["negatives"]


def test_alias_negatives_are_homonyms_first(folio, tmp_path):
    c = corpus(folio)
    raw = [r for r in pairs.alias(c) if r.query == "Twin" and r.positive == f"card:{pid(1)}"]
    assert raw and raw[0].candidates[0] == (f"card:{pid(2)}", "homonym")
    # Every alias pair has negatives; lookalikes follow the homonyms. (Same-kind entities
    # come last and only fill what is not already named by the first two kinds.)
    everyone = list(pairs.alias(c))
    assert everyone and all(r.candidates for r in everyone)
    assert {k for r in everyone for _, k in r.candidates} >= {"homonym", "lookalike"}


def test_disambiguation_negatives_are_siblings(folio):
    c = corpus(folio)
    raws = {r.positive: r for r in pairs.disambiguation(c)}
    assert set(raws) == {f"card:{pid(i)}" for i in (3, 4, 5)}  # "Solo" is not ambiguous
    r = raws[f"card:{pid(3)}"]
    assert r.query == "Mark P03"  # the word plus what is left of the target's name
    assert {u for u, _ in r.candidates} == {f"card:{pid(4)}", f"card:{pid(5)}"}


def test_section_negatives_are_other_sections_of_the_page(folio):
    c = corpus(folio)
    for r in pairs.section(c):
        u = c.units[r.positive]
        assert r.query == f"{u.page} {u.title}"
        for uid, kind in r.candidates:
            assert kind == "sibling_section"
            assert c.units[uid].page == u.page and c.units[uid].span_of != u.span_of


def test_speaker_negatives(folio):
    c = corpus(folio)
    raws = list(pairs.speaker(c))
    voice = [r for r in raws if r.variant == "voice"]
    assert voice
    for r in voice:
        (who,) = r.persons
        assert all(who in c.persons_of(u) and u != r.positive for u, _ in r.candidates)
    dlg = [r for r in raws if r.variant == "dialogue"]
    assert dlg
    for r in dlg:
        (who,) = r.persons
        kinds = {k for _, k in r.candidates}
        assert kinds <= {"same_speaker_other_scene", "same_scene_other_speaker"}
        for u, k in r.candidates:
            if k == "same_scene_other_speaker":
                assert who not in c.persons_of(u)
                assert c.units[u].span_of == c.units[r.positive].span_of
            else:
                assert who in c.persons_of(u)
                assert c.units[u].span_of != c.units[r.positive].span_of


def test_identity_negatives_are_other_cards_of_the_same_group(folio):
    c = corpus(folio)
    raws = list(pairs.identity(c, pairs.Params(group_facet="group")))
    assert {r.query for r in raws} >= {"P00 alt", "P06 style"}
    for r in raws:
        (who,) = r.persons
        group = c.persons[who].facets["group"]
        assert r.candidates
        for uid, kind in r.candidates:
            other = uid.removeprefix("card:")
            assert kind == "same_group_person" and other != who
            assert c.persons[other].facets["group"] == group


@pytest.mark.parametrize("mode", split.MODES)
def test_no_test_person_or_page_in_training(folio, tmp_path, mode):
    s = export.run(folio, tmp_path / mode, share=SHARE, mode=mode)
    units = {u["id"]: u for u in rows(s.outdir / export.UNITS)}
    sp = s.split
    assert sp.test_persons and sp.train_persons
    assert not sp.test_persons & sp.train_persons
    for p in rows(s.outdir / export.TRAIN):
        assert p["page"] in sp.train_pages
        assert not sp.test_persons & set(p["persons"])
        for uid in [p["positive"], *p["negatives"]]:
            assert units[uid]["page"] in sp.train_pages
            assert not sp.test_persons & set(units[uid]["persons"])
    for p in rows(s.outdir / export.TEST):
        assert p["page"] in sp.test_pages
    # a person on any test page is a test person
    c = corpus(folio)
    for page in sp.test_pages:
        assert c.page_persons()[page] <= sp.test_persons


def test_leak_check_fires_on_a_crafted_leak(out):
    sp = out.split
    units = {u["id"]: u for u in rows(out.outdir / export.UNITS)}
    train = rows(out.outdir / export.TRAIN)
    split.check(sp, train, units)  # the real export is clean

    leaker = next(iter(sorted(sp.test_persons)))
    bad = {**train[0], "persons": [*train[0]["persons"], leaker]}
    with pytest.raises(split.LeakError, match="test person"):
        split.check(sp, [bad], units)

    test_page = next(iter(sorted(sp.test_pages)))
    bad = {**train[0], "page": test_page}
    with pytest.raises(split.LeakError, match="test page"):
        split.check(sp, [bad], units)

    test_unit = next(u for u in units.values() if u["page"] in sp.test_pages)
    bad = {**train[0], "negatives": [*train[0]["negatives"], test_unit["id"]]}
    with pytest.raises(split.LeakError):
        split.check(sp, [bad], units)


def test_split_is_deterministic_and_stable(folio, tmp_path):
    a = export.run(folio, tmp_path / "a", share=SHARE)
    b = export.run(folio, tmp_path / "b", share=SHARE)
    for name in (export.UNITS, export.TRAIN, export.TEST, export.EVAL, export.SPLIT,
                 export.MANIFEST):
        assert (a.outdir / name).read_bytes() == (b.outdir / name).read_bytes(), name
    other = export.run(folio, tmp_path / "c", share=SHARE, seed=99)
    assert other.split.test_pages != a.split.test_pages

    # In page mode the verdict for a page does not depend on which other pages exist.
    pa = split.assign({"x": set(), "y": set()}, seed=3, share=0.5, mode="page")
    pb = split.assign({"x": set(), "z": set(), "w": set()}, seed=3, share=0.5, mode="page")
    assert ("x" in pa.test_pages) == ("x" in pb.test_pages)
    assert split.page_fraction(1, "abc") == split.page_fraction(1, "abc")
    with pytest.raises(ValueError):
        split.assign({}, share=1.0)
    with pytest.raises(ValueError):
        split.assign({}, mode="other")

    extra = build(tmp_path / "more.folio", extra_page=True)
    more = export.run(extra, tmp_path / "d", share=SHARE, mode="page")
    base = export.run(folio, tmp_path / "e", share=SHARE, mode="page")
    assert {p for p in base.split.test_pages if p.startswith("Lore")} \
        == {p for p in more.split.test_pages if p.startswith("Lore")}


def test_eval_set_is_in_the_suite_format(out):
    queries = rows(out.outdir / export.EVAL)
    tests = rows(out.outdir / export.TEST)
    assert len(queries) == len(tests)
    assert len({q["qid"] for q in queries}) == len(queries)
    for q in queries:
        assert set(q) == {"qid", "family", "text", "stratum", "note", "gold", "hard_negatives"}
        assert q["stratum"] in ("verbatim", "paraphrase")
        assert 2 in q["gold"].values() and set(q["gold"].values()) <= {1, 2}
        assert not set(q["gold"]) & set(q["hard_negatives"])


def test_manifest_names_the_folio_and_counts(out):
    text = (out.outdir / export.MANIFEST).read_text(encoding="utf-8")
    assert "sha256:toy" in text and "| speaker |" in text and "same_speaker_line" in text


def test_cap_per_family_is_deterministic(folio):
    c = corpus(folio)
    a = pairs.construct(c, pairs.Params(max_per_family=3))
    b = pairs.construct(c, pairs.Params(max_per_family=3))
    assert [(r.family, r.query) for r in a] == [(r.family, r.query) for r in b]
    assert all(sum(1 for r in a if r.family == f) <= 3 for f in pairs.FAMILIES)


def test_cli_writes_the_files(folio, tmp_path):
    paths = Paths.for_pack("toy", tmp_path / "home")
    paths.folio.parent.mkdir(parents=True)
    paths.folio.write_bytes(folio.read_bytes())
    app = typer.Typer()

    def die(message: str, hint: str = "") -> None:
        raise typer.Exit(1)

    from rich.console import Console
    research_cli.register(app, resolve=lambda *_: (None, paths), die=die, console=Console())
    res = CliRunner().invoke(app, ["research", "pairs", "--test-share", str(SHARE)])
    assert res.exit_code == 0, res.output
    assert (paths.pack_dir / "research" / export.TRAIN).exists()
