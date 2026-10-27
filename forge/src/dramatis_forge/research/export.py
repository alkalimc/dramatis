"""`forge research pairs`: write the structural training pairs, the held-out structural
evaluation set and the manifest that describes them.

Files (UTF-8 JSONL, one object per line, written to the research directory):

    units.jsonl                 every unit any pair or evaluation query references
    pairs.train.jsonl           structural pairs whose page and persons are all train
    pairs.test.jsonl            the same constructions on test pages
    split.json                  pages and persons per split, seed, share, counts
    eval.structural.test.jsonl  the test pairs in the evaluation-suite query format
    MANIFEST.md                 counts, how negatives were chosen, the folio fingerprint
"""

from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from ..folio import Folio
from ..records import sig
from . import pairs as pairs_mod
from . import split as split_mod
from . import source
from .pairs import Pair, Params

UNITS = "units.jsonl"
TRAIN = "pairs.train.jsonl"
TEST = "pairs.test.jsonl"
SPLIT = "split.json"
EVAL = "eval.structural.test.jsonl"
MANIFEST = "MANIFEST.md"

#: Graded-relevance entries beyond the positive that one evaluation query lists.
MAX_ALSO = 50

HOW = {
    "alias": "other entities with the same name (homonym), then entities whose names "
             "share the most character bigrams with the alias (lookalike), then other entities "
             "named by the same kind of alias, in a stable hash order (same_kind_entity; "
             "the weakest kind, so it fills only what the first two leave)",
    "disambiguation": "the other candidates on the same disambiguation page (sibling)",
    "section": "the other sections of the same page, nearest in document order first "
               "(sibling_section)",
    "speaker": "voice lines: the same speaker's other lines (same_speaker_line); dialogue: "
               "the speaker's lines in other scenes (same_speaker_other_scene) alternating "
               "with other speakers' lines in the same scene (same_scene_other_speaker)",
    "identity": "other people's identity cards, those sharing the most attributes with the "
                "person first, or only the same group when a group attribute is named "
                "(same_group_person)",
}


@dataclass
class Summary:
    outdir: Path
    split: split_mod.Split
    pairs: list[Pair]
    dropped: Counter
    units: int = 0
    paths: list[Path] = field(default_factory=list)

    def counts(self) -> dict[str, dict[str, int]]:
        out: dict[str, dict[str, int]] = {}
        for p in self.pairs:
            out.setdefault(p.family, Counter())[p.split] += 1
        return {f: dict(v) for f, v in out.items()}


def _dedupe(pairs: list[Pair]) -> list[Pair]:
    seen: set[tuple[str, str, str]] = set()
    out = []
    for p in sorted(pairs, key=lambda p: (p.family, p.variant, p.query, p.positive)):
        key = (p.family, p.query, p.positive)
        if key not in seen:
            seen.add(key)
            out.append(p)
    return out


def eval_query(c: source.Corpus, p: Pair) -> dict:
    """A test pair as one query of the evaluation suite (`Query` in the eval crate)."""
    gold = {p.positive: 2}
    for uid in p.also[:MAX_ALSO]:
        gold.setdefault(uid, 1)
    verbatim = any(p.query in c.text_of(uid) for uid in gold)
    return {
        "qid": f"{p.family}:{sig(p.variant, p.query, p.positive)}",
        "family": p.family, "text": p.query,
        "stratum": "verbatim" if verbatim else "paraphrase",
        "note": f"{p.variant}; {len(p.negatives)} structural negative(s)",
        "gold": gold,
        "hard_negatives": [n for n in p.negatives if n not in gold],
    }


