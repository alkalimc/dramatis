"""`*.archive` — the forge's own source of truth. One SQLite file.

It holds **normalised records**, not a copy of the wiki. Fetched wikitext lives in
a separate `*.rawcache` file, attached as `raw`. The split is not tidiness: it
makes "downstream never reads raw markup" a property of the file layout rather
than a rule people are asked to remember, and it means the archive can be handed
to someone without shipping 90 MB of markup that only serves re-runs.

Three defects of the previous schema are fixed here, and each was a silent
data-loss bug rather than an inconvenience:

1. **Uniqueness excluded the text.** `lore` keyed by `(page, section_path)` and
   `char_refs` by `(name, story_group)` meant distinct paragraphs at the same
   address overwrote each other. Both keys now include a content signature.
2. **Inserts replaced.** `INSERT OR REPLACE` destroyed the loser of a key
   collision and returned success. Inserts are now `OR IGNORE` and the number
   ignored is counted, so a discrepancy has to be explained (guard G1).
3. **The manifest carried a 282 KB blob.** The full redirect table was stuffed
   into a key-value row. Redirects and disambiguations now have their own tables
   and the manifest keeps counts.

Also gone: `media_refs`, a table with a schema, an index, and no writer. No media
is archived at all, so it described nothing.
"""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Iterable, Iterator, Sequence
from pathlib import Path
from typing import Any

from ..normalize.records import KINDS, PARSER_VERSION, Record

#: Columns added after a schema shipped. `CREATE TABLE IF NOT EXISTS` is a no-op when the
#: table exists with a *different* shape, so a new column reaches new databases only and
#: every existing archive fails at write time — after the network work is already done.
#: Reconciled on open instead, which is idempotent and cheap.
LATE_COLUMNS: dict[str, tuple[tuple[str, str], ...]] = {
    "guard_findings": (
        ("run_id", "INTEGER NOT NULL DEFAULT 0"),
        ("stage", "TEXT NOT NULL DEFAULT ''"),
    ),
}

