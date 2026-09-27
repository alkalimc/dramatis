"""A broad, reproducible reading sample of the archive, rebuilt after every sync.

Guards catch what fails loudly. What they cannot catch is a parse that is quietly wrong,
and the only defence against that is a person reading output beside input. This module
decides *which* pages that person reads, so the choice is neither ad hoc nor biased
towards the pages someone already suspected.

For every route (a seed set and the reader that handles it):

* a few pages at random, seeded from the archive watermark, so one sync always yields
  the same sample and the next sync yields a fresh one;
* the edge cases: largest source, smallest non-empty source, most records, a page with
  guard findings, and a page that produced nothing (with its stated reason);
* every page of a route small enough to read in full.

Across routes: at least one page yielding each record kind, every alias-producing page,
a person with several forms, and — after an incremental update — a handful of pages that
changed and every page that was added.

Each page is written with `inspect.dump` into a subdirectory per route, and `INDEX.md`
lists every page, why it was chosen and where its three files are.
"""

from __future__ import annotations

import random
import re
import shutil
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from ..normalize.guards import HIGH
from ..pack import Pack
from ..store.archive import Archive
from ..text import table_head
from . import inspect as inspect_mod

#: Random pages per route.
PER_ROUTE = 5
#: A route this small is sampled in full.
SMALL_ROUTE = 8
#: Pages sampled per record kind, and from the changed pages of an update.
PER_KIND = 2
CHANGED = 8
MULTI_FORM = 2

#: Example pages quoted per residue pattern in the formatting-quality section.
RESIDUE_EXAMPLES = 3

#: Markup that should never survive normalisation into record text. Generic MediaWiki and
#: HTML only; a pack's own script syntax is checked through `InlineRules.macro_shape`.
_RESIDUE: dict[str, str] = {
    "template": r"\{\{|\}\}",
    "link": r"\[\[|\]\]",
    "bold": r"'''",
    "tag": r"</?[A-Za-z][\w-]*(?:\s[^<>]*)?/?>",
    "entity": r"&(?:nbsp|amp|lt|gt|quot|#\d+);",
    "table": r"(?:^|\n)\s*(?:\{\||\|[-}])",
    "magic": r"__[A-Z]+__",
}

#: Which text column of each record table is prose a reader sees, and its page column.
_TEXT_OF: dict[str, tuple[str, str, str]] = {
    "line": ("lines", "text", "scene"), "choice": ("choices", "options", "scene"),
    "voice": ("voices", "text", "page"), "lore": ("lore", "text", "page"),
    "letter": ("letters", "body", "page"), "term": ("terms", "term", "page"),
    "char_ref": ("char_refs", "description", "page"), "dossier": ("dossiers", "sections", "page"),
}

#: Which table (and column) says a record came from a page.
_PAGE_OF: dict[str, tuple[str, str]] = {
    "scene": ("scenes", "id"), "line": ("lines", "scene"), "choice": ("choices", "scene"),
    "dossier": ("dossiers", "page"), "voice": ("voices", "page"), "lore": ("lore", "page"),
    "letter": ("letters", "page"), "term": ("terms", "page"), "char_ref": ("char_refs", "page"),
}


@dataclass
class Sample:
    route: str
    title: str
    #: (reason key, fields) pairs, rendered through `Pack.say("samples.why.<key>")`.
    why: list[tuple[str, dict]] = field(default_factory=list)


@dataclass
class Selection:
    seed: str
    samples: dict[tuple[str, str], Sample] = field(default_factory=dict)

    def add(self, route: str, title: str, why: str, **fields: object) -> None:
        entry = self.samples.setdefault((route, title), Sample(route, title))
        if all(r != why for r, _ in entry.why):
            entry.why.append((why, fields))

    def routes(self) -> list[str]:
        return sorted({route for route, _ in self.samples})

    def of(self, route: str) -> list[Sample]:
        return sorted((s for (r, _), s in self.samples.items() if r == route),
                      key=lambda s: s.title)


def _route_label(pack: Pack, seed: str, title: str) -> str | None:
    route = pack.route_for(seed, title)
    if route is None:
        return None
    return f"{seed} · {route.label or seed}"


