import json

import pytest

from dramatis_research import data
from dramatis_research.data import LeakError

from conftest import TEST_PAGES, TRAIN_PAGES, write_jsonl


def select(root, **kw):
    return data.select_training_pairs(root, data.load_units(root), **kw)


def test_clean_export_loads(root):
    sel = select(root, pairs="both")
    assert len(sel.examples) == 15 + 10
    assert sel.digest.startswith("sha256:")


def test_heldout_covers_pages_and_persons(root):
    units = data.load_units(root)
    held = data.load_heldout(root, units)
    assert held.pages == set(TEST_PAGES)
    assert held.persons == {"dee", "eli"}


def test_refuses_training_pair_on_a_test_page(root):
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["page"] = TEST_PAGES[0]
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError, match="page"):
        select(root)


def test_refuses_training_pair_with_a_test_person(root):
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["persons"] = ["dee"]  # a person who only appears in held-out pages
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError, match="person"):
        select(root)


def test_refuses_when_a_negative_comes_from_a_test_page(root):
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["negatives"] = ["lore:foxtrot0"]
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError, match="held-out"):
        select(root)


def test_refuses_synthetic_pair_in_test_split(root):
    rows = data.read_jsonl(root / "pairs.synth.train.jsonl")
    rows[0]["split"] = "test"
    write_jsonl(root / "pairs.synth.train.jsonl", rows)
    with pytest.raises(LeakError, match="split"):
        select(root, pairs="synthetic")


def test_leak_is_caught_even_when_limit_would_drop_it(root):
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[-1]["page"] = TEST_PAGES[0]
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError):
        select(root, limit=1)


def test_engine_format_gold_units_are_heldout_units(root):
    # A unit that only the engine-format eval set names is still held out, even on a page
    # that also has training pairs.
    (root / "pairs.test.jsonl").unlink()
    (root / "pairs.synth.test.jsonl").unlink()
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["negatives"] = ["lore:foxtrot0"]
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError, match="held-out unit"):
        select(root)


def test_declared_split_defines_persons_when_pairs_list_a_whole_cast(root):
    # Held-out pairs may name everyone on their page. Strictly, "ann" (a training person)
    # would then count as held out and every training pair would be refused.
    rows = data.read_jsonl(root / "pairs.test.jsonl")
    rows[0]["persons"] = ["dee", "ann"]
    write_jsonl(root / "pairs.test.jsonl", rows)
    with pytest.raises(LeakError):
        select(root)
    (root / "split.json").write_text(
        json.dumps({"persons": {"test": ["dee", "eli"]}, "pages": {"test": TEST_PAGES}})
    )
    sel = select(root)
    assert sel.person_source == "split.json" and len(sel.examples) == 15
    # the declared list is still enforced
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["persons"] = ["eli"]
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError, match="person"):
        select(root)


def test_gold_unit_on_a_training_page_is_caught(root):
    # Units are held out individually: a gold unit sitting on a training page is caught when
    # a training pair uses it, without condemning the whole page.
    rows = data.read_jsonl(root / "eval.structural.test.jsonl")
    rows[0]["gold"] = {"lore:alpha0": 1, **rows[0]["gold"]}
    write_jsonl(root / "eval.structural.test.jsonl", rows)
    with pytest.raises(LeakError, match="held-out unit"):
        select(root)


def test_pairs_flag_picks_sources(root):
    assert {e.source for e in select(root, pairs="structural").examples} == {"structural"}
    assert {e.source for e in select(root, pairs="synthetic").examples} == {"synthetic"}
    assert {e.source for e in select(root, pairs="both").examples} == {"structural", "synthetic"}


def test_drop_family_removes_only_that_source(root):
    full = select(root)
    cut = select(root, drop_families=["speaker"])
    assert full.counts["speaker"] == len(TRAIN_PAGES)
    assert "speaker" not in cut.counts
    assert cut.counts["redirect"] == full.counts["redirect"]
    # dropping a structural family leaves synthetic pairs alone
    both = select(root, pairs="both", drop_families=["speaker"])
    assert both.counts["synthetic"] == 10


def test_random_negatives_differ_but_never_leak_or_hit_the_positive(root):
    units = data.load_units(root)
    held = data.load_heldout(root, units)
    s = select(root, negatives="structural", neg_count=2)
    r = select(root, negatives="random", neg_count=2)
    assert [e.query for e in s.examples] == [e.query for e in r.examples]
    assert [e.negatives for e in s.examples] != [e.negatives for e in r.examples]
    by_doc = {u.doc: u for u in units.values()}
    for ex, pair in zip(r.examples, r.pairs, strict=True):
        assert len(ex.negatives) == 2
        assert ex.positive not in ex.negatives
        for neg in ex.negatives:
            assert by_doc[neg].page not in held.pages
        assert pair.positive in units


def test_structural_negatives_are_the_exported_ones(root):
    sel = select(root, neg_count=5)
    for ex, pair in zip(sel.examples, sel.pairs, strict=True):
        assert len(ex.negatives) == len(pair.negatives) == 2


def test_limit_is_nested_and_seeded(root):
    small = select(root, limit=4, seed=3)
    large = select(root, limit=8, seed=3)
    assert len(small.examples) == 4
    key = lambda p: (p.query, p.positive)  # noqa: E731
    assert [key(p) for p in small.pairs] == [key(p) for p in large.pairs][:4]
    other = select(root, limit=4, seed=4)
    assert [key(p) for p in other.pairs] != [key(p) for p in small.pairs]


def test_selection_digest_changes_with_the_data(root):
    assert select(root).digest == select(root).digest
    assert select(root).digest != select(root, negatives="random").digest
    assert select(root).digest != select(root, limit=3).digest


def test_unknown_options_are_rejected(root):
    with pytest.raises(ValueError):
        select(root, pairs="nope")
    with pytest.raises(ValueError):
        select(root, negatives="nope")


def test_missing_unit_reference_is_reported(root):
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["negatives"] = ["ghost"]
    write_jsonl(root / "pairs.train.jsonl", rows)
    with pytest.raises(LeakError, match="unknown unit"):
        select(root)


def test_hashes_are_stable(root):
    assert data.sha256_file(root / "units.jsonl") == data.sha256_file(root / "units.jsonl")
    assert json.loads((root / "units.jsonl").read_text().splitlines()[0])["id"]
