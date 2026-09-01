"""Storage invariants.

Several of these pin down defects that silently destroyed data in an earlier schema. Each
one presented the same way: a pipeline that looked like it worked, reporting counts that
were slightly odd. That is the characteristic failure of a store built on keys that do not
actually identify a row.

Fixtures are deliberately generic. The store knows nothing about any domain, so a test
that asserted its behaviour using one domain's page and character names would imply a
coupling that does not exist — and would put that domain's vocabulary in a repository
that ships no rules. Text stays non-ASCII on purpose: multibyte handling through SQLite,
FTS pre-tokenisation and the raw-cache split is part of what these tests cover.
"""

from __future__ import annotations

from dramatis_forge.normalize.records import CharRef, Lore, Voice
from dramatis_forge.store.archive import Archive


def test_same_address_different_text_coexists(archive: Archive):
    """Keying `lore` by `(page, path)` alone means two paragraphs under one heading
    overwrite each other. Real corpora do this routinely, and the loss is silent."""
    rows = [
        Lore(page="索引页", path=("章", "节"), text="第一段正文。"),
        Lore(page="索引页", path=("章", "节"), text="第二段正文。"),
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
    """One name can be described separately in two story groups. The group is part of
    the identity of the description, not decoration on it."""
    rows = [
        CharRef(page="角色索引", name="甲", story_group="第一部", description="一种描述。"),
        CharRef(page="角色索引", name="甲", story_group="第二部", description="另一种描述。"),
    ]
    stored, _ = archive.insert_records(rows)
    assert stored == 2


def test_inserts_never_replace(archive: Archive):
    """`INSERT OR REPLACE` destroyed the loser of a collision and returned success."""
    first = Voice(page="p/语音", subject="p", idx=1, text="原本的台词")
    second = Voice(page="p/语音", subject="p", idx=1, text="不该覆盖前一条")
    archive.insert_records([first])
    stored, ignored = archive.insert_records([second])
    assert (stored, ignored) == (0, 1)
    kept = archive.scalar("SELECT text FROM voices WHERE page=? AND idx=1", ("p/语音",))
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
    """A large redirect map belongs in a table. Stuffed into a key-value row it becomes
    a manifest measured in hundreds of kilobytes, which nothing can query."""
    archive.write_aliases_source({"别名": "目标"}, {"歧义词": ["候选一", "候选二"]})
    assert archive.redirects() == {"别名": "目标"}
    assert archive.disambigs() == {"歧义词": ["候选一", "候选二"]}
    assert "redirects" not in archive.manifest()


def test_rescope_preserves_discovered_seeds(archive: Archive):
    """A discovered set is populated by `fetch` and known to nothing else. Clearing it on
    re-scope loses pages that no later enumeration can find again, which is why the loss
    persists across runs instead of healing."""
    archive.write_seeds({"S1": ["某页"]})
    archive.add_seeds("S1D", ["某页/data"])
    archive.write_seeds({"S1": ["某页", "另一页"]}, preserve=("S1D",))
    assert archive.seed("S1D") == ["某页/data"]
    assert archive.get_meta("seed_counts")["S1D"] == 1


def test_rescope_replaces_enumerated_seeds(archive: Archive):
    archive.write_seeds({"S1": ["旧的"]})
    archive.write_seeds({"S1": ["新的"]})
    assert archive.seed("S1") == ["新的"]


def test_structured_tables_keep_multiple_rows_per_page(archive: Archive):
    """Some structured tables legitimately give one page several rows. Keying by page
    drops the extras without a word — so the pack has to declare which tables those are,
    and the store has to honour the declaration."""
    archive.write_source_rows({"multi": [
        {"page": "某页", "setName": "一"},
        {"page": "某页", "setName": "二"},
        {"page": "某页", "setName": "三"},
    ]})
    assert len(archive.source_rows("multi")["某页"]) == 3


def test_readonly_archive_refuses_to_be_the_thing_it_measures(tmp_path):
    """Probes open the archive read-only so a probe cannot alter what it reports."""
    path = tmp_path / "t.archive"
    with Archive(path) as writable:
        writable.put_page("页", "文本", 1, "now")
    with Archive(path, readonly=True) as ro:
        assert ro.page("页")["wikitext"] == "文本"
