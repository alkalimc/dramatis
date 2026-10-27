"""Scoring systems on the held-out sets, each set reported on its own.

Sets: `structural` (the construction applied to test pages), `synthetic` (LLM-written queries
on test pages) and `verify` (author-checked queries). Their scores are never merged. All
systems rank the same pool of units, so rows within a set are directly comparable; the
engine's own lexical numbers are carried as an extra reference row for the structural set
(its pool is the full index, so it is a cross-check rather than a peer).
"""

from __future__ import annotations

import json
import subprocess
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

import numpy as np

from . import data, metrics, models
from .bm25 import BM25
from .encoders import Embedder, Reranker
from .metrics import DEPTH, Query
from .train import load_run
from .windows import Windowing, build_windows

BASE = "base"
NONE = "none"
SET_NAMES = ("structural", "synthetic", "verify")


@dataclass(frozen=True)
class SystemSpec:
    name: str
    embed: str | None  # None = lexical; "base" or a run id
    rerank: str | None = None  # None, "base" or a run id

    @staticmethod
    def parse(text: str) -> SystemSpec:
        """`NAME=EMBED[,RERANK]` where each part is `base`, `none` or a run id."""
        name, _, rest = text.partition("=")
        if not rest:
            raise ValueError(f"system must look like NAME=EMBED[,RERANK], got {text!r}")
        embed, _, rerank = rest.partition(",")

        def norm(v: str) -> str | None:
            return None if v in ("", NONE) else v

        return SystemSpec(name, norm(embed), norm(rerank))


DEFAULT_SYSTEMS = (
    SystemSpec("base-embed", BASE),
    SystemSpec("base-embed+rerank", BASE, BASE),
)


class Backends:
    """Where embedders and rerankers come from. The CLI uses real models; tests inject
    stand-ins."""

    def __init__(
        self,
        make_embedder: Callable[[str, str], Embedder],
        make_reranker: Callable[[str], Reranker],
        runs_dir: Path,
    ):
        self._make_embedder = make_embedder
        self._make_reranker = make_reranker
        self.runs_dir = runs_dir
        self._embedders: dict[str, Embedder] = {}
        self._rerankers: dict[str, Reranker] = {}

    def embedder(self, spec: str) -> Embedder:
        if spec not in self._embedders:
            path, instruction = self._resolve(spec, models.EMBED_MODEL)
            self._embedders[spec] = self._make_embedder(path, instruction)
        return self._embedders[spec]

    def reranker(self, spec: str) -> Reranker:
        if spec not in self._rerankers:
            path, _ = self._resolve(spec, models.RERANK_MODEL)
            self._rerankers[spec] = self._make_reranker(path)
        return self._rerankers[spec]

    def _resolve(self, spec: str, base_model: str) -> tuple[str, str]:
        if spec == BASE:
            return base_model, models.QUERY_INSTRUCTION
        run = load_run(self.runs_dir, spec)
        instruction = run["config"].get("query_instruction", models.QUERY_INSTRUCTION)
        return str(self.runs_dir / spec / run["model_dir"]), instruction


def load_sets(root: Path, units: dict[str, data.Unit]) -> dict[str, list[Query]]:
    """The held-out query sets that exist under `root`. The structural set prefers the
    engine-format file and falls back to the held-out pair file."""
    sets: dict[str, list[Query]] = {}
    if (root / data.EVAL_STRUCTURAL).exists():
        sets["structural"] = metrics.load_queries(root / data.EVAL_STRUCTURAL)
    elif (root / data.PAIRS_TEST).exists():
        sets["structural"] = metrics.load_queries(root / data.PAIRS_TEST)
    if (root / data.SYNTH_TEST).exists():
        sets["synthetic"] = metrics.load_queries(root / data.SYNTH_TEST)
    if (root / data.EVAL_VERIFY).exists():
        sets["verify"] = metrics.load_queries(root / data.EVAL_VERIFY)
    return sets


