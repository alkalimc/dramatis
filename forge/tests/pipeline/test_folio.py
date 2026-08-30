"""Folio contracts: what ships, and what a reader may assume about it."""

from __future__ import annotations

from dramatis_forge.corpus.build import run as corpus_run
from dramatis_forge.normalize.runner import run as normalize_run
from dramatis_forge.store.archive import Archive
from dramatis_forge.store.folio import FOLIO_FORMAT_VERSION, Folio

STORY = """{{剧情模拟器|文本数据=
[name="阿米娅"]  博士，我们该出发了，罗德岛的准备已经完成。
[name="凯尔希"]  再等一下。还有些事没有交代清楚。
[name="阿米娅"]  是什么事？
[name="凯尔希"]  等到了地方你就知道了。现在没有必要多说。
}}"""


def _built(tmp_path, pack) -> tuple[Folio, object]:
    with Archive(tmp_path / "t.archive") as archive:
        archive.write_seeds({"S1": ["测试剧情"], "S2": ["阿米娅"]})
        archive.put_page("测试剧情", STORY, 11, "now")
        archive.put_page("阿米娅", "{{CharinfoV2|干员名=阿米娅}}\n"
                                  "{{人员档案|档案1=基础档案|档案1文本=罗德岛的公开领袖，"
                                  "卡特斯族，年龄不详，持有源石技艺。}}", 22, "now")
        archive.commit()
        normalize_run(archive, pack)
        report = corpus_run(archive, pack, tmp_path / "t.folio")
    return Folio(tmp_path / "t.folio", readonly=True), report


def test_every_unit_carries_a_revision(tmp_path, pack):
    """Per-record provenance is the whole basis of the attribution obligation: without a
    revid a passage cannot be traced to the version it came from."""
    folio, _ = _built(tmp_path, pack)
    with folio:
        missing = folio.count("chunks") and folio.db.execute(
            "SELECT COUNT(*) FROM chunks WHERE revid IS NULL").fetchone()[0]
        assert missing == 0


def test_every_unit_carries_a_header(tmp_path, pack):
    """A dialogue unit stripped of its scene and speakers is close to unsearchable."""
    folio, _ = _built(tmp_path, pack)
    with folio:
        assert folio.db.execute(
            "SELECT COUNT(*) FROM chunks WHERE header IS NULL OR header=''").fetchone()[0] == 0


def test_the_lexical_index_covers_every_unit(tmp_path, pack):
    folio, _ = _built(tmp_path, pack)
    with folio:
        units = folio.count("chunks")
        indexed = folio.db.execute("SELECT COUNT(*) FROM chunks_fts").fetchone()[0]
        assert units == indexed


def test_the_manifest_declares_what_a_reader_must_implement(tmp_path, pack):
    """Units are stored once, so context comes from widening a hit. Declaring it lets a
    reader refuse a corpus it cannot serve correctly, instead of silently truncating."""
    folio, _ = _built(tmp_path, pack)
    with folio:
        assert "neighbor_expand" in folio.get_meta("requires")
        assert folio.get_meta("format_version") == FOLIO_FORMAT_VERSION
        assert folio.get_meta("segmenter")


def test_the_fingerprint_is_content_derived(tmp_path, pack):
    """Two builds of the same inputs must fingerprint identically, or it is useless for
    lineage checks and reproducibility claims."""
    first, _ = _built(tmp_path / "a", pack)
    second, _ = _built(tmp_path / "b", pack)
    with first, second:
        assert first.get_meta("build_fingerprint") == second.get_meta("build_fingerprint")
        assert first.get_meta("build_fingerprint").startswith("sha256:")


def test_units_are_addressable_by_span_for_widening(tmp_path, pack):
    folio, _ = _built(tmp_path, pack)
    with folio:
        rows = list(folio.db.execute(
            "SELECT span_of, span_from, span_to FROM chunks WHERE template='dialogue'"))
        assert rows and all(r["span_of"] and r["span_from"] is not None for r in rows)


def test_the_roster_records_material_volume_not_a_verdict(tmp_path, pack):
    """Confidence measures how much source material exists. It is used to *change
    behaviour* — a thinly covered character is written as terse — rather than to apologise
    for the data."""
    folio, _ = _built(tmp_path, pack)
    with folio:
        row = folio.db.execute(
            "SELECT person_id, material, confidence FROM persons").fetchone()
        assert row["person_id"] == "阿米娅"
        assert row["material"] > 0 and 0.0 <= row["confidence"] <= 1.0


def test_per_template_statistics_are_published(tmp_path, pack):
    """Lexical and dense scores are not comparable across unit types whose lengths differ
    by an order of magnitude; the ranker needs these to calibrate rather than guess."""
    folio, report = _built(tmp_path, pack)
    with folio:
        stats = {r["template"]: r for r in folio.db.execute("SELECT * FROM template_stats")}
        assert set(stats) == set(report.by_template)
        assert all(s["count"] > 0 for s in stats.values())


def test_a_rebuild_replaces_rather_than_accumulates(tmp_path, pack):
    """A folio is a distributable whose fingerprint must describe its whole contents, so
    it is never patched in place."""
    folio, _ = _built(tmp_path, pack)
    with folio:
        first = folio.count("chunks")
    folio, _ = _built(tmp_path, pack)
    with folio:
        assert folio.count("chunks") == first
