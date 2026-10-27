"""Reading the exported units and pairs, enforcing the split, and selecting training data.

The no-leak guarantee lives here: `select_training_pairs` is the only way the trainers get
data, and it refuses (raises `LeakError`) when any pair, positive or negative touches a page
or a person that appears in a held-out set. The exporter is expected to split correctly;
this module does not trust it.
"""

from __future__ import annotations

import hashlib
import json
import random
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .windows import Windowing

STRUCTURAL = "structural"
SYNTHETIC = "synthetic"
BOTH = "both"
PAIR_SOURCES = (STRUCTURAL, SYNTHETIC, BOTH)
NEGATIVE_MODES = (STRUCTURAL, "random")

# File names inside the research root. Kept in one place so the trainers, the evaluator
# and the tests agree.
UNITS = "units.jsonl"
PAIRS_TRAIN = "pairs.train.jsonl"
PAIRS_TEST = "pairs.test.jsonl"
SYNTH_TRAIN = "pairs.synth.train.jsonl"
SYNTH_TEST = "pairs.synth.test.jsonl"
EVAL_STRUCTURAL = "eval.structural.test.jsonl"
EVAL_VERIFY = "eval.verify.jsonl"
COST = "cost.json"
SPLIT = "split.json"

# Family label carried by synthetic pairs when the exporter does not set one.
SYNTH_FAMILY = "synthetic"


class LeakError(RuntimeError):
    """Training data touches a held-out page or person."""


@dataclass(frozen=True)
class Unit:
    id: str
    header: str
    text: str
    template: str
    page: str
    persons: tuple[str, ...]

    @property
    def doc(self) -> str:
        """What a model sees for this unit: the header (if any) above the text."""
        return f"{self.header}\n{self.text}" if self.header else self.text


@dataclass(frozen=True)
class Pair:
    query: str
    positive: str
    negatives: tuple[str, ...]
    family: str
    split: str
    page: str
    persons: tuple[str, ...]
    source: str = STRUCTURAL


@dataclass
class TrainExample:
    """One training example with the texts already resolved."""

    query: str
    positive: str
    negatives: list[str]
    family: str
    source: str


@dataclass
class Selection:
    """The training data a run actually used, plus the facts needed to audit it."""

    examples: list[TrainExample]
    pairs: list[Pair]
    counts: dict[str, int] = field(default_factory=dict)
    digest: str = ""
    person_source: str = ""


