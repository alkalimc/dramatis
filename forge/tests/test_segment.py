import json
from pathlib import Path

import pytest

from dramatis_forge import segment

VECTORS = Path(__file__).parent / "fixtures" / "segment_vectors.json"


def vectors():
    return json.loads(VECTORS.read_text())["vectors"]


@pytest.mark.parametrize("v", vectors(), ids=lambda v: repr(v["in"]))
def test_shared_vectors(v):
    jieba = pytest.importorskip("jieba")
    assert segment.normalise(v["in"], jieba.cut_for_search) == v["tokens"]


def test_jieba_segmenter_records_the_rule_in_its_version():
    pytest.importorskip("jieba")
    seg = segment.load("jieba")
    assert seg.version.endswith("+" + segment.RULE_TAG)
    assert seg("UR-6 Title") == "ur 6 title"


def test_non_ascii_runs_reach_the_cutter_and_punctuation_is_dropped():
    cuts = []

    def cut(run):
        cuts.append(run)
        return [run[:1], "\u2014", run[1:]]

    assert segment.normalise("ab\u00e9\u00e8-cd", cut) == ["ab", "\u00e9", "\u00e8", "cd"]
    assert cuts == ["\u00e9\u00e8"]
