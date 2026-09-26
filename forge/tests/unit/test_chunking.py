"""Chunking: turn-aligned, non-overlapping, header-bearing.

Units do not overlap: context is recovered at query time by widening a hit to its
neighbours, so a sliding window's redundancy buys nothing. Speaker names are synthetic
placeholders; cut points come from a policy, tested with both ASCII and CJK punctuation.
"""

from __future__ import annotations

import pytest

from dramatis_forge.corpus.chunk import _split, _turn_aligned
from dramatis_forge.pack import ChunkPolicy

ASCII = ChunkPolicy(templates=()).breaks
CJK = ("\n\n", "\n", "。", "！", "？", "；", "，", " ")


def rendered(*speakers: str | None) -> list[tuple[int, str | None, str]]:
    return [(i + 1, s, f"{s or '-'}: line {i + 1}") for i, s in enumerate(speakers)]


# ---- boundary rule ----

def test_units_do_not_overlap():
    groups = _turn_aligned(rendered(*(["甲", "乙"] * 20)), target=12, cap=18, absorb=True)
    seqs = [seq for group in groups for seq, _, _ in group]
    assert len(seqs) == len(set(seqs)), "a record appears in two units"


def test_every_record_appears_exactly_once():
    lines = rendered(*(["甲", "乙", "丙"] * 15))
    groups = _turn_aligned(lines, target=12, cap=18, absorb=True)
    assert sorted(seq for g in groups for seq, _, _ in g) == [seq for seq, _, _ in lines]


def test_a_boundary_waits_for_the_speaker_to_finish():
    """Cutting mid-thought is the failure that actually damages a dialogue unit, and a
    fixed window guarantees it at every boundary."""
    # one speaker holds the floor across the target, then hands over
    lines = rendered(*(["甲"] * 14 + ["乙"] * 6))
    groups = _turn_aligned(lines, target=12, cap=18, absorb=True)
    first = groups[0]
    assert len(first) == 14, "should extend past the target to the handover"
    assert first[-1][1] == "甲" and groups[1][0][1] == "乙"


def test_the_cap_stops_an_unbroken_monologue():
    """Without a cap, one speaker talking for 200 lines becomes one enormous unit."""
    groups = _turn_aligned(rendered(*(["甲"] * 40)), target=12, cap=18, absorb=True)
    assert max(len(g) for g in groups) <= 18


def test_absorbing_a_tail_never_breaches_the_cap():
    """Found by this test: 40 lines from one speaker split 18 + 22, because absorption
    ignored the cap it exists to enforce."""
    for n in range(20, 60):
        groups = _turn_aligned(rendered(*(["甲"] * n)), target=12, cap=18, absorb=True)
        assert max(len(g) for g in groups) <= 18, f"{n} lines produced an oversized unit"


def test_a_stub_tail_is_absorbed_not_emitted():
    """A two-line fragment embeds as noise."""
    lines = rendered(*(["甲", "乙"] * 7))  # 14 lines: 12 + a 2-line remainder
    groups = _turn_aligned(lines, target=12, cap=18, absorb=True)
    assert len(groups) == 1
    assert len(groups[0]) == 14


def test_absorb_can_be_turned_off():
    lines = rendered(*(["甲", "乙"] * 7))
    groups = _turn_aligned(lines, target=12, cap=18, absorb=False)
    assert len(groups) == 2


def test_a_short_scene_is_one_unit():
    groups = _turn_aligned(rendered("甲", "乙", "甲"), target=12, cap=18, absorb=True)
    assert len(groups) == 1 and len(groups[0]) == 3


def test_no_input_yields_no_units():
    assert _turn_aligned([], target=12, cap=18, absorb=True) == []


# ---- splitting over-long text ----

def test_split_prefers_a_paragraph_break():
    text = "First part. " * 40 + "\n\n" + "Second part. " * 40
    parts = _split(text, 200, ASCII)
    assert all(not p.startswith("\n") for p in parts)
    assert len(parts) > 1


def test_split_falls_back_through_sentence_boundaries():
    text = "This is a sentence. " * 60
    parts = _split(text, 120, ASCII)
    assert all(p.endswith(".") for p in parts[:-1]), "should cut after a full stop"


def test_split_uses_the_policy_punctuation():
    """Cut points are the pack's; a CJK policy cuts after a CJK full stop."""
    text = "这是一个句子。" * 60
    parts = _split(text, 120, CJK)
    assert all(p.endswith("。") for p in parts[:-1])
    # the ASCII defaults find no boundary in CJK text and fall back to a hard cut
    assert not all(p.endswith("。") for p in _split(text, 120, ASCII)[:-1])


def test_split_leaves_short_text_alone():
    assert _split("Short.", 900, ASCII) == ["Short."]


def test_split_never_emits_empty_parts():
    assert all(p for p in _split("." * 500, 50, ASCII))


@pytest.mark.parametrize("limit", [40, 120, 400])
@pytest.mark.parametrize("text,breaks", [
    ("Item one. Item two. Item three. " * 30, ASCII),
    ("内容甲。内容乙。内容丙。" * 30, CJK),
])
def test_split_is_lossless_ignoring_whitespace(limit, text, breaks):
    joined = "".join(_split(text, limit, breaks))
    assert joined.replace(" ", "") == text.replace(" ", "").replace("\n", "")
