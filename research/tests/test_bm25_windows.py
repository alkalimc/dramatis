from dramatis_research import data
from dramatis_research.bm25 import BM25, tokenize
from dramatis_research.metrics import Query
from dramatis_research.windows import build_windows, write_windows


def test_tokenize_splits_unspaced_scripts_into_characters_and_pairs():
    assert tokenize("Hello, World 1") == ["hello", "world", "1"]
    han = "\u4e00\u4e8c\u4e09"
    assert tokenize(han) == list(han) + [han[:2], han[1:]]


def test_bm25_ranks_the_matching_document_first():
    docs = {"a": "red fox jumps", "b": "blue whale swims", "c": "green fox sleeps long long"}
    bm = BM25(list(docs), list(docs.values()))
    assert bm.rank("whale", 3) == ["b"]
    assert bm.rank("fox", 3)[0] == "a"  # shorter document wins on length normalisation
    assert bm.rank("absent", 3) == []


def test_windows_are_fixed_length_and_cover_the_page(root):
    units = data.load_units(root)
    w = build_windows(units, 40)
    page_len = sum(len(u.text) for u in units.values() if u.page == "alpha") + 2
    mine = [x for x in w.windows.values() if x.page == "alpha"]
    assert all(len(x.text) == 40 for x in mine[:-1])
    assert "".join(x.text for x in mine) == "\n".join(
        u.text for u in units.values() if u.page == "alpha"
    )
    assert len(mine) == -(-page_len // 40)
    assert all(x.template == "window" and x.header == "" for x in mine)


def test_every_unit_maps_to_overlapping_windows_on_its_own_page(root):
    units = data.load_units(root)
    w = build_windows(units, 40)
    for uid, u in units.items():
        assert w.spans[uid]
        for wid in w.spans[uid]:
            assert w.windows[wid].page == u.page
            assert set(u.persons) <= set(w.windows[wid].persons)


def test_remap_query_marks_overlapping_windows_as_gold(root):
    units = data.load_units(root)
    w = build_windows(units, 40)
    q = Query("q", "f", "t", {"lore:foxtrot0": 2}, ["line:foxtrot2"])
    m = w.remap_query(q)
    assert set(m.gold) == set(w.spans["lore:foxtrot0"])
    assert not set(m.hard_negatives) & set(m.gold)
    assert w.remap_query(Query("q", "f", "t", {"ghost": 2})) is None


def test_window_file_roundtrip(root, tmp_path):
    w = build_windows(data.load_units(root), 60)
    out = tmp_path / "w.jsonl"
    assert write_windows(w, out) == len(w.windows)
    assert len(data.units_from_rows(data.read_jsonl(out))) == len(w.windows)


def test_windowed_training_selection_uses_windows_and_still_checks_leaks(root):
    units = data.load_units(root)
    w = build_windows(units, 60)
    sel = data.select_training_pairs(root, units, windowing=w)
    assert sel.pairs[0].positive in w.windows
    assert all(n in w.windows for p in sel.pairs for n in p.negatives)
    # a negative window never overlaps the positive unit
    for p in sel.pairs:
        assert p.positive not in p.negatives