def _records_per_page(archive: Archive) -> tuple[Counter, dict[str, set[str]]]:
    """Records attributed to each page, and which record kinds each page yields."""
    total: Counter = Counter()
    kinds: dict[str, set[str]] = {}
    for kind, (table, column) in _PAGE_OF.items():
        for page, n in archive.db.execute(f"SELECT {column}, COUNT(*) FROM {table} GROUP BY 1"):
            total[page] += n
            kinds.setdefault(kind, set()).add(page)
    return total, kinds


def select(archive: Archive, pack: Pack) -> Selection:
    """Choose the sample. Deterministic for a given archive state."""
    watermark = archive.get_meta("watermark", 0) or 0
    sel = Selection(seed=f"{watermark}:{archive.get_meta('synced_at', '')}")
    held = archive.have_pages()
    sizes = {r[0]: r[1] for r in archive.db.execute(
        "SELECT title, LENGTH(wikitext) FROM raw.pages WHERE wikitext IS NOT NULL")}
    records, by_kind = _records_per_page(archive)
    findings: dict[str, list[tuple[str, str, str]]] = {}
    for guard, severity, page, detail in archive.db.execute(
        "SELECT guard, severity, page, detail FROM guard_findings WHERE page IS NOT NULL "
        "ORDER BY severity <> ?, guard, page", (HIGH,)
    ):
        findings.setdefault(page, []).append((guard, severity, detail))

    route_of: dict[str, str] = {}
    members: dict[str, list[str]] = {}
    for seed in pack.fetch_seeds:
        for title in archive.seed(seed):
            label = _route_label(pack, seed, title)
            if label is None or title not in held:
                continue
            route_of.setdefault(title, label)
            members.setdefault(label, []).append(title)

    for label, titles in sorted(members.items()):
        rng = random.Random(f"{sel.seed}:{label}")
        if len(titles) <= SMALL_ROUTE:
            for t in titles:
                sel.add(label, t, "random")
        else:
            for t in rng.sample(titles, PER_ROUTE):
                sel.add(label, t, "random")
        sel.add(label, max(titles, key=lambda t: (sizes.get(t, 0), t)), "largest")
        sel.add(label, min(titles, key=lambda t: (sizes.get(t, 0), t)), "smallest")
        busiest = max(titles, key=lambda t: (records.get(t, 0), t))
        if records.get(busiest):
            sel.add(label, busiest, "most_records")
        flagged = [t for t in titles if t in findings]
        if flagged:
            sel.add(label, flagged[0], "findings")
        # G3 records exactly the pages that produced nothing, with the stated reason.
        # Counting records per page cannot tell: a record may be filed under another
        # page's id (a transcluded body's lines belong to its parent's scene).
        for t in titles:
            reason = next((d for g, _s, d in findings.get(t, []) if g == "G3"), None)
            if reason is not None:
                sel.add(label, t, "empty", reason=reason)
                break

    for kind, pages in sorted(by_kind.items()):
        candidates = sorted(p for p in pages if p in route_of)
        rng = random.Random(f"{sel.seed}:kind:{kind}")
        for t in rng.sample(candidates, min(PER_KIND, len(candidates))):
            sel.add(route_of[t], t, "kind", kind=kind)

    for title in sorted(pack.alias_pages):
        if title in route_of:
            sel.add(route_of[title], title, "alias")

    multi = [r[0] for r in archive.db.execute(
        "SELECT person_id FROM persons WHERE form_count > 1 ORDER BY form_count DESC, person_id")]
    for person in multi[:MULTI_FORM]:
        if person in route_of:
            sel.add(route_of[person], person, "multi_form")

    last = archive.get_meta("last_update") or {}
    changed = [t for t in last.get("changed", []) if t in route_of]
    rng = random.Random(f"{sel.seed}:changed")
    for t in rng.sample(changed, min(CHANGED, len(changed))):
        sel.add(route_of[t], t, "changed")
    for t in last.get("added", []):
        if t in route_of:
            sel.add(route_of[t], t, "added")
    return sel


@dataclass
class Residue:
    kind: str
    pattern: str
    hits: int
    total: int
    examples: list[tuple[str, str]] = field(default_factory=list)