SCHEMA = """
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS manifest (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL              -- JSON
);

-- Scope: the enumerated seed sets.
CREATE TABLE IF NOT EXISTS seeds (
    seed  TEXT NOT NULL,
    title TEXT NOT NULL,
    PRIMARY KEY (seed, title)
) WITHOUT ROWID;

-- Structured side-channel captured during scope (Cargo and similar).
-- Cannot be keyed by (table, page): one page legitimately has several rows, and
-- keying by page drops the extras without a word. Measured: 55 lost rows.
CREATE TABLE IF NOT EXISTS source_rows (
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    tbl   TEXT NOT NULL,
    page  TEXT NOT NULL,
    data  TEXT NOT NULL              -- JSON
);
CREATE INDEX IF NOT EXISTS idx_source_tbl_page ON source_rows(tbl, page);

-- Alias sources, each in its own table so the manifest stays small.
CREATE TABLE IF NOT EXISTS redirects (
    alias  TEXT PRIMARY KEY,
    target TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS disambigs (
    word      TEXT NOT NULL,
    candidate TEXT NOT NULL,
    PRIMARY KEY (word, candidate)
) WITHOUT ROWID;

-- ---- records ----

CREATE TABLE IF NOT EXISTS scenes (
    id          TEXT PRIMARY KEY,
    story_type  TEXT,
    story_group TEXT,
    text_path   TEXT,
    revid       INTEGER
);
CREATE TABLE IF NOT EXISTS lines (
    scene   TEXT NOT NULL,
    seq     INTEGER NOT NULL,
    speaker TEXT,
    text    TEXT NOT NULL,
    kind    TEXT NOT NULL,
    PRIMARY KEY (scene, seq)
);
CREATE TABLE IF NOT EXISTS choices (
    scene   TEXT NOT NULL,
    seq     INTEGER NOT NULL,
    options TEXT NOT NULL,
    PRIMARY KEY (scene, seq)
);
CREATE TABLE IF NOT EXISTS dossiers (
    page     TEXT PRIMARY KEY,
    fields   TEXT NOT NULL,
    sections TEXT NOT NULL,
    items    TEXT NOT NULL,
    revid    INTEGER
);
CREATE TABLE IF NOT EXISTS voices (
    page    TEXT NOT NULL,
    subject TEXT NOT NULL,
    idx     INTEGER NOT NULL,
    title   TEXT,
    trigger TEXT,
    text    TEXT NOT NULL,
    unlock  TEXT,
    PRIMARY KEY (page, idx)
);
CREATE TABLE IF NOT EXISTS lore (
    page  TEXT NOT NULL,
    path  TEXT NOT NULL,
    sig   TEXT NOT NULL,
    text  TEXT NOT NULL,
    revid INTEGER,
    PRIMARY KEY (page, path, sig)
);
CREATE TABLE IF NOT EXISTS letters (
    page   TEXT NOT NULL,
    sender TEXT,
    date   TEXT,
    title  TEXT,
    sig    TEXT NOT NULL,
    body   TEXT NOT NULL,
    PRIMARY KEY (page, sig)
);
CREATE TABLE IF NOT EXISTS terms (
    page     TEXT NOT NULL,
    zh       TEXT NOT NULL,
    en       TEXT,
    other    TEXT,
    category TEXT,
    PRIMARY KEY (zh, category)
);
CREATE TABLE IF NOT EXISTS char_refs (
    page        TEXT NOT NULL,
    name        TEXT NOT NULL,
    story_group TEXT,
    sig         TEXT NOT NULL,
    description TEXT NOT NULL,
    source      TEXT,
    PRIMARY KEY (name, story_group, sig)
);
CREATE TABLE IF NOT EXISTS aliases (
    alias  TEXT NOT NULL,
    target TEXT NOT NULL,
    kind   TEXT NOT NULL,
    PRIMARY KEY (alias, target, kind)
);

-- ---- identity: a page is not a person ----

CREATE TABLE IF NOT EXISTS persons (
    person_id    TEXT PRIMARY KEY,   -- canonical page title
    primary_page TEXT NOT NULL,
    form_count   INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS forms (
    page      TEXT PRIMARY KEY,
    person_id TEXT NOT NULL,
    kind      TEXT NOT NULL,          -- canonical | alter | variant
    ordinal   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_forms_person ON forms(person_id);

-- ---- guards ----

-- `run_id` exists because findings used to accumulate across runs with no way to tell
-- which run produced which row. A probe found one high-severity row from an earlier run
-- sitting beside a current-run tally of zero, and the same duplicate finding recorded six
-- times. Severity is only meaningful relative to a run.
CREATE TABLE IF NOT EXISTS guard_findings (
    run_id   INTEGER NOT NULL DEFAULT 0,
    stage    TEXT NOT NULL DEFAULT '',
    guard    TEXT NOT NULL,
    severity TEXT NOT NULL,
    page     TEXT,
    detail   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_findings ON guard_findings(stage, guard, severity);

CREATE INDEX IF NOT EXISTS idx_lines_speaker ON lines(speaker);
CREATE INDEX IF NOT EXISTS idx_lore_page     ON lore(page);
CREATE INDEX IF NOT EXISTS idx_voices_subj   ON voices(subject);
"""

RAW_SCHEMA = """
CREATE TABLE IF NOT EXISTS pages (
    title    TEXT PRIMARY KEY,
    wikitext TEXT,
    revid    INTEGER,
    fetched  TEXT
);
CREATE TABLE IF NOT EXISTS raw_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"""

RECORD_TABLES: tuple[str, ...] = tuple(dict.fromkeys(k.TABLE for k in KINDS))


