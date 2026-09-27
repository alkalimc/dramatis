"""Token sizes of what the runtime puts into a prompt, measured on the built corpus.

A context budget is only as good as its per-piece sizes, and those depend on the corpus,
its language and the model's tokenizer, so they are measured here rather than assumed.
The pack names a reference tokenizer (`Pack.tokenizer`): `hf:<repo id>` (fetched once into
the Hugging Face cache), a local `tokenizer.json`, or a `tiktoken` encoding name. The runtime's endpoint
may tokenize differently; the reference is for sizing budgets, not for billing.

Three measurements, all deterministic for a given corpus:

* per retrieval unit, by builder shape, header included (that is what gets injected);
* per spoken line;
* per lexical top-k retrieval block over a fixed sample of benchmark queries, using the
  same query path as the engine: segment, drop stopwords, OR the quoted tokens, BM25.
"""

from __future__ import annotations

import json
import random
import sqlite3
from collections.abc import Callable
from pathlib import Path

#: Units measured per shape, and benchmark queries per family.
SAMPLE_UNITS = 400
SAMPLE_LINES = 2000
QUERIES_PER_FAMILY = 12
SEED = "tokens"


def load_counter(spec: str, base: Path) -> Callable[[str], int] | None:
    """A token counter for `spec`, or None when neither backend can load it."""
    if not spec:
        return None
    if spec.startswith("hf:"):
        try:
            from tokenizers import Tokenizer
            tok = Tokenizer.from_pretrained(spec[3:])
        except Exception:  # missing backend, no network on first use, unknown repo
            return None
        return lambda s: len(tok.encode(s, add_special_tokens=False).ids)
    path = Path(spec)
    if not path.is_absolute():
        path = base / path
    if path.suffix == ".json" and path.exists():
        try:
            from tokenizers import Tokenizer
        except ImportError:
            return None
        tok = Tokenizer.from_file(str(path))
        return lambda s: len(tok.encode(s, add_special_tokens=False).ids)
    try:
        import tiktoken
    except ImportError:
        return None
    try:
        enc = tiktoken.get_encoding(spec)
    except ValueError:
        return None
    return lambda s: len(enc.encode(s, disallowed_special=()))


def p50(values: list[int]) -> int:
    ordered = sorted(values)
    return ordered[len(ordered) // 2] if ordered else 0


def unit_tokens(folio: sqlite3.Connection, templates: list[str],
                count: Callable[[str], int]) -> int | None:
    """Median tokens of one injected unit (header + body) across `templates`."""
    if not templates:
        return None
    marks = ",".join("?" * len(templates))
    rows = folio.execute(
        f"select header, text from chunks where template in ({marks}) order by ord",
        templates).fetchall()
    if not rows:
        return None
    rng = random.Random(f"{SEED}:{','.join(templates)}")
    picked = rng.sample(rows, min(SAMPLE_UNITS, len(rows)))
    return p50([count(f"{h or ''}\n{t}") for h, t in picked])


def line_tokens(archive: sqlite3.Connection, count: Callable[[str], int]) -> int | None:
    rows = [r[0] for r in archive.execute(
        "select text from lines where speaker is not null and speaker <> '' order by scene, seq")]
    if not rows:
        return None
    rng = random.Random(f"{SEED}:lines")
    return p50([count(t) for t in rng.sample(rows, min(SAMPLE_LINES, len(rows)))])


def _queries(suite: Path) -> list[str]:
    by_family: dict[str, list[str]] = {}
    with suite.open(encoding="utf-8") as fh:
        for line in fh:
            q = json.loads(line)
            by_family.setdefault(q.get("family", ""), []).append(q["text"])
    rng = random.Random(f"{SEED}:queries")
    out: list[str] = []
    for family in sorted(by_family):
        texts = by_family[family]
        out += rng.sample(texts, min(QUERIES_PER_FAMILY, len(texts)))
    return out


def retrieval_tokens(folio: sqlite3.Connection, suite: Path, k: int, *,
                     segment: Callable[[str], str], stopwords: set[str],
                     count: Callable[[str], int]) -> int | None:
    """Median tokens of a lexical top-`k` block, as the engine would inject it."""
    if not suite.exists():
        return None
    sizes: list[int] = []
    for text in _queries(suite):
        tokens = segment(text).split()
        kept = [t for t in tokens if t not in stopwords] or tokens
        if not kept:
            continue
        expr = " OR ".join('"' + t.replace('"', '""') + '"' for t in kept)
        rows = folio.execute(
            "select c.header, c.text from chunks_fts f join chunks c on c.ord = f.rowid "
            "where chunks_fts match ? order by bm25(chunks_fts) limit ?", (expr, k)).fetchall()
        sizes.append(sum(count(f"{h or ''}\n{t}") for h, t in rows))
    return p50(sizes) if sizes else None
