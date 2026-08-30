"""`*.folio` — the knowledge base is one file.

A single SQLite database holding everything the runtime needs and nothing it does
not: retrieval units, their lexical index, their vectors, the person roster, the
alias dictionary, the synthesised prompts, and a manifest that says exactly which
build produced all of it.

Design notes worth the space:

**Vectors live here too, in one contiguous blob ordered by `chunks.ord`.** An
earlier design shipped them as a separate file to support "one corpus, many
encoders". That flexibility had no user, and it cost an extra distributable plus a
version-compatibility rule between two files. Maintainers who want to compare two
encoders build both locally; the distribution does not have to model it.

**Prompts live here too.** They are derived from this corpus by a generator whose
output is only valid for it, so co-locating them makes lineage automatic instead of
making it a constraint someone has to enforce.

**Chunks carry their span.** `span_of` / `span_from` / `span_to` identify the source
range a unit covers. Dialogue windows overlap by construction — that is good for
recall and bad for result diversity, since a top-6 could otherwise be six
half-identical windows of one conversation. Recording the span lets the engine merge
or cap overlapping hits at query time, which is where the decision belongs: the
corpus should keep the recall, the ranker should spend it.

**One integer format version plus a build fingerprint**, not a five-component
semver range. This is a single-user local application; a dependency solver would be
modelling a distribution problem that does not exist. The client's rules are: refuse
to load an unknown `format_version`; warn but continue when the encoder fingerprint
does not match the local weights, because mismatched lineage degrades retrieval
without breaking it, and refusing to start is the worse failure.
"""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Iterable, Sequence
from pathlib import Path
from typing import Any

FOLIO_FORMAT_VERSION = 1

SCHEMA = """
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS chunks (
    id         TEXT PRIMARY KEY,      -- stable, content-derived
    ord        INTEGER NOT NULL,      -- dense 0..N-1; indexes the vector blob
    template   TEXT NOT NULL,         -- retrieval-unit type
    person     TEXT,                  -- person_id when the unit belongs to someone
    page       TEXT NOT NULL,         -- source page title
    revid      INTEGER,               -- source revision: attribution and staleness
    title      TEXT,                  -- human label for citation cards
    header     TEXT,                  -- context line prepended when embedding
    text       TEXT NOT NULL,         -- body as retrieved and shown
    chars      INTEGER NOT NULL,
    span_of    TEXT,                  -- container id for overlapping units
    span_from  INTEGER,
    span_to    INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_ord     ON chunks(ord);
CREATE INDEX IF NOT EXISTS        idx_chunks_tmpl    ON chunks(template);
CREATE INDEX IF NOT EXISTS        idx_chunks_person  ON chunks(person);
CREATE INDEX IF NOT EXISTS        idx_chunks_span    ON chunks(span_of, span_from);

-- Lexical path. `tokens` is pre-segmented text written into a plain unicode61
-- column: BM25 comes out identical to a custom tokeniser, with no native
-- dependency and no per-platform shared library, and without requiring a SQLite
-- build that permits extension loading (some platform Python builds do not).
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    tokens,
    content=''
);

CREATE TABLE IF NOT EXISTS vectors (
    id    INTEGER PRIMARY KEY CHECK (id = 0),
    dim   INTEGER NOT NULL,
    count INTEGER NOT NULL,
    dtype TEXT NOT NULL,              -- f16 | f32
    data  BLOB NOT NULL               -- count * dim, row-major, ordered by chunks.ord
);

CREATE TABLE IF NOT EXISTS persons (
    person_id    TEXT PRIMARY KEY,
    primary_page TEXT NOT NULL,
    display      TEXT NOT NULL,
    forms        TEXT NOT NULL,        -- JSON [{page, kind}]
    facets       TEXT NOT NULL,        -- JSON: roster attributes, site wording
    material     INTEGER NOT NULL,     -- chars of source material, for confidence
    confidence   REAL NOT NULL DEFAULT 0.0
);

CREATE TABLE IF NOT EXISTS aliases (
    alias  TEXT NOT NULL,
    target TEXT NOT NULL,
    kind   TEXT NOT NULL,
    PRIMARY KEY (alias, target, kind)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_aliases_alias ON aliases(alias);

CREATE TABLE IF NOT EXISTS prompts (
    subject   TEXT NOT NULL,           -- person_id, or a system role
    slot      TEXT NOT NULL,           -- system | tone | capability | fallback | schedule | …
    body      TEXT NOT NULL,
    generator TEXT NOT NULL DEFAULT "",
    PRIMARY KEY (subject, slot)
) WITHOUT ROWID;

-- Per-template score statistics. Lexical and dense scores are not comparable
-- across unit types whose lengths differ by an order of magnitude; the ranker
-- needs these to calibrate rather than to guess.
CREATE TABLE IF NOT EXISTS template_stats (
    template TEXT PRIMARY KEY,
    count    INTEGER NOT NULL,
    chars_p50 INTEGER NOT NULL,
    chars_p95 INTEGER NOT NULL,
    chars_max INTEGER NOT NULL,
    stats    TEXT NOT NULL DEFAULT "{}"
);

CREATE TABLE IF NOT EXISTS manifest (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"""


