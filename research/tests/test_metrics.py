import math

import pytest

from dramatis_research import metrics
from dramatis_research.metrics import Query


def q():
    return Query("q", "fam", "text", {"a": 2, "b": 1}, ["n"])


def test_ndcg_hand_computed():
    # DCG = 1/log2(3) (b at rank 2) + 3/log2(4) (a at rank 3); ideal = 3 + 1/log2(3)
    got = metrics.ndcg(q(), ["n", "b", "a", "x"])
    assert got == pytest.approx((1 / math.log2(3) + 1.5) / (3 + 1 / math.log2(3)))
    assert got == pytest.approx(0.586887, abs=1e-5)


def test_perfect_ranking_is_one():
    assert metrics.ndcg(q(), ["a", "b"]) == pytest.approx(1.0)


def test_mrr_recall_and_negative_win():
    ranked = ["n", "b", "a"]
    assert metrics.reciprocal_rank(q(), ranked) == 0.5
    assert metrics.recall(q(), ranked, 10) == 1.0
    assert metrics.recall(q(), ranked, 2) == 0.5
    assert metrics.negative_won(q(), ranked)
    assert not metrics.negative_won(q(), ["a", "n", "b"])
    # a negative with no relevant unit retrieved at all counts as a win
    assert metrics.negative_won(q(), ["x", "n"])
    assert not metrics.negative_won(q(), ["x", "y"])


def test_metrics_respect_the_cutoff():
    ranked = ["x"] * 10 + ["a"]
    assert metrics.ndcg(q(), ranked, 10) == 0.0
    assert metrics.reciprocal_rank(q(), ranked) == 0.0
    assert metrics.recall(q(), ranked, 50) == 0.5


def test_families_are_reported_apart_and_macro_is_equal_weight():
    queries = [
        Query("1", "big", "t", {"a": 2}, ["n"]),
        Query("2", "big", "t", {"a": 2}, ["n"]),
        Query("3", "big", "t", {"a": 2}, ["n"]),
        Query("4", "small", "t", {"a": 2}, []),
    ]
    rankings = [["a"], ["a"], ["a"], ["x"]]
    out = metrics.evaluate_rankings(queries, rankings)
    assert out["by_family"]["big"]["ndcg@10"] == 1.0
    assert out["by_family"]["small"]["ndcg@10"] == 0.0
    # no hard negatives in the small family: reported as absent, not 0%
    assert out["by_family"]["small"]["negwin"] is None
    assert out["macro"]["ndcg@10"] == 0.5  # not 0.75, which a pooled mean would give
    assert out["macro"]["queries"] == 4


def test_load_queries_reads_both_formats(root):
    engine = metrics.load_queries(root / "eval.structural.test.jsonl")
    assert engine[0].gold and engine[0].hard_negatives
    pairs = metrics.load_queries(root / "pairs.test.jsonl")
    assert list(pairs[0].gold.values()) == [2]
