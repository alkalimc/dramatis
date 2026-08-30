"""Rebuild a current archive from a previous-generation one.

Only the **inputs** are carried across: fetched page bodies, the enumerated seed
sets, the structured tables, and the alias sources. Records are not migrated, and
that is the point — records are derived, so copying them would preserve whatever the
old rules got wrong while making it look like fresh output. Re-running normalisation
on carried-over inputs regenerates everything under the current rules and, in the
same pass, tests those rules against the full corpus.

The alternative to migrating is re-harvesting, which means several thousand requests
to a volunteer-run wiki to retrieve text already sitting on disk. That is not a cost
worth paying for tidiness.
"""

from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass, field
from pathlib import Path

from ..pack import Pack
from .archive import Archive

#: Old seed keys → current ones. The old names encoded their reader ("roster",
#: "disambig", "data"); the current ones are opaque so that renaming a reader does
#: not invalidate a stored scope.
SEED_MAP: dict[str, str] = {
    "S1": "S1", "S2": "S2", "S3": "S3", "S4": "S4", "S5": "S5", "S6": "S6",
    "S5_roster": "S5R",
    "S6_disambig": "S6D",
    "S1_data": "S1D",
}


@dataclass
class Migration:
    pages: int = 0
    seeds: dict[str, int] = field(default_factory=dict)
    tables: dict[str, int] = field(default_factory=dict)
    redirects: int = 0
    disambigs: int = 0
    carried_meta: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)


CARRY_META = ("fetched_at", "pages_fetched", "rc_watermark", "updated_at")
META_RENAME = {"rc_watermark": "watermark", "pages_fetched": "pages_held"}


