"""Generated reports: figures with targets, and the reading sample.

Built end to end on a toy pack (defined in conftest), never on a real one.
"""

from __future__ import annotations

from dramatis_forge.corpus.build import run as corpus_run
from dramatis_forge.normalize.runner import run as normalize_run
from dramatis_forge.pack import FigureSpec
from dramatis_forge.report import figures, samples
from dramatis_forge.store.archive import Archive
from dramatis_forge.store.folio import Folio


def _built(tmp_path, pack, n_scenes: int = 12):
    path = tmp_path / "t.archive"
    with Archive(path) as a:
        scenes = [f"s{i}" for i in range(n_scenes)]
        a.write_seeds({"A": scenes, "B": ["p1", "blank"], "R": ["x", "y"]})
        for i, s in enumerate(scenes):
            a.put_page(s, "\n".join(f"{'xy'[j % 2]}: line {j} of {s}" for j in range(i + 3)),
                       i + 1, "now")
        a.put_page("p1", "== Head ==\nsome prose here", 90, "now")
        a.put_page("blank", " ", 91, "now")
        a.put_page("x", "person", 92, "now")
        a.put_page("y", "person", 93, "now")
        a.set_meta("watermark", 42)
        a.commit()
        normalize_run(a, pack)
        corpus_run(a, pack, tmp_path / "t.folio", segmenter="none")
    return path, tmp_path / "t.folio"


def test_derived_figures_and_targets(tmp_path, toy_pack):
    archive, folio = _built(tmp_path, toy_pack)
    specs = (
        FigureSpec("units", "folio:chunk_count", target=">= 1"),
        FigureSpec("redundancy", "derived:redundancy", target="<= 0.005"),
        FigureSpec("aligned", "derived:boundary_aligned", target=">= 0.9"),
        FigureSpec("speakers", "derived:speakers_distinct"),
        FigureSpec("scorable", "derived:scorable_at", member="5"),
        FigureSpec("guards.high", "derived:guards_high", target="== 0"),
        FigureSpec("guards.g4", "derived:guards_low", member="G4"),
        FigureSpec("impossible", "folio:chunk_count", target="< 0"),
    )
    got = {f.key: f for f in figures.resolve(specs, archive, folio)}
    assert got["units"].passed is True
    assert got["redundancy"].passed is True
    assert got["speakers"].value == 2
    assert got["scorable"].value == 2
    assert got["guards.g4"].value == 1  # "no accepted baseline"
    assert got["impossible"].passed is False
    report = figures.render(got.values(), toy_pack, fingerprint="sha256:x")
    assert "❌" in report and "✅" in report


def test_folio_carries_stopwords_and_shapes(tmp_path, toy_pack):
    _archive, folio = _built(tmp_path, toy_pack)
    with Folio(folio, readonly=True) as f:
        assert f.get_meta("stopwords") == ["the", "a"]
        assert f.get_meta("template_shapes") == {"talk": "dialogue", "lore": "lore"}


def test_sample_selection_is_deterministic_and_broad(tmp_path, toy_pack):
    archive, _folio = _built(tmp_path, toy_pack)
    with Archive(archive, readonly=True) as a:
        first = samples.select(a, toy_pack)
        second = samples.select(a, toy_pack)
    assert list(first.samples) == list(second.samples)
    routes = first.routes()
    assert routes == ["A · scene", "B · prose"]
    scene_route = first.of("A · scene")
    assert len(scene_route) >= samples.PER_ROUTE
    reasons = {key for s in scene_route for key, _ in s.why}
    assert {"random", "largest", "smallest", "most_records"} <= reasons
    blank = [s for s in first.of("B · prose") if s.title == "blank"]
    assert blank and any(k == "empty" and f["reason"] == "blank page" for k, f in blank[0].why)


def test_the_sample_changes_when_the_archive_does(tmp_path, toy_pack):
    archive, _folio = _built(tmp_path, toy_pack, n_scenes=40)
    with Archive(archive) as a:
        before = [t for (r, t), s in samples.select(a, toy_pack).samples.items()
                  if any(k == "random" for k, _ in s.why)]
        a.set_meta("watermark", 43)
        a.set_meta("last_update", {"changed": ["s3"], "added": ["s7"], "gone": []})
        sel = samples.select(a, toy_pack)
    after = [t for (r, t), s in sel.samples.items() if any(k == "random" for k, _ in s.why)]
    assert before != after
    why = {t: {k for k, _ in s.why} for (_r, t), s in sel.samples.items()}
    assert "changed" in why["s3"] and "added" in why["s7"]


def test_samples_write_an_index_and_three_files_per_page(tmp_path, toy_pack):
    archive, _folio = _built(tmp_path, toy_pack)
    out = tmp_path / "samples"
    (out / "stale").mkdir(parents=True)
    with Archive(archive, readonly=True) as a:
        index, sel = samples.write(a, toy_pack, out)
    assert not (out / "stale").exists()
    text = index.read_text()
    assert text.startswith("# Samples")
    folders = [p for p in out.iterdir() if p.is_dir()]
    assert len(folders) == 2
    assert all(len(list(f.glob("*.3-archived.md"))) == len(list(f.glob("*.1-source.wikitext")))
               for f in folders)
