"""`*.archive` — the forge's own source of truth. One SQLite file.

It holds **normalised records**, not a copy of the wiki. Fetched wikitext lives in
a separate `*.rawcache` file, attached as `raw`. The split is not tidiness: it
makes "downstream never reads raw markup" a property of the file layout rather
than a rule people are asked to remember, and it means the archive can be handed
to someone without shipping the markup that only serves re-runs.

Three schema rules, each of which prevents a silent data-loss bug rather than an
inconvenience:

1. **Uniqueness includes the text.** Keying `lore` by `(page, path)` or `char_refs`
   by `(name, group)` alone lets distinct paragraphs at the same address overwrite
   each other, so both keys carry a content signature.
2. **Inserts never replace.** `INSERT OR REPLACE` destroys the loser of a key
   collision and returns success. Inserts are `OR IGNORE` and the number ignored is
   counted, so a discrepancy has to be explained (guard G1).
3. **Large relations get tables, not manifest blobs.** Redirects and disambiguations
   have their own tables and the manifest keeps counts.

Record tables are derived: `normalize` is always a full rebuild, so when their shape
changes they are dropped and recreated rather than migrated. Raw pages, seeds and source
rows are inputs and are never dropped.
"""

from __future__ import annotations

import datetime as dt
import json
import sqlite3
from collections.abc import Iterable, Iterator, Sequence
from pathlib import Path
from typing import Any

from ..normalize.guards import HIGH, LEGACY_SEVERITY
from ..normalize.records import KINDS, PARSER_VERSION, Record

#: Columns added after a schema shipped. `CREATE TABLE IF NOT EXISTS` is a no-op when the
#: table exists with a *different* shape, so a new column reaches new databases only and
#: every existing archive fails at write time — after the network work is already done.
#: Reconciled on open instead, which is idempotent and cheap. Only for tables holding
#: *inputs*; derived tables are rebuilt instead (see `_rebuild_stale_records`).
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

-- Structured side-channel captured during scope (structured tables and similar).
-- Cannot be keyed by (table, page): one page legitimately has several rows, and
-- keying by page drops the extras without a word.
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
    category    TEXT,
    grp         TEXT,
    source_ref  TEXT,
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
    condition TEXT,
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
    page         TEXT NOT NULL,
    term         TEXT NOT NULL,
    translations TEXT NOT NULL DEFAULT '{}',   -- JSON: label -> rendering
    category     TEXT,
    PRIMARY KEY (term, category)
);
CREATE TABLE IF NOT EXISTS char_refs (
    page        TEXT NOT NULL,
    name        TEXT NOT NULL,
    grp         TEXT,
    sig         TEXT NOT NULL,
    description TEXT NOT NULL,
    source      TEXT,
    PRIMARY KEY (name, grp, sig)
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
    kind      TEXT NOT NULL,          -- canonical, or a pack-defined form kind
    ordinal   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_forms_person ON forms(person_id);

-- ---- guards ----

-- `run_id` exists so a finding can be tied to the run that produced it. Without it,
-- findings accumulate across runs: a stale high-severity row sits beside a current tally
-- of zero, and one finding is recorded once per run. Severity only means something
-- relative to a run.
CREATE TABLE IF NOT EXISTS guard_findings (
    run_id   INTEGER NOT NULL DEFAULT 0,
    stage    TEXT NOT NULL DEFAULT '',
    guard    TEXT NOT NULL,
    severity TEXT NOT NULL,
    page     TEXT,
    detail   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_findings ON guard_findings(stage, guard, severity);

-- Last editor of a page at a given revision, for attribution. Cached so a rerun only
-- asks the site about pages whose revision changed.
CREATE TABLE IF NOT EXISTS editors (
    title TEXT PRIMARY KEY,
    revid INTEGER NOT NULL,
    user  TEXT NOT NULL
);

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
            self._rebuild_stale_records()
            self.db.executescript(SCHEMA)
            self._add_late_columns()
            self._migrate_severity()
            self._migrate_sync_time()
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

    def _rebuild_stale_records(self) -> None:
        """Drop derived tables whose columns differ from the declared record shape.

        Records are always regenerated in full by `normalize`, so a shape change is
        handled by dropping the table and letting the schema recreate it empty. Inputs
        (raw pages, seeds, source rows) are never touched here.
        """
        for cls in KINDS:
            present = [r[1] for r in self.db.execute(f"PRAGMA table_info({cls.TABLE})")]
            if present and not set(cls.COLUMNS) <= set(present):
                self.db.execute(f"DROP TABLE {cls.TABLE}")

    def _migrate_severity(self) -> None:
        """Rewrite severities stored by earlier versions to the current values. Idempotent."""
        for old, new in LEGACY_SEVERITY.items():
            self.db.execute("UPDATE guard_findings SET severity=? WHERE severity=?", (new, old))

    def _migrate_sync_time(self) -> None:
        """Fold the two per-path sync timestamps of earlier versions into `synced_at`."""
        legacy = [v for v in (self.get_meta("fetched_at"), self.get_meta("updated_at")) if v]
        if legacy and self.get_meta("synced_at") is None:
            self.set_meta("synced_at", max(legacy))
        self.db.execute("DELETE FROM manifest WHERE key IN ('fetched_at','updated_at')")

    def mark_synced(self) -> str:
        """Record that content was just brought level with the site. One key, every path."""
        now = dt.datetime.now(dt.UTC).isoformat(timespec="seconds")
        self.set_meta("synced_at", now)
        return now

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

        Counts **every** row, across all stages, and lists **every** guard, including
        those with nothing to report: a guard absent from the tally is otherwise
        indistinguishable from a guard that never ran. Two writers for one fact is how a
        manifest comes to publish a tally the table contradicts, so there is one writer,
        and it reads exactly what a person auditing the artifact would read.
        """
        from ..normalize.guards import GUARDS

        out: dict[str, tuple[int, int]] = {g: (0, 0) for g in GUARDS}
        for guard, severity, count in self.db.execute(
            "SELECT guard,severity,COUNT(*) FROM guard_findings GROUP BY guard,severity"
        ):
            high, low = out.get(guard, (0, 0))
            out[guard] = (high + count, low) if severity == HIGH else (high, low + count)
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

    # ---- attribution ----

    def editors(self) -> dict[str, tuple[int, str]]:
        """title -> (revid, last editor at that revid)."""
        return {r["title"]: (r["revid"], r["user"])
                for r in self.db.execute("SELECT title,revid,user FROM editors")}

    def put_editors(self, rows: Iterable[tuple[str, int, str]]) -> None:
        self.db.executemany(
            "INSERT INTO editors(title,revid,user) VALUES(?,?,?) ON CONFLICT(title) "
            "DO UPDATE SET revid=excluded.revid, user=excluded.user", list(rows))

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