def subsample(queries: Sequence[Query], limit: int) -> list[Query]:
    """Every n-th query within each family, so small families survive (0 keeps all)."""
    if limit <= 0 or len(queries) <= limit:
        return list(queries)
    by_family: dict[str, list[Query]] = {}
    for q in queries:
        by_family.setdefault(q.family, []).append(q)
    per = max(1, limit // len(by_family))
    out: list[Query] = []
    for items in by_family.values():
        step = max(1, len(items) // per)
        out += items[::step][:per]
    return out


def top_k(sims: np.ndarray, depth: int) -> np.ndarray:
    depth = min(depth, sims.shape[1])
    part = np.argpartition(-sims, depth - 1, axis=1)[:, :depth]
    rows = np.arange(sims.shape[0])[:, None]
    order = np.argsort(-sims[rows, part], axis=1, kind="stable")
    return part[rows, order]


def dense_rankings(
    embedder: Embedder, pool_ids: Sequence[str], doc_matrix: np.ndarray, queries: Sequence[Query]
) -> list[list[str]]:
    q = embedder.encode_queries([x.text for x in queries])
    out: list[list[str]] = []
    for start in range(0, len(queries), 512):
        sims = q[start : start + 512] @ doc_matrix.T
        out += [[pool_ids[j] for j in row] for row in top_k(sims, DEPTH)]
    return out


def rerank_rankings(
    reranker: Reranker,
    rankings: list[list[str]],
    queries: Sequence[Query],
    docs: dict[str, str],
    depth: int,
) -> list[list[str]]:
    """Reorder the first `depth` hits of each list; the tail keeps its original order."""
    out = []
    for q, ranked in zip(queries, rankings, strict=True):
        head, tail = ranked[:depth], ranked[depth:]
        scores = reranker.score_pairs([(q.text, docs[uid]) for uid in head])
        order = sorted(range(len(head)), key=lambda i: (-scores[i], i))
        out.append([head[i] for i in order] + tail)
    return out


def engine_reference(path: Path) -> dict:
    """Map `dramatis-cli evaluate --json` output to this module's result shape."""
    raw = json.loads(path.read_text())

    def conv(a: dict) -> dict:
        return {
            "queries": a["queries"],
            "ndcg@10": a["ndcg"],
            "recall@10": a["recall"],
            "recall@50": None,
            "mrr": a["mrr"],
            "negwin": a["negative_win_rate"],
        }

    fams = {k: conv(v) for k, v in raw["by_family"].items()}
    return {"by_family": fams, "macro": conv(raw["macro_average"]), "queries": raw["scored"]}


def run_engine_bm25(cli: Path, folio: Path, suite: Path, out_json: Path, k: int = 10) -> Path:
    """Call the engine's lexical evaluation; its JSON is read back by `engine_reference`."""
    subprocess.run(
        [
            str(cli),
            "--folio",
            str(folio),
            "evaluate",
            "--suite",
            str(suite),
            "--k",
            str(k),
            "--json",
            str(out_json),
        ],
        check=True,
        capture_output=True,
    )
    return out_json


def system_cost(runs_dir: Path, spec: SystemSpec) -> dict:
    """Cost columns: LLM calls spent on the training data and accelerator hours of training."""
    gpu = 0.0
    calls: int | None = 0
    for part in (spec.embed, spec.rerank):
        if part in (None, BASE):
            continue
        run = load_run(runs_dir, part)
        gpu += run.get("train.gpu_hours", 0.0)
        c = run["data"].get("llm_calls_for_data")
        calls = None if (c is None or calls is None) else calls + c
    return {"llm_calls_for_data": calls, "gpu_hours": gpu}


def evaluate(
    root: Path,
    backends: Backends,
    systems: Sequence[SystemSpec],
    *,
    window: int = 0,
    rerank_depth: int = 20,
    max_queries: int = 0,
    engine_json: Path | None = None,
    lexical_floor: float = 0.8,
) -> dict:
    units = data.load_units(root)
    windowing: Windowing | None = build_windows(units, window) if window else None
    pool = windowing.windows if windowing else units
    pool_ids = list(pool)
    docs = {uid: u.doc for uid, u in pool.items()}

    sets = load_sets(root, units)
    dropped: dict[str, int] = {}
    if windowing:
        for name, qs in sets.items():
            mapped = [windowing.remap_query(q) for q in qs]
            sets[name] = [q for q in mapped if q is not None]
            dropped[name] = len(qs) - len(sets[name])
    sets = {name: subsample(qs, max_queries) for name, qs in sets.items()}

    bm25 = BM25(pool_ids, [docs[i] for i in pool_ids])
    doc_cache: dict[str, np.ndarray] = {}
    result_sets: dict[str, dict] = {}
    all_specs = [SystemSpec("bm25", None), *systems]

    for name, queries in sets.items():
        block: dict[str, dict] = {}
        for spec in all_specs:
            if spec.embed is None:
                rankings = [bm25.rank(q.text, DEPTH) for q in queries]
            else:
                emb = backends.embedder(spec.embed)
                if spec.embed not in doc_cache:
                    doc_cache[spec.embed] = emb.encode_docs([docs[i] for i in pool_ids])
                rankings = dense_rankings(emb, pool_ids, doc_cache[spec.embed], queries)
                if spec.rerank:
                    rankings = rerank_rankings(
                        backends.reranker(spec.rerank), rankings, queries, docs, rerank_depth
                    )
            block[spec.name] = metrics.evaluate_rankings(queries, rankings)
        if name == "structural" and engine_json is not None:
            block["bm25-engine"] = engine_reference(engine_json)
        fams = sorted({q.family for q in queries})
        result_sets[name] = {"queries": len(queries), "families": fams, "systems": block}

    return {
        "meta": {
            "created": datetime.now(UTC).isoformat(timespec="seconds"),
            "pool": "windows" if windowing else "typed units",
            "window": window,
            "pool_size": len(pool_ids),
            "rerank_depth": rerank_depth,
            "max_queries": max_queries,
            "dropped_by_windowing": dropped,
            "lexical_floor": lexical_floor,
            "inputs": {
                n: data.sha256_file(root / n)
                for n in (
                    data.UNITS,
                    data.EVAL_STRUCTURAL,
                    data.PAIRS_TEST,
                    data.SYNTH_TEST,
                    data.EVAL_VERIFY,
                )
                if (root / n).exists()
            },
        },
        "systems": {
            s.name: {
                "embed": s.embed,
                "rerank": s.rerank,
                "cost": system_cost(backends.runs_dir, s),
            }
            for s in all_specs
        },
        "sets": result_sets,
    }
