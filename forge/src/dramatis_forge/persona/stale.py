"""Which personas were written from material that has since changed.

A reply's raw cache entry records its `source`: the ids of the units whose text was sent
and the ids of every unit that could have been (before the budget). Unit ids are content
hashes, so after a sync and rebuild the two sets can be compared with the new folio
without a network call:

* stale: a unit whose text was sent is gone from the folio (its text changed, or the
  unit was removed or re-cut). The persona was written from words that no longer exist.
* new material only: every sent unit is still there, but units now attributed to the
  person were not before. Weaker, since the budget may never have picked them up, so it
  is listed apart.
* never generated: no cache entry and the person has material.
* no record: an entry written before sources were recorded; nothing to compare.

Entries of an older generator version are only counted: a normal run regenerates them.
Regeneration stays a choice: `forge persona --stale` lists, `--regenerate` calls again
for the stale ones only.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path

from .generator import Generator
from .material import HOST, Material, for_host, for_person
from .params import Persona
from .store import RawCache

#: Budgets large enough to keep everything, so a material pass reports every unit.
_ALL = Persona(material_tokens=10**9, story_tokens=10**9)


def source_of(mat: Material) -> dict[str, list[str]]:
    return {"sent": sorted(mat.sent), "attributed": sorted(mat.attributed)}


def current_material(db: sqlite3.Connection, gen: Generator, sep: str
                     ) -> Callable[[str], Material]:
    """Material of a subject from the folio as it is now, unbudgeted. Token counts are
    not needed here, so none are taken."""
    def of(subject: str) -> Material:
        if subject == HOST:
            assert gen.host is not None
            return for_host(db, gen.host, gen, _ALL, lambda _t: 0, sep)
        return for_person(db, subject, gen, _ALL, lambda _t: 0, sep)
    return of


@dataclass(frozen=True)
class Row:
    subject: str
    sent: int
    #: Sent units no longer in the folio.
    changed: int
    #: Units attributed now that were not when the persona was written.
    new: int


@dataclass
class Result:
    version: str
    stale: list[Row] = field(default_factory=list)
    new_only: list[Row] = field(default_factory=list)
    never: list[str] = field(default_factory=list)
    no_record: list[str] = field(default_factory=list)
    older_version: int = 0
    current: int = 0

    @property
    def to_regenerate(self) -> list[str]:
        return [r.subject for r in self.stale]


def check(db: sqlite3.Connection, cache: RawCache, version: str, subjects: Iterable[str],
          material_of: Callable[[str], Material]) -> Result:
    ids = {r[0] for r in db.execute("SELECT id FROM chunks")}
    res = Result(version=version)
    for subject in subjects:
        entry = cache.entry(subject)
        if entry is None:
            if not material_of(subject).empty:
                res.never.append(subject)
            continue
        if entry.get("generator_version") != version:
            res.older_version += 1
            continue
        source = entry.get("source")
        if not isinstance(source, dict):
            res.no_record.append(subject)
            continue
        sent, before = set(source.get("sent", ())), set(source.get("attributed", ()))
        row = Row(subject, len(sent), len(sent - ids),
                  len(material_of(subject).attributed - before))
        if row.changed:
            res.stale.append(row)
        elif row.new:
            res.new_only.append(row)
        else:
            res.current += 1
    return res


def _table(rows: list[Row]) -> list[str]:
    out = ["| subject | sent units | changed or removed | new units |", "| --- | ---: | ---: | ---: |"]
    out += [f"| {r.subject} | {r.sent} | {r.changed} | {r.new} |"
            for r in sorted(rows, key=lambda r: (-r.changed, -r.new, r.subject))]
    return out


def markdown(res: Result) -> str:
    out = [
        "# forge persona --stale", "",
        f"- generator version: `{res.version}`",
        f"- stale: {len(res.stale)} · new material only: {len(res.new_only)} · "
        f"never generated: {len(res.never)} · no source record: {len(res.no_record)} · "
        f"up to date: {res.current}",
        f"- written by an older generator version (a normal run regenerates them): "
        f"{res.older_version}", "",
        "## Stale", "",
        "A unit whose text was sent to the generator is no longer in the folio. "
        "`forge persona --stale --regenerate` calls the endpoint again for these.", "",
    ]
    out += _table(res.stale) if res.stale else ["None."]
    out += ["", "## New material only", "",
            "Every sent unit is unchanged; units attributed to the person now were not "
            "when the persona was written. Regenerate one with "
            "`forge persona --only <subject> --force`.", ""]
    out += _table(res.new_only) if res.new_only else ["None."]
    out += ["", "## Never generated", ""]
    out += [f"- {s}" for s in res.never] or ["None."]
    out += ["", "## No source record", "",
            "Written before sources were recorded; they cannot be compared.", ""]
    out += [f"- {s}" for s in res.no_record] or ["None."]
    return "\n".join(out) + "\n"


def write(res: Result, root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    path = root / "STALE.md"
    path.write_text(markdown(res), encoding="utf-8")
    return path