def read_jsonl(path: Path) -> list[dict]:
    rows = []
    with path.open(encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


def load_units(root: Path) -> dict[str, Unit]:
    return units_from_rows(read_jsonl(root / UNITS))


def units_from_rows(rows: Iterable[dict]) -> dict[str, Unit]:
    units = {}
    for r in rows:
        units[r["id"]] = Unit(
            id=r["id"],
            header=r.get("header", ""),
            text=r["text"],
            template=r.get("template", ""),
            page=r.get("page", ""),
            persons=tuple(r.get("persons", ())),
        )
    return units


def pair_from_row(r: dict, source: str) -> Pair:
    return Pair(
        query=r["query"],
        positive=r["positive"],
        negatives=tuple(r.get("negatives", ())),
        family=r.get("family") or (SYNTH_FAMILY if source == SYNTHETIC else "unknown"),
        split=r.get("split", ""),
        page=r.get("page", ""),
        persons=tuple(r.get("persons", ())),
        source=source,
    )


def load_pairs(path: Path, source: str) -> list[Pair]:
    return [pair_from_row(r, source) for r in read_jsonl(path)]


@dataclass(frozen=True)
class Heldout:
    """What must never be seen at training time: whole pages, persons, and individual units
    (the gold and positive units of the held-out sets)."""

    pages: frozenset[str]
    persons: frozenset[str]
    units: frozenset[str] = frozenset()
    # Where the person set came from: "split.json" (declared by the exporter) or "pairs"
    # (derived from the held-out files), recorded in every run.
    person_source: str = "pairs"


def heldout_from(pairs: Iterable[Pair], gold_ids: Iterable[str] = ()) -> Heldout:
    pages: set[str] = set()
    persons: set[str] = set()
    units: set[str] = set(gold_ids)
    for p in pairs:
        if p.page:
            pages.add(p.page)
        persons.update(p.persons)
        units.add(p.positive)
    return Heldout(frozenset(pages), frozenset(persons), frozenset(units))


def load_heldout(root: Path, units: dict[str, Unit]) -> Heldout:
    """Union of every held-out file present under `root`.

    Pages are derived from the held-out pair files, plus any the optional `split.json`
    declares; units are the gold and positive units of every held-out file. (Pages of gold
    units are not used: a shared index page can hold both training and held-out units, and
    the unit-level rule is the one that keeps held-out text out of training.) Persons come from `split.json` (`persons.test`) when it exists, because a
    held-out pair's `persons` field may list a whole page's cast, which includes people who
    are legitimately in training. Without it, every person named by a held-out pair counts
    as held out, which is the strict reading.
    """
    pairs: list[Pair] = []
    gold: list[str] = []
    for name, source in ((PAIRS_TEST, STRUCTURAL), (SYNTH_TEST, SYNTHETIC)):
        if (root / name).exists():
            pairs += load_pairs(root / name, source)
    for name in (EVAL_STRUCTURAL, EVAL_VERIFY):
        if (root / name).exists():
            for r in read_jsonl(root / name):
                if "gold" in r:
                    gold += list(r["gold"])
                elif "positive" in r:
                    pairs.append(pair_from_row(r, STRUCTURAL))
    derived = heldout_from(pairs, gold)
    split_path = root / SPLIT
    if not split_path.exists():
        return derived
    declared = json.loads(split_path.read_text())
    return Heldout(
        derived.pages | frozenset(declared.get("pages", {}).get("test", ())),
        frozenset(declared.get("persons", {}).get("test", ())),
        derived.units,
        "split.json",
    )


def find_leaks(pairs: Sequence[Pair], units: dict[str, Unit], heldout: Heldout) -> list[str]:
    """Human-readable reasons, one per offending pair (empty means clean)."""
    problems = []

    def touches(page: str, persons: Iterable[str]) -> str | None:
        if page and page in heldout.pages:
            return f"page {page!r}"
        for person in persons:
            if person in heldout.persons:
                return f"person {person!r}"
        return None

    for p in pairs:
        if p.split and p.split != "train":
            problems.append(f"split={p.split!r}: {p.query!r}")
            continue
        hit = touches(p.page, p.persons)
        if hit is None:
            for uid in (p.positive, *p.negatives):
                u = units.get(uid)
                if u is None:
                    problems.append(f"unknown unit {uid!r} in pair {p.query!r}")
                    break
                hit = f"held-out unit {uid!r}" if uid in heldout.units else None
                hit = hit or touches(u.page, u.persons)
                if hit:
                    hit = hit if hit.startswith("held-out") else f"{hit} via unit {uid!r}"
                    break
        if hit:
            problems.append(f"{hit}: {p.query!r}")
    return problems


def assert_no_leak(pairs: Sequence[Pair], units: dict[str, Unit], heldout: Heldout) -> None:
    problems = find_leaks(pairs, units, heldout)
    if problems:
        shown = "\n  ".join(problems[:5])
        raise LeakError(
            f"{len(problems)} of {len(pairs)} training pairs touch a held-out page or person:\n"
            f"  {shown}"
        )


def _digest(pairs: Sequence[Pair]) -> str:
    h = hashlib.sha256()
    for p in pairs:
        h.update(json.dumps([p.query, p.positive, list(p.negatives), p.source]).encode())
    return "sha256:" + h.hexdigest()


def random_negatives(
    avoid: set[str], pool: Sequence[str], count: int, rng: random.Random
) -> tuple[str, ...]:
    """`count` units drawn uniformly from the training-side pool, never one in `avoid`."""
    picked: list[str] = []
    seen = set(avoid)
    attempts = 0
    while len(picked) < count and attempts < count * 20 and pool:
        attempts += 1
        uid = pool[rng.randrange(len(pool))]
        if uid not in seen:
            seen.add(uid)
            picked.append(uid)
    return tuple(picked)


def select_training_pairs(
    root: Path,
    units: dict[str, Unit],
    *,
    pairs: str = STRUCTURAL,
    negatives: str = STRUCTURAL,
    limit: int = 0,
    seed: int = 0,
    drop_families: Iterable[str] = (),
    neg_count: int = 1,
    windowing: Windowing | None = None,
) -> Selection:
    """Choose training pairs and resolve them to texts.

    pairs:          which supervision source(s) to use.
    negatives:      keep the exported structural hard negatives, or swap them for random
                    units from the training side (same count per pair).
    limit:          keep at most this many pairs. The order is a seeded shuffle, so a smaller
                    limit is always a subset of a larger one with the same seed (the
                    data-size curve is nested).
    drop_families:  remove structural families, for the per-source ablation.
    neg_count:      hard negatives kept per example (random mode draws the same number).
    windowing:      train on fixed windows instead of typed units: every unit id is mapped to
                    the window that overlaps it most. The leak check still runs on typed units.
    """
    if pairs not in PAIR_SOURCES:
        raise ValueError(f"pairs must be one of {PAIR_SOURCES}, got {pairs!r}")
    if negatives not in NEGATIVE_MODES:
        raise ValueError(f"negatives must be one of {NEGATIVE_MODES}, got {negatives!r}")
    dropped = set(drop_families)

    chosen: list[Pair] = []
    if pairs in (STRUCTURAL, BOTH):
        chosen += [p for p in load_pairs(root / PAIRS_TRAIN, STRUCTURAL) if p.family not in dropped]
    if pairs in (SYNTHETIC, BOTH):
        chosen += load_pairs(root / SYNTH_TRAIN, SYNTHETIC)

    heldout = load_heldout(root, units)
    # Checked on the full candidate list, before any limit, so a truncated run cannot hide a
    # leak that a full run would hit.
    assert_no_leak(chosen, units, heldout)

    # Stable pre-order so the shuffle does not depend on file ordering quirks.
    chosen.sort(key=lambda p: (p.source, p.family, p.query, p.positive))
    random.Random(seed).shuffle(chosen)
    if limit > 0:
        chosen = chosen[:limit]

    rng = random.Random(seed + 1)
    view = windowing.windows if windowing else units
    pool = [
        uid
        for uid, u in sorted(view.items())
        if uid not in heldout.units
        and u.page not in heldout.pages
        and not (set(u.persons) & heldout.persons)
    ]

    examples = []
    final_pairs = []
    for p in chosen:
        if windowing:
            pos_id = windowing.best(p.positive)
            avoid = set(windowing.overlapping(p.positive))
            hard = windowing.map_negatives(p.negatives, avoid)
        else:
            pos_id, avoid, hard = p.positive, {p.positive}, p.negatives
        negs = random_negatives(avoid, pool, neg_count, rng) if negatives == "random" else hard
        final = Pair(p.query, pos_id, tuple(negs), p.family, p.split, p.page, p.persons, p.source)
        final_pairs.append(final)
        # Hard negatives are kept in file order: the exporter lists the closest ones first.
        texts = [view[n].doc for n in negs[:neg_count]]
        examples.append(TrainExample(p.query, view[pos_id].doc, texts, p.family, p.source))

    counts: dict[str, int] = {}
    for p in final_pairs:
        key = p.family if p.source == STRUCTURAL else SYNTH_FAMILY
        counts[key] = counts.get(key, 0) + 1
    return Selection(examples, final_pairs, counts, _digest(final_pairs), heldout.person_source)
