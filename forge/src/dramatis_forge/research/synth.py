"""`forge research synth`: LLM-written queries for the units of one split, filtered.

The control for the structural supervision: the same kind of (query, unit) pair, but the
query is written by a model from the unit's text. Two filters keep it honest:

* a query whose gold unit BM25 cannot find in the top `top_k` is dropped. It would be
  noise in training and an unanswerable question in the held-out set.
* a query that copies the unit (its longest verbatim run covers `copy_ratio` of the
  query) is dropped. BM25 would solve it and it would teach nothing about paraphrase.

Negatives are the BM25 hits that are not the gold unit, the standard mined negative. On
the training side they pass the same split rule as structural pairs.

Calls are cached per (model, prompt) in `synth.cache.jsonl`, so a rerun or a changed
filter costs no new request. Only the model call runs off the main thread: the folio's
sqlite connection stays on the thread that opened it.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
from collections import Counter
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any

from .. import segment
from ..folio import Folio
from . import export, pairs as pairs_mod
from . import split as split_mod
from .source import Corpus
from .source import load as load_corpus

FAMILY = "synthetic"
CACHE = "synth.cache.jsonl"
SCHEMA_NAME = "synthetic_queries"
SCHEMA = {
    "type": "object",
    "properties": {"queries": {"type": "array", "items": {"type": "string"}}},
    "required": ["queries"],
    "additionalProperties": False,
}
SYSTEM = (
    "You write search queries for a knowledge base built from a wiki. You are given one "
    "passage. Write {n} different queries a reader might type when looking for exactly this "
    "passage. Ask about what the passage says or about the people and things it concerns. "
    "Write in the passage's own language. Do not copy sentences or long phrases from the "
    "passage; paraphrase. Vary the form: a question, a short keyword phrase, a loose "
    "description. Each query must be answerable from this passage. Reply with JSON only: "
    '{{"queries": ["..."]}}.'
)


@dataclass(frozen=True)
class Params:
    #: Queries asked for per unit.
    per_unit: int = 3
    #: A query survives only when its unit is in BM25's top `top_k`.
    top_k: int = 10
    #: A query is a copy when its longest verbatim run covers at least this share of it.
    copy_ratio: float = 0.6
    #: Mined negatives kept per pair.
    negatives: int = 8
    #: Characters of the unit sent to the model.
    max_chars: int = 2000
    #: Units shorter than this (header and text) are skipped.
    min_chars: int = 40
    #: A query longer than this is dropped.
    max_query_chars: int = 160
    #: Take at most this many units (0 = all), in a stable hash order.
    limit: int = 0
    #: Restrict to these templates (empty = all).
    templates: tuple[str, ...] = ()


class Bm25:
    """BM25 over the folio's own lexical index, queried the way the engine does: the same
    segmenter the build used, the corpus's stopwords, an OR of the remaining terms."""

    def __init__(self, folio_path: Path) -> None:
        self.db = sqlite3.connect(f"file:{folio_path}?mode=ro", uri=True)
        meta = {k: json.loads(v) for k, v in self.db.execute(
            "SELECT key,value FROM manifest WHERE key IN ('segmenter','stopwords')")}
        name = str(meta.get("segmenter", "")).split("/")[0]
        self.seg = segment.load("jieba" if name == "jieba" else "bigram")
        self.stop = set(meta.get("stopwords") or [])
        self.ids = {o: i for o, i in self.db.execute("SELECT ord,id FROM chunks")}

    def close(self) -> None:
        self.db.close()

    def __call__(self, query: str, k: int) -> list[str]:
        terms = [t for t in self.seg(query).split() if t not in self.stop]
        if not terms:
            return []
        match = " OR ".join('"' + t.replace('"', '""') + '"' for t in dict.fromkeys(terms))
        rows = self.db.execute(
            "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ? "
            "ORDER BY bm25(chunks_fts) LIMIT ?", (match, k)).fetchall()
        return [self.ids[r[0]] for r in rows if r[0] in self.ids]


def copy_share(query: str, text: str) -> float:
    """Share of the query covered by its longest run found verbatim in the text."""
    q, t = query.casefold(), text.casefold()
    if not q:
        return 0.0
    run = SequenceMatcher(None, q, t, autojunk=False).find_longest_match(0, len(q), 0, len(t))
    return run.size / len(q)


def select_units(c: Corpus, sp: split_mod.Split, side: str, params: Params) -> list[str]:
    out = []
    for uid in c.order:
        u = c.units[uid]
        if params.templates and u.template not in params.templates:
            continue
        if len(u.header) + len(u.text) < params.min_chars:
            continue
        if side == "test":
            ok = u.page in sp.test_pages
        else:
            ok = split_mod.clean_for_train(sp, u.page, c.persons_of(uid))
        if ok:
            out.append(uid)
    if params.limit:
        out = sorted(out, key=lambda i: hashlib.sha256(f"synth\0{i}".encode()).digest())
        out = sorted(out[:params.limit], key=lambda i: c.units[i].ord)
    return out


def prompt(c: Corpus, uid: str, params: Params) -> tuple[str, str]:
    u = c.units[uid]
    body = u.text if len(u.text) <= params.max_chars else u.text[:params.max_chars]
    return SYSTEM.format(n=params.per_unit), f"{u.header}\n\n{body}"


def cache_key(model: str, system: str, user: str) -> str:
    return hashlib.sha256(f"{model}\0{system}\0{user}".encode()).hexdigest()[:24]


