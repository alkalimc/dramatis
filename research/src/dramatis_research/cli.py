"""Command line: train-embed, train-rerank, evaluate, ablate, windows."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from . import ablate, data, models, windows
from .encoders import HashingEmbedder, OverlapReranker, STEmbedder, YesNoReranker
from .evaluate import DEFAULT_SYSTEMS, Backends, SystemSpec, evaluate, run_engine_bm25
from .report import write
from .train import TrainConfig, train_embed, train_rerank

ROOT_ENV = "DRAMATIS_RESEARCH_ROOT"


def _root(ns) -> Path:
    root = ns.root or os.environ.get(ROOT_ENV)
    if not root:
        sys.exit(f"pass --root <research data dir> or set {ROOT_ENV}")
    return Path(root)


def _add_common(p: argparse.ArgumentParser) -> None:
    p.add_argument("--root", help=f"research data directory (default: ${ROOT_ENV})")
    p.add_argument("--runs-dir", help="where runs are written (default: <root>/runs)")


def _add_train(p: argparse.ArgumentParser, kind: str) -> None:
    _add_common(p)
    default_base = models.EMBED_MODEL if kind == "embed" else models.RERANK_MODEL
    p.add_argument("--base", default=default_base, help="base model id or path")
    p.add_argument("--pairs", choices=data.PAIR_SOURCES, default=data.STRUCTURAL)
    p.add_argument("--negatives", choices=data.NEGATIVE_MODES, default=data.STRUCTURAL)
    p.add_argument("--limit", type=int, default=0, help="keep at most N pairs (0 = all)")
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--drop-family", action="append", default=[], help="repeatable")
    p.add_argument("--window", type=int, default=0, help="train on fixed windows of N characters")
    p.add_argument("--hard-negatives", type=int, default=1, help="negatives per example")
    p.add_argument("--epochs", type=int, default=1)
    p.add_argument("--batch-size", type=int, default=16)
    p.add_argument("--learning-rate", type=float, default=2e-5)
    p.add_argument("--max-seq-length", type=int, default=256)
    p.add_argument("--query-instruction", default=models.QUERY_INSTRUCTION)
    p.add_argument("--lora-rank", type=int, default=0, help="0 = full fine-tuning")
    p.add_argument("--bf16", action="store_true")
    p.add_argument("--gradient-checkpointing", action="store_true")
    p.add_argument("--device", default="", help="default: cuda, then mps, then cpu")
    p.add_argument("--run-id", default="")


def _train_cfg(ns, kind: str) -> TrainConfig:
    return TrainConfig(
        root=_root(ns),
        kind=kind,
        base=ns.base,
        pairs=ns.pairs,
        negatives=ns.negatives,
        limit=ns.limit,
        seed=ns.seed,
        drop_families=ns.drop_family,
        window=ns.window,
        hard_negatives=ns.hard_negatives,
        epochs=ns.epochs,
        batch_size=ns.batch_size,
        learning_rate=ns.learning_rate,
        max_seq_length=ns.max_seq_length,
        query_instruction=ns.query_instruction,
        lora_rank=ns.lora_rank,
        bf16=ns.bf16,
        gradient_checkpointing=ns.gradient_checkpointing,
        device=ns.device,
        run_id=ns.run_id,
        runs_dir=Path(ns.runs_dir) if ns.runs_dir else None,
    )


def _backends(runs_dir: Path, dry: bool, max_seq_length: int, device: str) -> Backends:
    if dry:
        return Backends(lambda p, i: HashingEmbedder(), lambda p: OverlapReranker(), runs_dir)
    return Backends(
        lambda path, instruction: STEmbedder(path, device or None, max_seq_length, instruction),
        lambda path: YesNoReranker(path, device or None, max_seq_length),
        runs_dir,
    )


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(prog="dramatis-research", description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)
    _add_train(sub.add_parser("train-embed", help="fine-tune the embedding model"), "embed")
    _add_train(sub.add_parser("train-rerank", help="fine-tune the reranker"), "rerank")

    ev = sub.add_parser("evaluate", help="score systems on the held-out sets")
    _add_common(ev)
    ev.add_argument(
        "--system",
        action="append",
        default=[],
        help="NAME=EMBED[,RERANK]; each part is base, none or a run id (repeatable). "
        "Default: base-embed and base-embed+rerank. BM25 always runs.",
    )
    ev.add_argument("--window", type=int, default=0, help="score on fixed windows of N characters")
    ev.add_argument("--rerank-depth", type=int, default=20)
    ev.add_argument("--max-queries", type=int, default=0, help="cap per set (0 = all)")
    ev.add_argument("--max-seq-length", type=int, default=512)
    ev.add_argument("--device", default="")
    ev.add_argument(
        "--engine-json", help="`dramatis-cli evaluate --json` output for the structural set"
    )
    ev.add_argument("--engine-cli", help="run this dramatis-cli to produce the engine JSON")
    ev.add_argument("--folio", help="corpus for --engine-cli")
    ev.add_argument("--lexical-floor", type=float, default=0.8)
    ev.add_argument("--dry", action="store_true", help="hashing stand-ins instead of models")
    ev.add_argument("--out-dir", help="default: <runs-dir>")

    ab = sub.add_parser("ablate", help="plan or run the four ablations")
    _add_train(ab, "embed")
    ab.add_argument("action", choices=["plan", "run"])
    ab.add_argument("--sizes", default=",".join(map(str, ablate.DEFAULT_SIZES)))
    ab.add_argument("--window-size", type=int, default=ablate.DEFAULT_WINDOW)
    ab.add_argument("--rerank-depth", type=int, default=20)
    ab.add_argument("--max-queries", type=int, default=0)
    ab.add_argument("--dry", action="store_true")

    wn = sub.add_parser("windows", help="write the fixed-window unit file derived from units")
    _add_common(wn)
    wn.add_argument("--size", type=int, default=ablate.DEFAULT_WINDOW)
    wn.add_argument("--out", help="default: <root>/units.windows<size>.jsonl")

    ns = parser.parse_args(argv)
    if ns.cmd == "train-embed":
        print(train_embed(_train_cfg(ns, "embed")))
    elif ns.cmd == "train-rerank":
        print(train_rerank(_train_cfg(ns, "rerank")))
    elif ns.cmd == "windows":
        root = _root(ns)
        w = windows.build_windows(data.load_units(root), ns.size)
        out = Path(ns.out) if ns.out else root / f"units.windows{ns.size}.jsonl"
        print(f"{windows.write_windows(w, out)} windows -> {out}")
    elif ns.cmd == "ablate":
        cfg = _train_cfg(ns, "embed")
        sizes = tuple(int(x) for x in ns.sizes.split(",") if x)
        entries = ablate.plan(cfg, sizes=sizes, window=ns.window_size)
        if ns.action == "plan":
            for e in entries:
                print(f"{e.ablation:12} {e.system:34} {json.dumps(_diff(e.cfg, cfg))}")
            return
        runs_dir = cfg.resolved_runs_dir()
        written = ablate.run(
            cfg,
            lambda d: _backends(d, ns.dry, 512, cfg.device),
            entries=entries,
            rerank_depth=ns.rerank_depth,
            max_queries=ns.max_queries,
        )
        print(json.dumps({k: str(v) for k, v in written.items()}, indent=2), runs_dir)
    elif ns.cmd == "evaluate":
        root = _root(ns)
        runs_dir = Path(ns.runs_dir) if ns.runs_dir else root / "runs"
        engine_json = Path(ns.engine_json) if ns.engine_json else None
        if ns.engine_cli:
            if not ns.folio:
                sys.exit("--engine-cli needs --folio")
            suite = root / data.EVAL_STRUCTURAL
            engine_json = run_engine_bm25(
                Path(ns.engine_cli), Path(ns.folio), suite, runs_dir / "engine-bm25.json"
            )
        systems = [SystemSpec.parse(s) for s in ns.system] or list(DEFAULT_SYSTEMS)
        results = evaluate(
            root,
            _backends(runs_dir, ns.dry, ns.max_seq_length, ns.device),
            systems,
            window=ns.window,
            rerank_depth=ns.rerank_depth,
            max_queries=ns.max_queries,
            engine_json=engine_json,
            lexical_floor=ns.lexical_floor,
        )
        md, js = write(results, Path(ns.out_dir) if ns.out_dir else runs_dir)
        print(md)
        print(js)


def _diff(cfg: TrainConfig, base: TrainConfig) -> dict:
    out = {}
    for f in ("pairs", "negatives", "limit", "window"):
        if getattr(cfg, f) != getattr(base, f) or f in ("pairs", "negatives"):
            out[f] = getattr(cfg, f)
    if cfg.drop_families:
        out["drop_families"] = cfg.drop_families
    return out


if __name__ == "__main__":
    main()
