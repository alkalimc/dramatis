"""The verification set: a small stratified sample of test queries the author checks by hand.

`sample` writes `verify.todo.jsonl`, one query per line with the passage beside it and
empty verdict fields. The author edits the file in place:

    verdict           "ok" (the query is fine as is), "fix" (use `corrected_query`),
                      or "bad" (drop it)
    corrected_query   the replacement text, when the verdict is "fix"
    also_relevant     ids of further units that also answer the query (graded 1)
    note              free text

`read` turns the verified rows into an evaluation set in the suite format. Rows without
a verdict are left out and counted, so a half-finished file still yields a valid set.
"""

from __future__ import annotations

import hashlib
from collections import Counter
from pathlib import Path

from ..records import sig
from . import export

TODO = "verify.todo.jsonl"
VERIFIED = "eval.verified.test.jsonl"
SOURCES = (export.TEST, "pairs.synth.test.jsonl")
VERDICTS = ("ok", "fix", "bad")
#: Characters of the passage shown beside a query.
EXCERPT = 600


class VerifyError(Exception):
    pass


def sample(outdir: Path, *, per_family: int = 10, seed: int = 1) -> tuple[Path, Counter]:
    units = {u["id"]: u for u in export.read_lines(outdir / export.UNITS)}
    pool: dict[str, list[dict]] = {}
    for name in SOURCES:
        path = outdir / name
        if path.exists():
            for p in export.read_lines(path):
                pool.setdefault(p["family"], []).append(p)
    if not pool:
        raise VerifyError(f"no test pairs in {outdir}; run `forge research pairs` first")
    out: list[dict] = []
    for family in sorted(pool):
        ranked = sorted(pool[family], key=lambda p: hashlib.sha256(
            f"{seed}\0{p['query']}\0{p['positive']}".encode()).digest())
        for p in ranked[:per_family]:
            u = units[p["positive"]]
            out.append({
                "id": sig(p["family"], p["query"], p["positive"]),
                "family": p["family"], "query": p["query"], "positive": p["positive"],
                "negatives": p["negatives"], "header": u["header"],
                "excerpt": u["text"][:EXCERPT],
                "verdict": None, "corrected_query": None, "also_relevant": [], "note": "",
            })
    path = outdir / TODO
    export.write_lines(path, out)
    return path, Counter(r["family"] for r in out)


def read(outdir: Path, todo: Path | None = None) -> tuple[Path, Counter]:
    todo = todo or outdir / TODO
    units = {u["id"]: u for u in export.read_lines(outdir / export.UNITS)}
    seen: Counter = Counter()
    queries = []
    for row in export.read_lines(todo):
        verdict = row.get("verdict")
        if verdict is None:
            seen["unverified"] += 1
            continue
        if verdict not in VERDICTS:
            raise VerifyError(f"{row['id']}: verdict {verdict!r} is not one of {VERDICTS}")
        seen[verdict] += 1
        if verdict == "bad":
            continue
        text = row["query"]
        if verdict == "fix":
            text = (row.get("corrected_query") or "").strip()
            if not text:
                raise VerifyError(f"{row['id']}: verdict fix needs corrected_query")
        gold = {row["positive"]: 2}
        for uid in row.get("also_relevant") or []:
            if uid not in units:
                raise VerifyError(f"{row['id']}: also_relevant names unknown unit {uid}")
            gold.setdefault(uid, 1)
        verbatim = any(text in f"{units[u]['header']}\n{units[u]['text']}" for u in gold)
        queries.append({
            "qid": f"verified:{row['id']}", "family": row["family"], "text": text,
            "stratum": "verbatim" if verbatim else "paraphrase",
            "note": f"verified by hand ({verdict})",
            "gold": gold, "hard_negatives": [n for n in row["negatives"] if n not in gold],
        })
    path = outdir / VERIFIED
    export.write_lines(path, sorted(queries, key=lambda q: q["qid"]))
    return path, seen