class Folio:
    def __init__(self, path: Path, *, readonly: bool = False) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        self.path = path
        self.readonly = readonly
        if readonly:
            self.db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        else:
            self.db = sqlite3.connect(path)
        self.db.row_factory = sqlite3.Row
        if not readonly:
            self.db.executescript(SCHEMA)

    @classmethod
    def create(cls, path: Path) -> Folio:
        """Fresh build. A folio is never patched in place: it is a distributable
        whose fingerprint must describe its whole contents."""
        if path.exists():
            path.unlink()
        for suffix in ("-wal", "-shm"):
            side = path.with_name(path.name + suffix)
            if side.exists():
                side.unlink()
        return cls(path)

    def close(self) -> None:
        if not self.readonly:
            self.db.commit()
        self.db.close()

    def __enter__(self) -> Folio:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def commit(self) -> None:
        if not self.readonly:
            self.db.commit()

    # ---- writing ----

    def add_chunks(self, rows: Sequence[tuple]) -> int:
        self.db.executemany(
            "INSERT OR IGNORE INTO chunks"
            "(id,ord,template,person,page,revid,title,header,text,chars,"
            " span_of,span_from,span_to) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
            rows,
        )
        return len(rows)

    def add_fts(self, rows: Iterable[tuple[int, str]]) -> None:
        """`content=''` makes the FTS table contentless: it stores only the index,
        not a second copy of the text. Halves the lexical path's disk cost, and the
        row id ties back to `chunks.ord`."""
        self.db.executemany(
            "INSERT INTO chunks_fts(rowid, tokens) VALUES(?,?)", rows)

    def write_vectors(self, dim: int, count: int, dtype: str, data: bytes) -> None:
        self.db.execute("DELETE FROM vectors")
        self.db.execute(
            "INSERT INTO vectors(id,dim,count,dtype,data) VALUES(0,?,?,?,?)",
            (dim, count, dtype, data),
        )

    def write_persons(self, rows: Sequence[tuple]) -> None:
        self.db.executemany(
            "INSERT OR REPLACE INTO persons"
            "(person_id,primary_page,display,forms,facets,material,confidence) "
            "VALUES(?,?,?,?,?,?,?)",
            rows,
        )

    def write_aliases(self, rows: Sequence[tuple[str, str, str]]) -> None:
        self.db.executemany(
            "INSERT OR IGNORE INTO aliases(alias,target,kind) VALUES(?,?,?)", rows)

    def write_prompts(self, rows: Sequence[tuple[str, str, str, str]]) -> None:
        self.db.executemany(
            "INSERT OR REPLACE INTO prompts(subject,slot,body,generator) VALUES(?,?,?,?)", rows)

    def write_template_stats(self, rows: Sequence[tuple]) -> None:
        self.db.executemany(
            "INSERT OR REPLACE INTO template_stats"
            "(template,count,chars_p50,chars_p95,chars_max,stats) VALUES(?,?,?,?,?,?)",
            rows,
        )

    def set_meta(self, key: str, value: Any) -> None:
        self.db.execute(
            "INSERT INTO manifest(key,value) VALUES(?,?) "
            "ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (key, json.dumps(value, ensure_ascii=False)),
        )

    def get_meta(self, key: str, default: Any = None) -> Any:
        row = self.db.execute("SELECT value FROM manifest WHERE key=?", (key,)).fetchone()
        return json.loads(row["value"]) if row else default

    def manifest(self) -> dict[str, Any]:
        return {r["key"]: json.loads(r["value"]) for r in self.db.execute("SELECT * FROM manifest")}

    def optimize(self) -> None:
        """Called once at the end of a build. `optimize` on the FTS index is not
        cosmetic — an unoptimised contentless index can be several times larger."""
        self.db.execute("INSERT INTO chunks_fts(chunks_fts) VALUES('optimize')")
        self.db.commit()
        self.db.execute("VACUUM")
        self.db.execute("ANALYZE")
        self.db.commit()

    # ---- reading ----

    def count(self, table: str = "chunks") -> int:
        return int(self.db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])

    def templates(self) -> dict[str, int]:
        return {r["template"]: r["n"] for r in self.db.execute(
            "SELECT template, COUNT(*) AS n FROM chunks GROUP BY template ORDER BY n DESC")}