class Archive:
    """The archive plus its attached raw cache.

    `INSERT OR IGNORE` everywhere, with `total_changes` sampled around each batch
    so the caller can reconcile produced against stored. That reconciliation is
    the whole reason the write path is not a one-liner.
    """

    def __init__(self, path: Path, rawcache: Path | None = None, *, readonly: bool = False) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        self.path = path
        self.rawcache = rawcache if rawcache is not None else default_rawcache(path)
        self.readonly = readonly
        if readonly:
            self.db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        else:
            self.db = sqlite3.connect(path)
        self.db.row_factory = sqlite3.Row
        if not readonly:
            self.db.executescript(SCHEMA)
            self._add_late_columns()
        # The raw cache is attached, not embedded. Pass it as a bound parameter: a
        # URI filename is only honoured when the connection enabled URI handling, so
        # interpolating `file:…` into the statement of a non-URI connection attaches a
        # file literally named `file:…`.
        if readonly:
            self.has_raw = self.rawcache.exists()
            if self.has_raw:
                self.db.execute(
                    "ATTACH DATABASE ? AS raw", (f"file:{self.rawcache}?mode=ro",))
        else:
            self.rawcache.parent.mkdir(parents=True, exist_ok=True)
            self.db.execute("ATTACH DATABASE ? AS raw", (str(self.rawcache),))
            self.has_raw = True
            self.db.executescript(
                RAW_SCHEMA.replace("CREATE TABLE IF NOT EXISTS ", "CREATE TABLE IF NOT EXISTS raw.")
            )
            self.set_meta("parser_version", PARSER_VERSION)

    def _add_late_columns(self) -> None:
        """Bring an older database up to the declared shape, one column at a time."""
        for table, columns in LATE_COLUMNS.items():
            present = {r[1] for r in self.db.execute(f"PRAGMA table_info({table})")}
            if not present:
                continue  # table not created yet; the schema script owns it
            for name, decl in columns:
                if name not in present:
                    self.db.execute(f"ALTER TABLE {table} ADD COLUMN {name} {decl}")

    # ---- lifecycle ----

    def close(self) -> None:
        if not self.readonly:
            self.db.commit()
        self.db.close()

    def commit(self) -> None:
        if not self.readonly:
            self.db.commit()

    def __enter__(self) -> Archive:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # ---- manifest ----

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

    # ---- scope ----

    def write_seeds(self, seeds: dict[str, Sequence[str]], *, preserve: Iterable[str] = ()) -> None:
        """Replace the enumerated seed sets, leaving discovered ones alone.

        `preserve` exists because some membership is only learnable by fetching — a
        page whose body is transcluded from a subpage the site registers nowhere.
        Clearing the whole table on every re-scope would forget those, and they would
        stay forgotten until someone noticed two stories had gone missing.
        """
        kept = set(preserve)
        for seed in [r["seed"] for r in self.db.execute("SELECT DISTINCT seed FROM seeds")]:
            if seed not in kept:
                self.db.execute("DELETE FROM seeds WHERE seed=?", (seed,))
        self.db.executemany(
            "INSERT OR IGNORE INTO seeds(seed,title) VALUES(?,?)",
            [(k, t) for k, titles in seeds.items() for t in titles if k not in kept],
        )
        counts = {k: len(v) for k, v in seeds.items() if k not in kept}
        for seed in kept:
            counts[seed] = self.count("seeds", "seed=?", (seed,))
        self.set_meta("seed_counts", counts)

    def add_seeds(self, seed: str, titles: Iterable[str]) -> None:
        self.db.executemany(
            "INSERT OR IGNORE INTO seeds(seed,title) VALUES(?,?)", [(seed, t) for t in titles]
        )
        # Republish immediately: this is the fetch-stage path, and leaving the scope-stage
        # count in place is what made the manifest disagree with the table.
        self.refresh_seed_counts()

    def write_source_rows(self, tables: dict[str, list[dict]]) -> None:
        self.db.execute("DELETE FROM source_rows")
        self.db.executemany(
            "INSERT INTO source_rows(tbl,page,data) VALUES(?,?,?)",
            [
                (tbl, r.get("page", ""), json.dumps(r, ensure_ascii=False))
                for tbl, rows in tables.items()
                for r in rows
            ],
        )

    def write_aliases_source(self, redirects: dict[str, str], disambigs: dict[str, list[str]]) -> None:
        self.db.execute("DELETE FROM redirects")
        self.db.executemany(
            "INSERT OR IGNORE INTO redirects(alias,target) VALUES(?,?)",
            [(a, t) for a, t in redirects.items() if a and t],
        )
        self.db.execute("DELETE FROM disambigs")
        self.db.executemany(
            "INSERT OR IGNORE INTO disambigs(word,candidate) VALUES(?,?)",
            [(w, c) for w, cands in disambigs.items() for c in cands if w and c],
        )
        self.set_meta("alias_source_counts",
                      {"redirects": len(redirects),
                       "disambigs": sum(len(v) for v in disambigs.values())})

    def redirects(self) -> dict[str, str]:
        return {r["alias"]: r["target"] for r in self.db.execute("SELECT * FROM redirects")}

    def disambigs(self) -> dict[str, list[str]]:
        out: dict[str, list[str]] = {}
        for r in self.db.execute("SELECT word,candidate FROM disambigs ORDER BY word,candidate"):
            out.setdefault(r["word"], []).append(r["candidate"])
        return out

    def seed(self, name: str) -> list[str]:
        return [r["title"] for r in self.db.execute(
            "SELECT title FROM seeds WHERE seed=? ORDER BY title", (name,))]

    def seed_map(self) -> dict[str, str]:
        """title -> seed. Ambiguity is resolved by lexical seed order for
        determinism; a title in two seed sets is itself worth knowing about."""
        out: dict[str, str] = {}
        for r in self.db.execute("SELECT seed,title FROM seeds ORDER BY seed"):
            out.setdefault(r["title"], r["seed"])
        return out

    def titles_in(self, seeds: Iterable[str]) -> list[str]:
        keys = list(seeds)
        if not keys:
            return []
        q = ",".join("?" * len(keys))
        return [r["title"] for r in self.db.execute(
            f"SELECT DISTINCT title FROM seeds WHERE seed IN ({q}) ORDER BY title", keys)]

    def source_rows(self, tbl: str) -> dict[str, list[dict]]:
        out: dict[str, list[dict]] = {}
        for r in self.db.execute(
            "SELECT page,data FROM source_rows WHERE tbl=? ORDER BY id", (tbl,)
        ):
            out.setdefault(r["page"], []).append(json.loads(r["data"]))
        return out

    def all_source_rows(self) -> dict[str, dict[str, list[dict]]]:
        out: dict[str, dict[str, list[dict]]] = {}
        for r in self.db.execute("SELECT tbl,page,data FROM source_rows ORDER BY id"):
            out.setdefault(r["tbl"], {}).setdefault(r["page"], []).append(json.loads(r["data"]))
        return out

    # ---- raw pages ----

    def put_page(self, title: str, wikitext: str | None, revid: int | None, when: str) -> None:
        self.db.execute(
            "INSERT INTO raw.pages(title,wikitext,revid,fetched) VALUES(?,?,?,?) "
            "ON CONFLICT(title) DO UPDATE SET "
            "wikitext=excluded.wikitext, revid=excluded.revid, fetched=excluded.fetched",
            (title, wikitext, revid, when),
        )

    def drop_page(self, title: str) -> None:
        self.db.execute("DELETE FROM raw.pages WHERE title=?", (title,))

    def have_pages(self) -> set[str]:
        if not self.has_raw:
            return set()
        return {r["title"] for r in self.db.execute(
            "SELECT title FROM raw.pages WHERE wikitext IS NOT NULL")}

    def page(self, title: str) -> sqlite3.Row | None:
        if not self.has_raw:
            return None
        return self.db.execute(
            "SELECT title,wikitext,revid FROM raw.pages WHERE title=?", (title,)).fetchone()

    def pages(self, titles: Iterable[str]) -> Iterator[sqlite3.Row]:
        for t in titles:
            row = self.page(t)
            if row is not None and row["wikitext"]:
                yield row

    # ---- records ----

    def reset_records(self) -> None:
        for tbl in RECORD_TABLES:
            self.db.execute(f"DELETE FROM {tbl}")
        self.db.execute("DELETE FROM persons")
        self.db.execute("DELETE FROM forms")
        # G1 was excluded here, so seed-drift findings accumulated run over run forever.
        # Runs are now identified, so clearing by guard is unnecessary — a reader asks for
        # the latest run and gets exactly that run's findings.
        self.db.execute("DELETE FROM guard_findings WHERE guard IN ('G2','G3','G4','G5')")

    def insert_records(self, records: Sequence[Record]) -> tuple[int, int]:
        """Insert one kind's worth of records. Returns (stored, ignored).

        `total_changes` around the batch is the reliable way to learn how many
        rows an `OR IGNORE` batch actually wrote; `cursor.rowcount` after
        `executemany` is documented as implementation-defined for this case.
        """
        if not records:
            return 0, 0
        cls = type(records[0])
        cols = ",".join(cls.COLUMNS)
        marks = ",".join("?" * len(cls.COLUMNS))
        before = self.db.total_changes
        self.db.executemany(
            f"INSERT OR IGNORE INTO {cls.TABLE}({cols}) VALUES({marks})",
            [r.row() for r in records],
        )
        stored = self.db.total_changes - before
        return stored, len(records) - stored

    def write_identity(self, persons: dict[str, list[tuple[str, str, int]]]) -> None:
        """persons: person_id -> [(page, kind, ordinal)]. Canonical page first."""
        self.db.execute("DELETE FROM persons")
        self.db.execute("DELETE FROM forms")
        self.db.executemany(
            "INSERT INTO persons(person_id,primary_page,form_count) VALUES(?,?,?)",
            [(pid, pid, len(forms)) for pid, forms in persons.items()],
        )
        self.db.executemany(
            "INSERT OR IGNORE INTO forms(page,person_id,kind,ordinal) VALUES(?,?,?,?)",
            [(page, pid, kind, ordinal)
             for pid, forms in persons.items()
             for page, kind, ordinal in forms],
        )
        self.set_meta("person_count", len(persons))

    def persons(self) -> dict[str, list[sqlite3.Row]]:
        out: dict[str, list[sqlite3.Row]] = {}
        for r in self.db.execute(
            "SELECT person_id,page,kind,ordinal FROM forms ORDER BY person_id,ordinal,page"
        ):
            out.setdefault(r["person_id"], []).append(r)
        return out

    def person_of(self) -> dict[str, str]:
        return {r["page"]: r["person_id"] for r in self.db.execute("SELECT page,person_id FROM forms")}

    # ---- guards ----

    def next_run_id(self) -> int:
        """One more than the highest run recorded. Runs are numbered, not timestamped:
        the question asked of them is always "is this the latest", never "when"."""
        return int(self.scalar("SELECT COALESCE(MAX(run_id),0)+1 FROM guard_findings") or 1)

    def write_findings(
        self,
        rows: Sequence[tuple[str, str, str | None, str]],
        *,
        stage: str,
        run_id: int | None = None,
    ) -> None:
        """Replace this stage's findings. Other stages' rows are left alone.

        Clearing is **per stage**, not per run. Findings legitimately arrive from more
        than one stage — `scope` reports seed drift, `normalize` reports produced-vs-stored
        — so a tally scoped to "the latest run" would silently omit whichever stage ran
        first. That is the same shape as the defect this bookkeeping exists to fix, so the
        unit of replacement is the stage that owns the finding.
        """
        run = self.next_run_id() if run_id is None else run_id
        self.db.execute("DELETE FROM guard_findings WHERE stage=?", (stage,))
        if rows:
            self.db.executemany(
                "INSERT INTO guard_findings(run_id,stage,guard,severity,page,detail) "
                "VALUES(?,?,?,?,?,?)",
                [(run, stage, *r) for r in rows])

    def latest_run(self) -> int:
        return int(self.scalar("SELECT COALESCE(MAX(run_id),0) FROM guard_findings") or 0)

    def tally_from_table(self) -> dict[str, tuple[int, int]]:
        """The guard tally, derived from the rows rather than written beside them.

        Counts **every** row, across all stages. Two writers for one fact is how the
        manifest came to publish a tally the table contradicted; there is now one writer,
        and it reads exactly what a person auditing the artifact would read.
        """
        out: dict[str, tuple[int, int]] = {}
        for guard, severity, count in self.db.execute(
            "SELECT guard,severity,COUNT(*) FROM guard_findings GROUP BY guard,severity"
        ):
            high, low = out.get(guard, (0, 0))
            out[guard] = (high + count, low) if severity == "高" else (high, low + count)
        return out

    def seed_counts_from_table(self) -> dict[str, int]:
        """Seed sizes read from the seeds table.

        The manifest used to publish a count written during `scope`, before `fetch`
        discovered the sets whose membership only fetching can learn — so it published a
        pre-discovery snapshot that looked like a final count.
        """
        return {r[0]: r[1] for r in self.db.execute(
            "SELECT seed,COUNT(*) FROM seeds GROUP BY seed")}

    def refresh_seed_counts(self) -> dict[str, int]:
        counts = self.seed_counts_from_table()
        self.set_meta("seed_counts", counts)
        return counts

    def clear_findings(self, *guards: str) -> None:
        for g in guards:
            self.db.execute("DELETE FROM guard_findings WHERE guard=?", (g,))

    def findings(self, guard: str, severity: str | None = None) -> list[sqlite3.Row]:
        sql = "SELECT * FROM guard_findings WHERE guard=?"
        args: list[Any] = [guard]
        if severity:
            sql += " AND severity=?"
            args.append(severity)
        return list(self.db.execute(sql, args))

    # ---- misc ----

    def count(self, table: str, where: str = "", args: Sequence[Any] = ()) -> int:
        sql = f"SELECT COUNT(*) AS n FROM {table}"
        if where:
            sql += f" WHERE {where}"
        return int(self.db.execute(sql, tuple(args)).fetchone()["n"])

    def scalar(self, sql: str, args: Sequence[Any] = ()) -> Any:
        row = self.db.execute(sql, tuple(args)).fetchone()
        return row[0] if row else None


def default_rawcache(archive: Path) -> Path:
    """`x.archive` → `x.rawcache`."""
    return archive.with_suffix(".rawcache")
