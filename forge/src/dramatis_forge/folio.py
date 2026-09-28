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

FOLIO_FORMAT_VERSION = 2

SCHEMA = """
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS chunks (
    id         TEXT PRIMARY KEY,      -- stable, content-derived
    ord        INTEGER NOT NULL,      -- dense 0..N-1; indexes the vector blob
    template   TEXT NOT NULL,         -- retrieval-unit type
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
    material     INTEGER NOT NULL,     -- chars of the person's own material, all templates
    -- Material thickness in [0, 1): m / (m + median), m = `material`, median over the
    -- roster persons with m > 0. `material` counts the person's own text only: whole
    -- non-dialogue units attributed to them (voice, dossier, letters) plus their own
    -- spoken lines, not the other speakers of a shared dialogue unit. Saturating, 0.5 at
    -- the median person, 0 with no material; the corpus's own median sets the scale, so
    -- there is no tuned constant. A volume, not a quality verdict; readers only compare.
    persona_confidence REAL NOT NULL DEFAULT 0.0,
    birthday     TEXT                  -- MM-DD, NULL when the source gives none
        CHECK (birthday IS NULL OR birthday GLOB '[0-1][0-9]-[0-3][0-9]')
);

-- The tables below join on `chunks.id` and `persons.person_id` without declaring
-- foreign keys, like the rest of this schema: the file is written once by one builder
-- and opened read-only, so there is no writer for a constraint to protect against.

-- Speaker attribution, many-valued: every person a unit belongs to (all speakers of a
-- dialogue unit, forms expanded to their person). A unit that belongs to nobody has
-- no rows.
CREATE TABLE IF NOT EXISTS unit_persons (
    chunk_id  TEXT NOT NULL,
    person_id TEXT NOT NULL,
    PRIMARY KEY (chunk_id, person_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_unit_persons_person ON unit_persons(person_id, chunk_id);

-- Co-appearance history from the corpus: how many scenes two persons share. One row
-- per unordered pair, stored with a < b so a pair has exactly one address.
CREATE TABLE IF NOT EXISTS cooccur (
    a      TEXT NOT NULL,
    b      TEXT NOT NULL,
    scenes INTEGER NOT NULL CHECK (scenes > 0),
    PRIMARY KEY (a, b),
    CHECK (a < b)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_cooccur_b ON cooccur(b, a);

-- What a person knows first-hand, materialised at build time: `self` = units they
-- are attributed to, `lived` = the other dialogue units of scenes they speak in.
-- Retrieval weights by it; it never filters.
CREATE TABLE IF NOT EXISTS knowledge_scope (
    person_id TEXT NOT NULL,
    chunk_id  TEXT NOT NULL,
    kind      TEXT NOT NULL CHECK (kind IN ('self', 'lived')),
    PRIMARY KEY (person_id, chunk_id)
) WITHOUT ROWID;

-- The topical part of knowledge scope: terms that put a lore hit inside a person's
-- domain. One row per term.
CREATE TABLE IF NOT EXISTS topic_terms (
    person_id TEXT NOT NULL,
    term      TEXT NOT NULL,
    PRIMARY KEY (person_id, term)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS aliases (
    alias  TEXT NOT NULL,
    target TEXT NOT NULL,
    kind   TEXT NOT NULL,
    PRIMARY KEY (alias, target, kind)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_aliases_alias ON aliases(alias);

-- Assembled, never generated, at runtime. Slots are a closed set:
--   subject = person_id      system | tone | fallback | capability
--   subject = 'host'         in_world | meta (the host's two layers; meta never enters
--                            another person's session), coldstart (the builder's
--                            first-run recommendation order), and any person slot
-- `generator_version` names the generator that wrote the row, so a gated persona run
-- can resume by skipping rows the current version already produced. Builder-computed
-- rows (coldstart) carry the builder's own version string.
CREATE TABLE IF NOT EXISTS prompts (
    subject           TEXT NOT NULL,
    slot              TEXT NOT NULL CHECK (slot IN (
                          'system', 'tone', 'fallback', 'capability',
                          'in_world', 'meta', 'coldstart')),
    body              TEXT NOT NULL,
    generator_version TEXT NOT NULL DEFAULT '',
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
    stats    TEXT NOT NULL DEFAULT '{}'
);

-- key -> JSON value. Keys readers act on: format_version, requires (JSON list of
-- capabilities the reader must implement, or refuse to load), segmenter, stopwords,
-- wording (JSON object: every user-facing string and in-world name, the client's only
-- source of UI text), scene_marker (JSON [open, close] wrapping a scene line),
-- clock.year_offset (integer: in-world year = local year minus this).
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
            "(id,ord,template,page,revid,title,header,text,chars,"
            " span_of,span_from,span_to) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            rows,
        )
        return len(rows)

    def add_unit_persons(self, rows: Iterable[tuple[str, str]]) -> None:
        """(chunk_id, person_id) pairs."""
        self.db.executemany(
            "INSERT OR IGNORE INTO unit_persons(chunk_id,person_id) VALUES(?,?)", rows)

    def add_fts(self, rows: Iterable[tuple[int, str]]) -> None:
        """`content=''` makes the FTS table contentless: it stores only the index,
        not a second copy of the text. Halves the lexical path's disk cost, and the
        row id ties back to `chunks.ord`."""
        self.db.executemany(
            "INSERT INTO chunks_fts(rowid, tokens) VALUES(?,?)", rows)

    def write_persons(self, rows: Sequence[tuple]) -> None:
        self.db.executemany(
            "INSERT OR REPLACE INTO persons"
            "(person_id,primary_page,display,forms,facets,material,persona_confidence,"
            " birthday) VALUES(?,?,?,?,?,?,?,?)",
            rows,
        )

    def write_topic_terms(self, rows: Iterable[tuple[str, str]]) -> None:
        """(person_id, term) pairs."""
        self.db.executemany(
            "INSERT OR IGNORE INTO topic_terms(person_id,term) VALUES(?,?)", rows)

    def write_cooccur(self, rows: Iterable[tuple[str, str, int]]) -> None:
        """(a, b, scenes) with a < b."""
        self.db.executemany("INSERT INTO cooccur(a,b,scenes) VALUES(?,?,?)", rows)

    def write_prompt(self, subject: str, slot: str, body: str, generator_version: str) -> None:
        self.db.execute(
            "INSERT OR REPLACE INTO prompts(subject,slot,body,generator_version) VALUES(?,?,?,?)",
            (subject, slot, body, generator_version))

    def write_aliases(self, rows: Sequence[tuple[str, str, str]]) -> None:
        self.db.executemany(
            "INSERT OR IGNORE INTO aliases(alias,target,kind) VALUES(?,?,?)", rows)

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