def from_legacy(
    legacy_archive: Path,
    new_archive: Path,
    pack: Pack,
    *,
    legacy_cache: Path | None = None,
) -> Migration:
    """Carry inputs from `legacy_archive` (+ its cache) into a fresh archive.

    Does not normalise; the caller runs that stage so its report is the normal one
    rather than a special migration variant.
    """
    m = Migration()
    legacy_cache = legacy_cache or legacy_archive.with_suffix(".tcache")
    if not legacy_archive.exists():
        raise FileNotFoundError(legacy_archive)

    with Archive(new_archive) as archive:
        archive.db.execute("ATTACH DATABASE ? AS old", (str(legacy_archive),))
        if legacy_cache.exists():
            archive.db.execute("ATTACH DATABASE ? AS oldraw", (str(legacy_cache),))

        legacy_tables = _tables(archive.db, "old")
        legacy_raw_tables = _tables(archive.db, "oldraw") if legacy_cache.exists() else set()

        # ---- page bodies ----
        if "pages_raw" in legacy_raw_tables:
            archive.db.execute(
                "INSERT OR REPLACE INTO raw.pages(title,wikitext,revid,fetched) "
                "SELECT title,wikitext,revid,fetched FROM oldraw.pages_raw"
            )
            m.pages = archive.count("raw.pages", "wikitext IS NOT NULL")
        else:
            m.notes.append(
                f"no page cache at {legacy_cache} — a re-fetch is required before normalising"
            )

        # ---- seed sets ----
        if "seeds" in legacy_tables:
            unmapped: set[str] = set()
            rows: list[tuple[str, str]] = []
            for r in archive.db.execute("SELECT seed,title FROM old.seeds"):
                key = SEED_MAP.get(r["seed"])
                if key is None:
                    unmapped.add(r["seed"])
                    continue
                rows.append((key, r["title"]))
            archive.db.executemany(
                "INSERT OR IGNORE INTO seeds(seed,title) VALUES(?,?)", rows)
            for key, title in rows:
                m.seeds[key] = m.seeds.get(key, 0) + 1
            archive.set_meta("seed_counts", dict(m.seeds))
            if unmapped:
                m.notes.append(f"seed keys with no mapping, dropped: {sorted(unmapped)}")

        # ---- structured tables ----
        if "cargo_rows" in legacy_tables:
            archive.db.execute(
                "INSERT INTO source_rows(tbl,page,data) SELECT tbl,page,data FROM old.cargo_rows"
            )
            for r in archive.db.execute(
                "SELECT tbl, COUNT(*) AS n FROM source_rows GROUP BY tbl"
            ):
                m.tables[r["tbl"]] = r["n"]
            missing = set(pack.tables) - set(m.tables)
            if missing:
                m.notes.append(f"pack expects tables absent from the old archive: {sorted(missing)}")

        # ---- alias sources: out of the manifest blob, into their own tables ----
        redirects: dict[str, str] = {}
        disambigs: dict[str, list[str]] = {}
        if "manifest" in legacy_tables:
            legacy_meta = {
                r["key"]: json.loads(r["value"])
                for r in archive.db.execute("SELECT key,value FROM old.manifest")
            }
            redirects = legacy_meta.get("redirects") or {}
            disambigs = legacy_meta.get("disambigs") or {}
            for key in CARRY_META:
                if key in legacy_meta:
                    archive.set_meta(META_RENAME.get(key, key), legacy_meta[key])
                    m.carried_meta.append(META_RENAME.get(key, key))
        archive.write_aliases_source(redirects, disambigs)
        m.redirects = len(redirects)
        m.disambigs = sum(len(v) for v in disambigs.values())

        # ---- reconcile the carried scope with the pack, offline ----
        # A carried scope reflects the pack as it was. Two classes of difference can
        # be corrected from disk alone, and correcting them here is what makes the
        # migrated archive verifiable without several hundred requests to the site.
        for key, titles in pack.fixed_seeds.items():
            before = set(archive.seed(key))
            wanted = set(titles)
            if before == wanted:
                continue
            archive.db.execute("DELETE FROM seeds WHERE seed=?", (key,))
            archive.add_seeds(key, sorted(wanted))
            m.seeds[key] = len(wanted)
            added, removed = sorted(wanted - before), sorted(before - wanted)
            m.notes.append(
                f"{key} is a fixed list; re-applied it "
                f"(+{len(added)} {added} / -{len(removed)} {removed})"
            )

        # A redirect has no body to parse, so it belongs to the alias set rather than
        # to any corpus set. Membership is decidable from the cached text.
        for key in pack.corpus_seeds:
            stale = [
                row["title"]
                for row in archive.pages(archive.seed(key))
                if _is_redirect(row["wikitext"])
            ]
            if not stale:
                continue
            archive.db.executemany(
                "DELETE FROM seeds WHERE seed=? AND title=?", [(key, t) for t in stale])
            m.seeds[key] = archive.count("seeds", "seed=?", (key,))
            m.notes.append(
                f"{key}: dropped {len(stale)} redirect page(s) with no body to parse: "
                f"{', '.join(stale)}"
            )

        # ---- rebuild discovered seed sets from the pages we already hold ----
        # The old pipeline registered these during fetch and then lost them the next
        # time `scope` ran, because scope cleared the whole seed table. The page bodies
        # survived in the cache, so the membership can be recovered here without any
        # network access — and recovering it is what makes the migrated archive
        # complete rather than merely current.
        for hook in pack.followups:
            found: set[str] = set()
            for row in archive.pages(archive.seed(hook.of_seed)):
                found.update(hook.discover(row["title"], row["wikitext"]))
            present = sorted(t for t in found if archive.page(t) is not None)
            missing = sorted(found - set(present))
            if present:
                archive.add_seeds(hook.seed, present)
                m.seeds[hook.seed] = m.seeds.get(hook.seed, 0) + len(present)
                m.notes.append(
                    f"recovered {len(present)} {hook.label or hook.seed} title(s) from the cache: "
                    f"{', '.join(present)}"
                )
            if missing:
                m.notes.append(
                    f"{len(missing)} {hook.label or hook.seed} title(s) referenced but not cached; "
                    f"fetch before normalising: {', '.join(missing)}"
                )

        archive.set_meta("migrated_from", str(legacy_archive))
        archive.set_meta("seed_counts", dict(m.seeds))
        archive.commit()
        archive.db.execute("DETACH DATABASE old")
        if legacy_cache.exists():
            archive.db.execute("DETACH DATABASE oldraw")
    return m


#: Both spellings occur; MediaWiki accepts localised redirect syntax.
_REDIRECT_PREFIXES = ("#redirect", "#重定向")


def _is_redirect(wikitext: str | None) -> bool:
    return bool(wikitext) and wikitext.lstrip()[:9].lower().startswith(_REDIRECT_PREFIXES)


def _tables(db: sqlite3.Connection, schema: str) -> set[str]:
    return {
        r[0] for r in db.execute(f"SELECT name FROM {schema}.sqlite_master WHERE type='table'")
    }