def residue(archive: Archive, pack: Pack) -> list[Residue]:
    """Markup left in record text after normalisation: one row per (record kind, pattern).

    Guards report constructs the rules did not predict *while parsing*; this reads the
    output afterwards, so it also catches markup a rule let through on purpose and got
    wrong. A clean corpus yields an empty list.
    """
    patterns = dict(_RESIDUE)
    if pack.inline.macro_shape:
        patterns["macro"] = pack.inline.macro_shape
    compiled = {name: re.compile(p) for name, p in patterns.items()}
    out: list[Residue] = []
    for kind, (table, column, page) in _TEXT_OF.items():
        rows = archive.db.execute(f"SELECT {page}, {column} FROM {table}").fetchall()
        for name, rx in compiled.items():
            hits = [(p, t) for p, t in rows if t and rx.search(t)]
            if not hits:
                continue
            found = Residue(kind, name, len(hits), len(rows))
            for p, t in hits[:RESIDUE_EXAMPLES]:
                m = rx.search(t)
                found.examples.append((p, t[max(0, m.start() - 30): m.end() + 30]))
            out.append(found)
    return out


def _status(archive: Archive, pack: Pack) -> list[str]:
    """What this sync did and what state it left the archive in."""
    say = pack.say
    last = archive.get_meta("last_update")
    lines = [say("samples.status"), ""]
    lines.append(say(
        "samples.sync",
        at=archive.get_meta("synced_at", "—"), watermark=archive.get_meta("watermark", 0) or 0,
        kind=say("samples.sync.update") if last is not None else say("samples.sync.full"),
        pages=archive.get_meta("pages_held", 0) or 0))
    if last is not None:
        lines.append(say("samples.delta", changed=len(last.get("changed", [])),
                         added=len(last.get("added", [])), gone=len(last.get("gone", []))))
        for title in last.get("gone", []):
            lines.append(f"  - {say('samples.gone', title=title)}")
    lines.append("")

    now = archive.get_meta("record_counts") or {}
    before = archive.get_meta("record_counts_previous") or {}
    lines += table_head(say("samples.records_head"))
    for kind in sorted(set(now) | set(before)):
        n, b = now.get(kind, 0), before.get(kind)
        delta = "—" if b is None else f"{n - b:+,}"
        lines.append(f"| `{kind}` | {pack.say(f'record.{kind}')} | {n:,} | {delta} |")
    lines.append("")

    tally = archive.tally_from_table()
    lines += table_head(say("samples.guards_head"))
    for guard, (high, low) in tally.items():
        lines.append(f"| {guard} | {say(f'guard.{guard}')} | {high:,} | {low:,} |")
    lines.append("")
    return lines


def _quality(archive: Archive, pack: Pack) -> list[str]:
    say = pack.say
    found = residue(archive, pack)
    lines = [say("samples.quality"), ""]
    if not found:
        return lines + [say("samples.quality_clean"), ""]
    lines += table_head(say("samples.quality_head"))
    for r in found:
        examples = " · ".join(
            f"{p}: `{e.replace('`', '′').replace(chr(10), ' ').replace('|', '/')}`"
            for p, e in r.examples)
        lines.append(f"| `{r.kind}` | {r.pattern} | {r.hits:,} / {r.total:,} | {examples} |")
    return lines + [""]


def write(archive: Archive, pack: Pack, outdir: Path) -> tuple[Path, Selection]:
    """Clear `outdir`, dump every selected page, and write `INDEX.md`."""
    if outdir.exists():
        shutil.rmtree(outdir)
    outdir.mkdir(parents=True)
    sel = select(archive, pack)
    say = pack.say

    lines = [say("samples.title"), "", say("samples.intro"), "",
             say("samples.meta", watermark=archive.get_meta("watermark", 0) or 0,
                 pages=len(sel.samples), routes=len(sel.routes())), ""]
    lines += _status(archive, pack)
    lines += _quality(archive, pack)
    for route in sel.routes():
        folder = inspect_mod.safe_name(route.replace(" · ", "-"))
        lines += [say("samples.route", route=route), "", *table_head(say("samples.head"))]
        for sample in sel.of(route):
            written = inspect_mod.dump(archive, pack, sample.title, outdir / folder)
            if not written:
                continue
            rel = [f"{folder}/{p.name}" for p in written]
            why = "; ".join(say(f"samples.why.{key}", **fields) for key, fields in sample.why)
            files = say("samples.files", source=rel[0], records=rel[1], archived=rel[2])
            lines.append(f"| {sample.title} | {why.replace('|', '/')} | {files} |")
        lines.append("")

    index = outdir / "INDEX.md"
    index.write_text("\n".join(lines), encoding="utf-8")
    return index, sel