@dataclass
class Report:
    side: str
    units: int = 0
    calls: int = 0
    cached: int = 0
    failed: int = 0
    generated: int = 0
    dropped: Counter = field(default_factory=Counter)
    kept: int = 0
    usage: dict[str, int] = field(default_factory=lambda: Counter())
    errors: list[str] = field(default_factory=list)
    samples: list[tuple[str, str]] = field(default_factory=list)
    path: Path | None = None


def run(folio_path: Path, outdir: Path, side: str, *, params: Params | None = None,
        client: Any = None, model: str = "", jobs: int = 1, dry_run: bool = False,
        show: int = 2, retrieve: Callable[[str, int], list[str]] | None = None,
        progress: Callable[[str], None] | None = None) -> Report:
    """Generate, filter and write `pairs.synth.<side>.jsonl`. `client.call(system, user,
    schema, name)` is the persona client's; with `dry_run` nothing is called."""
    params = params or Params()
    if side not in ("train", "test"):
        raise ValueError("side must be train or test")
    split_path = outdir / export.SPLIT
    if not split_path.exists():
        raise FileNotFoundError(f"no {export.SPLIT} in {outdir}; run `forge research pairs` first")
    sp = split_mod.load(split_path)
    with Folio(folio_path, readonly=True) as folio:
        c = load_corpus(folio)
    say = progress or (lambda _m: None)
    report = Report(side)
    units = select_units(c, sp, side, params)
    report.units = len(units)
    tasks = []
    for uid in units:
        system, user = prompt(c, uid, params)
        tasks.append((uid, system, user, cache_key(model, system, user)))
    cache_path = outdir / CACHE
    cache: dict[str, list[str]] = {}
    if cache_path.exists():
        cache = {r["key"]: r["queries"] for r in export.read_lines(cache_path)}
    todo = [t for t in tasks if t[3] not in cache]
    report.cached = len(tasks) - len(todo)
    report.calls = len(todo)
    report.samples = [(t[1], t[2]) for t in (todo or tasks)[:show]]
    if dry_run:
        return report
    if todo and client is None:
        raise ValueError("a client is needed to generate")

    def one(task: tuple[str, str, str, str]):
        try:
            return task, client.call(task[1], task[2], SCHEMA, SCHEMA_NAME), None
        except Exception as exc:  # a failed unit is reported, not fatal to the run
            return task, None, exc

    cache_path.parent.mkdir(parents=True, exist_ok=True)
    window = max(1, jobs) * 4
    with ThreadPoolExecutor(max_workers=max(1, jobs), thread_name_prefix="synth") as pool, \
            cache_path.open("a", encoding="utf-8") as fh:
        for start in range(0, len(todo), window):
            for task, reply, err in pool.map(one, todo[start:start + window]):
                if err is not None or reply is None:
                    report.failed += 1
                    report.errors.append(f"{task[0]}: {err}")
                    usage = getattr(err, "usage", None)
                else:
                    queries = reply.output.get("queries")
                    if not isinstance(queries, list):
                        report.failed += 1
                        report.errors.append(f"{task[0]}: reply has no `queries` list")
                        continue
                    queries = [q for q in queries if isinstance(q, str)]
                    cache[task[3]] = queries
                    fh.write(json.dumps({"key": task[3], "unit": task[0], "queries": queries},
                                        ensure_ascii=False) + "\n")
                    fh.flush()
                    usage = reply.usage + reply.wasted
                if usage is not None:
                    for k, v in usage.as_dict().items():
                        report.usage[k] += v
            say(f"{min(start + window, len(todo))}/{len(todo)} units")

    own = None
    if retrieve is None:
        own = Bm25(folio_path)
        retrieve = own
    seen: set[str] = set()
    rows: list[pairs_mod.Pair] = []
    for uid, _, _, key in tasks:
        for q in cache.get(key, []):
            report.generated += 1
            text = " ".join(q.split())
            if not text or len(text) > params.max_query_chars:
                report.dropped["empty_or_long"] += 1
            elif text in seen:
                report.dropped["duplicate"] += 1
            elif copy_share(text, c.text_of(uid)) >= params.copy_ratio:
                report.dropped["copy"] += 1
            else:
                hits = retrieve(text, params.top_k)
                if uid not in hits:
                    report.dropped["not_retrievable"] += 1
                    continue
                seen.add(text)
                negs = [h for h in hits if h != uid and (
                    side == "test" or split_mod.clean_for_train(
                        sp, c.units[h].page, c.persons_of(h)))][:params.negatives]
                who = set(c.persons_of(uid))
                for n in negs:
                    who.update(c.persons_of(n))
                rows.append(pairs_mod.Pair(text, uid, negs, FAMILY, side, c.units[uid].page,
                                           sorted(who), "llm", ["bm25_hit"] * len(negs)))
    if own is not None:
        own.close()
    report.kept = len(rows)
    needed = {r.positive for r in rows} | {n for r in rows for n in r.negatives}
    unit_rows = {u: pairs_mod.unit_row(c, u) for u in needed}
    if side == "train":
        split_mod.check(sp, [r.as_dict() for r in rows], unit_rows)
    path = outdir / f"pairs.synth.{side}.jsonl"
    export.write_lines(path, [r.as_dict() for r in rows])
    export.merge_units(outdir, [unit_rows[u] for u in sorted(unit_rows)])
    report.path = path
    return report
