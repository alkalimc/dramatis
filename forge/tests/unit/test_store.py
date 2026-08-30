"""Storage invariants.

Three of these pin down defects that silently destroyed data in the previous schema.
Each one looked like a working pipeline reporting slightly odd counts.
"""

from __future__ import annotations

from dramatis_forge.normalize.records import CharRef, Lore, Voice
from dramatis_forge.store.archive import Archive


def test_same_address_different_text_coexists(archive: Archive):
    """`lore` keyed by `(page, path)` meant two paragraphs under one heading overwrote
    each other. Measured: 21 rows destroyed — 13 worldview tips, 8 codex entries."""
    rows = [
        Lore(page="贴士一览", path=("贴士", "背景"), text="源石是泰拉的矿物。"),
        Lore(page="贴士一览", path=("贴士", "背景"), text="天灾由源石引发。"),
    ]
    stored, ignored = archive.insert_records(rows)
    assert (stored, ignored) == (2, 0)


def test_exact_duplicates_collapse_and_are_counted(archive: Archive):
    """Deduplication is fine; it just has to be *visible*, so the difference between
    produced and stored can be explained rather than guessed at."""
    same = Lore(page="p", path=("a",), text="一样的正文")
    stored, ignored = archive.insert_records([same, same])
    assert (stored, ignored) == (1, 1)


def test_char_refs_with_the_same_name_in_different_arcs_coexist(archive: Archive):
    rows = [
        CharRef(page="剧情角色一览", name="海伦", story_group="黑暗时代", description="爱国者的妻子。"),
        CharRef(page="剧情角色一览", name="海伦", story_group="切城", description="另一个海伦。"),
    ]
    stored, _ = archive.insert_records(rows)
    assert stored == 2


def test_inserts_never_replace(archive: Archive):
    """`INSERT OR REPLACE` destroyed the loser of a collision and returned success."""
    first = Voice(page="p/语音记录", subject="p", idx=1, text="原本的台词")
    second = Voice(page="p/语音记录", subject="p", idx=1, text="不该覆盖前一条")
    archive.insert_records([first])
    stored, ignored = archive.insert_records([second])
    assert (stored, ignored) == (0, 1)
    kept = archive.scalar("SELECT text FROM voices WHERE page=? AND idx=1", ("p/语音记录",))
    assert kept == "原本的台词"


def test_raw_text_lives_in_a_separate_file(archive: Archive):
    """The split makes "downstream never reads raw markup" a property of the file layout
    rather than a rule people are asked to remember."""
    archive.put_page("某页", "原始 wikitext", 1, "now")
    archive.commit()
    assert archive.rawcache.exists()
    assert archive.page("某页")["wikitext"] == "原始 wikitext"
    tables = {r[0] for r in archive.db.execute(
        "SELECT name FROM main.sqlite_master WHERE type='table'")}
    assert "pages" not in tables


def test_alias_sources_are_tables_not_a_manifest_blob(archive: Archive):
    """7,412 redirects were being stuffed into a key-value row — 282 KB in the manifest."""
    archive.write_aliases_source({"兔子": "阿米娅"}, {"米兰": ["凯尔希", "米兰 (关卡)"]})
    assert archive.redirects() == {"兔子": "阿米娅"}
    assert archive.disambigs() == {"米兰": ["凯尔希", "米兰 (关卡)"]}
    assert "redirects" not in archive.manifest()


def test_rescope_preserves_discovered_seeds(archive: Archive):
    """A discovered set is populated by `fetch` and known to nothing else. Clearing it on
    re-scope silently lost two story scripts, and they stayed lost."""
    archive.write_seeds({"S1": ["剧情页"]})
    archive.add_seeds("S1D", ["剧情页/data"])
    archive.write_seeds({"S1": ["剧情页", "新剧情"]}, preserve=("S1D",))
    assert archive.seed("S1D") == ["剧情页/data"]
    assert archive.get_meta("seed_counts")["S1D"] == 1


def test_rescope_replaces_enumerated_seeds(archive: Archive):
    archive.write_seeds({"S1": ["旧的"]})
    archive.write_seeds({"S1": ["新的"]})
    assert archive.seed("S1") == ["新的"]


def test_structured_tables_keep_multiple_rows_per_page(archive: Archive):
    """`char_memory` gives one operator several record sets; keying by page drops the
    extras without a word. Measured: 55 rows."""
    archive.write_source_rows({"char_memory": [
        {"page": "暴行", "storySetName": "一"},
        {"page": "暴行", "storySetName": "二"},
        {"page": "暴行", "storySetName": "三"},
    ]})
    assert len(archive.source_rows("char_memory")["暴行"]) == 3


def test_readonly_archive_refuses_to_be_the_thing_it_measures(tmp_path):
    """Probes open the archive read-only so a probe cannot alter what it reports."""
    path = tmp_path / "t.archive"
    with Archive(path) as writable:
        writable.put_page("页", "文本", 1, "now")
    with Archive(path, readonly=True) as ro:
        assert ro.page("页")["wikitext"] == "文本"
