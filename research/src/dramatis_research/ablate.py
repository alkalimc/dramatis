"""The four ablations from the research plan, as a list of runs plus a driver.

  1. drop one supervision source at a time (one run per structural family)
  2. structural versus random negatives
  3. training-data size (nested subsets of one seeded shuffle)
  4. typed units versus fixed windows (evaluated on its own window pool)

`plan` is pure and cheap; `run` trains whatever is missing (a run directory that already holds
a run.json is reused) and then evaluates everything, writing one report for the typed pool and
one for the window pool.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Callable
from pathlib import Path

from . import data, report
from .evaluate import DEFAULT_SYSTEMS, Backends, SystemSpec, evaluate
from .train import RUN_JSON, TrainConfig, make_run_id, train_embed

DEFAULT_SIZES = (1000, 3000, 10000, 30000)
DEFAULT_WINDOW = 400


@dataclasses.dataclass
class Entry:
    ablation: str  # which of the four this belongs to ("main" for the reference systems)
    system: str  # name shown in the report
    cfg: TrainConfig
    pool_window: int  # 0 = typed pool, N = window pool of N characters


def structural_families(root: Path) -> list[str]:
    return sorted({r.get("family", "unknown") for r in data.read_jsonl(root / data.PAIRS_TRAIN)})


def plan(
    base: TrainConfig,
    *,
    sizes: tuple[int, ...] = DEFAULT_SIZES,
    window: int = DEFAULT_WINDOW,
    families: list[str] | None = None,
) -> list[Entry]:
    root = base.root
    has_synth = (root / data.SYNTH_TRAIN).exists()
    fams = families if families is not None else structural_families(root)

    def cfg(**kw) -> TrainConfig:
        return dataclasses.replace(base, run_id="", **kw)

    entries = [Entry("main", "ft-structural", cfg(pairs=data.STRUCTURAL), 0)]
    if has_synth:
        entries += [
            Entry("main", "ft-synthetic", cfg(pairs=data.SYNTHETIC), 0),
            Entry("main", "ft-both", cfg(pairs=data.BOTH), 0),
        ]
    for fam in fams:
        entries.append(
            Entry("drop-source", f"ft-structural-drop-{fam}", cfg(drop_families=[fam]), 0)
        )
    entries.append(Entry("negatives", "ft-structural-random-neg", cfg(negatives="random"), 0))
    if has_synth:
        entries.append(
            Entry("negatives", "ft-both-random-neg", cfg(pairs=data.BOTH, negatives="random"), 0)
        )
    for n in sizes:
        entries.append(Entry("size", f"ft-structural-n{n}", cfg(limit=n), 0))
    entries.append(Entry("windows", f"ft-structural-win{window}", cfg(window=window), window))
    return entries


def run(
    base: TrainConfig,
    backends_for: Callable[[Path], Backends],
    *,
    entries: list[Entry] | None = None,
    train: Callable[[TrainConfig], Path] = train_embed,
    out_dir: Path | None = None,
    **eval_kwargs,
) -> dict[str, Path]:
    entries = entries if entries is not None else plan(base)
    runs_dir = base.resolved_runs_dir()
    for e in entries:
        if not (runs_dir / make_run_id(e.cfg) / RUN_JSON).exists():
            train(e.cfg)

    out_dir = out_dir or runs_dir
    written: dict[str, Path] = {}
    typed = [e for e in entries if e.pool_window == 0]
    windowed = [e for e in entries if e.pool_window]
    backends = backends_for(runs_dir)

    def specs(items: list[Entry]) -> list[SystemSpec]:
        return [SystemSpec(e.system, make_run_id(e.cfg)) for e in items]

    results = evaluate(base.root, backends, [*DEFAULT_SYSTEMS, *specs(typed)], **eval_kwargs)
    written["typed"] = report.write(results, out_dir)[0]
    if windowed:
        size = windowed[0].pool_window
        kwargs = {k: v for k, v in eval_kwargs.items() if k != "window"}
        results = evaluate(
            base.root,
            backends,
            [
                DEFAULT_SYSTEMS[0],
                *specs([e for e in typed if e.system == "ft-structural"]),
                *specs(windowed),
            ],
            window=size,
            **kwargs,
        )
        written["windows"] = report.write(results, out_dir, "RESULTS-research-windows")[0]
    return written
