"""Metrics, matching the engine's evaluation crate so numbers are comparable.

nDCG uses exponential gain (2^grade - 1) and a log2 discount; recall is the textbook fraction
of relevant units returned within k; MRR is the reciprocal rank of the first relevant unit
within k; a hard negative "wins" when it outranks every relevant unit in the list. There is
deliberately no pooled mean across families: `macro` averages families with equal weight and
is only ever computed inside one evaluation set.
"""

from __future__ import annotations

import math
from collections import defaultdict
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field

from .data import Pair, read_jsonl

NDCG_K = 10
RECALL_KS = (10, 50)
# Ranked lists are produced at this depth; recall@50 needs the longest.
DEPTH = max(RECALL_KS)


@dataclass
class Query:
    qid: str
    family: str
    text: str
    gold: dict[str, int]
    hard_negatives: list[str] = field(default_factory=list)
    stratum: str = ""


def queries_from_pairs(pairs: Iterable[Pair], prefix: str = "q") -> list[Query]:
    """Held-out pairs as graded queries: the positive is grade 2, negatives are the
    confusable units."""
    out = []
    for i, p in enumerate(pairs):
        out.append(Query(f"{prefix}:{i}", p.family, p.query, {p.positive: 2}, list(p.negatives)))
    return out


def load_queries(path) -> list[Query]:
    """Read either engine-suite rows (`gold` map) or pair rows (`positive` id)."""
    out = []
    for i, r in enumerate(read_jsonl(path)):
        if "gold" in r:
            out.append(
                Query(
                    r.get("qid", f"q:{i}"),
                    r.get("family", "unknown"),
                    r.get("text") or r["query"],
                    {k: int(v) for k, v in r["gold"].items()},
                    list(r.get("hard_negatives", [])),
                    r.get("stratum", ""),
                )
            )
        else:
            out.append(
                Query(
                    f"q:{i}",
                    r.get("family", "unknown"),
                    r["query"],
                    {r["positive"]: 2},
                    list(r.get("negatives", [])),
                )
            )
    return out


def _gain(grade: int) -> float:
    return float((1 << grade) - 1)


def _discount(rank: int) -> float:
    return math.log2(rank + 2)


def ndcg(query: Query, ranked: Sequence[str], k: int = NDCG_K) -> float:
    ideal = sorted(query.gold.values(), reverse=True)[:k]
    denom = sum(_gain(g) / _discount(i) for i, g in enumerate(ideal))
    if denom <= 0:
        return 0.0
    dcg = sum(
        _gain(query.gold[uid]) / _discount(i)
        for i, uid in enumerate(ranked[:k])
        if uid in query.gold
    )
    return dcg / denom


def recall(query: Query, ranked: Sequence[str], k: int) -> float:
    if not query.gold:
        return 0.0
    return sum(1 for uid in ranked[:k] if uid in query.gold) / len(query.gold)


def reciprocal_rank(query: Query, ranked: Sequence[str], k: int = NDCG_K) -> float:
    for i, uid in enumerate(ranked[:k]):
        if uid in query.gold:
            return 1.0 / (i + 1)
    return 0.0


def negative_won(query: Query, ranked: Sequence[str], k: int = NDCG_K) -> bool:
    negs = set(query.hard_negatives)
    first_rel = first_neg = None
    for i, uid in enumerate(ranked[:k]):
        if uid in query.gold:
            first_rel = i if first_rel is None else first_rel
        elif uid in negs:
            first_neg = i if first_neg is None else first_neg
    if first_neg is None:
        return False
    return first_rel is None or first_neg < first_rel


@dataclass
class Scored:
    family: str
    ndcg: float
    recall: dict[int, float]
    mrr: float
    negative_won: bool
    has_negatives: bool


def score(query: Query, ranked: Sequence[str]) -> Scored:
    return Scored(
        query.family,
        ndcg(query, ranked),
        {k: recall(query, ranked, k) for k in RECALL_KS},
        reciprocal_rank(query, ranked),
        negative_won(query, ranked),
        bool(query.hard_negatives),
    )


def aggregate(scored: Sequence[Scored]) -> dict:
    n = len(scored)
    if n == 0:
        return {"queries": 0}
    with_neg = [s for s in scored if s.has_negatives]
    out = {
        "queries": n,
        f"ndcg@{NDCG_K}": sum(s.ndcg for s in scored) / n,
        "mrr": sum(s.mrr for s in scored) / n,
        # Over queries that have hard negatives; None when the family has none, so the
        # report never shows a misleading 0%.
        "negwin": (sum(s.negative_won for s in with_neg) / len(with_neg)) if with_neg else None,
    }
    for k in RECALL_KS:
        out[f"recall@{k}"] = sum(s.recall[k] for s in scored) / n
    return out


def by_family(scored: Sequence[Scored]) -> dict[str, dict]:
    groups: dict[str, list[Scored]] = defaultdict(list)
    for s in scored:
        groups[s.family].append(s)
    return {fam: aggregate(items) for fam, items in sorted(groups.items())}


def macro(per_family: dict[str, dict]) -> dict:
    """Equal-weight mean over families, within one evaluation set."""
    fams = [a for a in per_family.values() if a.get("queries")]
    if not fams:
        return {"queries": 0}
    out: dict = {"queries": sum(a["queries"] for a in fams), "families": len(fams)}
    for key in fams[0]:
        if key == "queries":
            continue
        vals = [a[key] for a in fams if a.get(key) is not None]
        out[key] = sum(vals) / len(vals) if vals else None
    return out


def evaluate_rankings(queries: Sequence[Query], rankings: Sequence[Sequence[str]]) -> dict:
    scored = [score(q, r) for q, r in zip(queries, rankings, strict=True)]
    fams = by_family(scored)
    return {"by_family": fams, "macro": macro(fams), "queries": len(scored)}