def write_lines(path: Path, rows: list[dict]) -> None:
    with path.open("w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(pairs_mod.dumps(row) + "\n")


def read_lines(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as fh:
        return [json.loads(line) for line in fh if line.strip()]


def merge_units(outdir: Path, rows: list[dict]) -> int:
    """Add unit rows to units.jsonl (by id); returns the file's row count."""
    path = outdir / UNITS
    have = {r["id"]: r for r in read_lines(path)} if path.exists() else {}
    for r in rows:
        have[r["id"]] = r
    write_lines(path, [have[k] for k in sorted(have)])
    return len(have)


def run(folio_path: Path, outdir: Path, *, seed: int = split_mod.DEFAULT_SEED,
        share: float = split_mod.DEFAULT_TEST_SHARE, mode: str = split_mod.DEFAULT_MODE,
        params: Params | None = None) -> Summary:
    params = params or Params()
    with Folio(folio_path, readonly=True) as folio:
        c = source.load(folio)
    sp = split_mod.assign(c.page_persons(), seed=seed, share=share, mode=mode)
    done = pairs_mod.assemble(c, pairs_mod.construct(c, params), sp, params)
    pairs = _dedupe(done.pairs)
    train = [p for p in pairs if p.split == "train"]
    test = [p for p in pairs if p.split == "test"]
    evals = [eval_query(c, p) for p in test]

    ref: set[str] = set()
    for p in pairs:
        ref.add(p.positive)
        ref.update(p.negatives)
    for q in evals:
        ref.update(q["gold"])
    units = {uid: pairs_mod.unit_row(c, uid) for uid in ref}
    # The check runs on the rows about to be written, not on the construction's own
    # bookkeeping, so it catches a bug in either.
    split_mod.check(sp, [p.as_dict() for p in train], units)

    outdir.mkdir(parents=True, exist_ok=True)
    (outdir / UNITS).unlink(missing_ok=True)
    merge_units(outdir, [units[u] for u in sorted(units, key=lambda u: c.units[u].ord)])
    write_lines(outdir / TRAIN, [p.as_dict() for p in train])
    write_lines(outdir / TEST, [p.as_dict() for p in test])
    write_lines(outdir / EVAL, sorted(evals, key=lambda q: q["qid"]))
    info = sp.as_dict()
    info["folio"] = {"fingerprint": c.fingerprint, "chunks": c.chunk_count}
    (outdir / SPLIT).write_text(json.dumps(info, ensure_ascii=False, indent=1) + "\n",
                                encoding="utf-8")
    summary = Summary(outdir, sp, pairs, done.dropped, units=len(units))
    (outdir / MANIFEST).write_text(manifest(summary, c, params), encoding="utf-8")
    summary.paths = [outdir / n for n in (UNITS, TRAIN, TEST, EVAL, SPLIT, MANIFEST)]
    return summary


def manifest(s: Summary, c: source.Corpus, params: Params) -> str:
    sp = s.split
    lines = ["# Research export", "",
             "Generated by `forge research pairs`. Do not edit.", "",
             "## Source", "",
             f"- folio fingerprint: `{c.fingerprint}`", f"- folio units: {c.chunk_count}",
             f"- split mode {sp.mode}, seed {sp.seed}, test share {sp.share}",
             f"- pages: {len(sp.train_pages)} train, {len(sp.test_pages)} test",
             f"- persons: {len(sp.train_persons)} train-only, {len(sp.test_persons)} test "
             "(a test person appears in no training pair)",
             f"- units referenced: {s.units}", "",
             "## Pairs per family and split", "",
             "| family | train | test | train refused (test page or person) |",
             "| --- | ---: | ---: | ---: |"]
    counts = s.counts()
    for fam in pairs_mod.FAMILIES:
        n = counts.get(fam, {})
        lines.append(f"| {fam} | {n.get('train', 0)} | {n.get('test', 0)} | "
                     f"{s.dropped.get(fam, 0)} |")
    lines += ["", "## By construction", "", "| family | construction | train | test |",
              "| --- | --- | ---: | ---: |"]
    by: Counter = Counter((p.family, p.variant, p.split) for p in s.pairs)
    for fam, var in sorted({(f, v) for f, v, _ in by}):
        lines.append(f"| {fam} | {var} | {by[(fam, var, 'train')]} | {by[(fam, var, 'test')]} |")
    lines += ["", "## Negatives", "",
              f"At most {params.negatives} per pair, in the order below.", ""]
    lines += [f"- **{fam}**: {HOW[fam]}" for fam in pairs_mod.FAMILIES]
    lines += ["", "| family | split | pairs | without negatives | mean negatives |",
              "| --- | --- | ---: | ---: | ---: |"]
    for fam in pairs_mod.FAMILIES:
        for side in ("train", "test"):
            ps = [p for p in s.pairs if p.family == fam and p.split == side]
            if ps:
                lines.append(f"| {fam} | {side} | {len(ps)} | "
                             f"{sum(1 for p in ps if not p.negatives)} | "
                             f"{sum(len(p.negatives) for p in ps) / len(ps):.1f} |")
    kinds: Counter = Counter((p.family, k) for p in s.pairs for k in p.neg_kinds)
    lines += ["", "Negatives by kind:", ""]
    lines += [f"- {fam} / {k}: {n}" for (fam, k), n in sorted(kinds.items())]
    lines += ["", "## Evaluation set", "",
              f"`{EVAL}` holds the test pairs in the evaluation-suite format "
              f"({counts and sum(n.get('test', 0) for n in counts.values())} queries). "
              "Positive graded 2, other units of the same entity or section graded 1; "
              "negatives are listed as hard negatives. `stratum` is `verbatim` when the "
              "query text occurs in a gold unit.", ""]
    return "\n".join(lines)
