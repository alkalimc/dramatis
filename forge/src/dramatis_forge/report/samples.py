"""A reading sample of the archive, rebuilt at the end of every build.

Guards catch what fails loudly. What they cannot catch is a parse that is quietly wrong,
and the only defence against that is a person reading output beside input. This module
decides *which* pages that person reads. Nothing is random: the same archive always
yields the same sample, and what changes between two samples is what the sync changed.

* **This sync's changes, all of them**: every held page whose revision changed, every
  page added, and every page removed. Removed pages are read from the snapshot the sync
  took before deleting them (`raw.removed`).
* **Boundaries, per route** (a seed set and the reader that handles it): the first page by
  title, the largest source, the smallest non-empty source, the page with most records and
  the first page with guard findings, among the pages that produce units. A page that
  produces none (an explained empty page, a page whose person left the roster) is never
  sampled: `INDEX.md` lists those by reason, one page name per line.
* **Coverage**: the first page yielding each record kind, every alias-producing page, and
  the first person with several forms.

`INDEX.md` starts with the sync's status and a scan for markup left in record text, then
lists every sampled page with why it was chosen and links to its three files
(`inspect.dump`: source, records, archived text).
"""

from __future__ import annotations

import json
import re
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from ..archive import Archive
from ..folio import Folio
from ..guards import HIGH
from ..normalize import YIELD_DETAIL
from ..pack import Pack
from ..text import cell, table_head
from . import inspect as inspect_mod

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
    "scene": ("scenes", "source_page"), "line": ("lines", "scene"), "choice": ("choices", "scene"),
    "dossier": ("dossiers", "page"), "voice": ("voices", "page"), "lore": ("lore", "page"),
    "letter": ("letters", "page"), "term": ("terms", "page"), "char_ref": ("char_refs", "page"),
}

#: Sub-directory holding this sync's changed, added and removed pages.
CHANGES = "changes"
#: The change kinds of `last_update`, in the order the index lists them.
CHANGE_KINDS = ("changed", "added", "removed")


@dataclass
class Selection:
    """route -> title -> [(reason key, fields)], rendered via `samples.why.<key>`."""

    routes: dict[str, dict[str, list[tuple[str, dict]]]] = field(default_factory=dict)
    #: (reason key, fields) -> pages held but producing no unit, by title.
    silent: dict[tuple[str, tuple], list[str]] = field(default_factory=dict)

    def add(self, route: str, title: str, why: str, **fields: object) -> None:
        reasons = self.routes.setdefault(route, {}).setdefault(title, [])
        if (why, fields) not in reasons:
            reasons.append((why, fields))

    @property
    def count(self) -> int:
        return sum(len(v) for v in self.routes.values())


def _records_per_page(archive: Archive) -> tuple[Counter, dict[str, set[str]]]:
    """Records attributed to each page, and which pages yield each record kind."""
    total: Counter = Counter()
    kinds: dict[str, set[str]] = {}
    for kind, (table, column) in _PAGE_OF.items():
        for page, n in archive.db.execute(f"SELECT {column}, COUNT(*) FROM {table} GROUP BY 1"):
            total[page] += n
            kinds.setdefault(kind, set()).add(page)
    return total, kinds


def unit_pages(archive: Archive, folio: Folio) -> set[str]:
    """Pages that produced something the folio keeps: a unit (the page a unit cites, the page
    a scene body was read from, a form page whose text is stored once with another form) or
    glossary terms, which become aliases rather than units."""
    pages = {r[0] for r in folio.db.execute("SELECT DISTINCT page FROM chunks")}
    pages.update(r[0] for r in archive.db.execute(
        "SELECT source_page FROM scenes WHERE source_page IS NOT NULL AND id IN "
        "(SELECT value FROM json_each(?))", (json.dumps(sorted(pages)),)))
    pages.update(r[0] for r in archive.db.execute("SELECT DISTINCT page FROM terms"))
    for copies in (folio.get_meta("form_copies") or {}).values():
        pages.update(copies)
    return pages


def select(archive: Archive, pack: Pack, producing: set[str] | None = None) -> Selection:
    """The boundary and coverage samples. Deterministic for a given archive state.

    `producing` is the set of pages that made units; without it (no folio yet) a page counts
    when the archive holds records for it.
    """
    sel = Selection()
    held = archive.have_pages()
    sizes = {r[0]: r[1] for r in archive.db.execute(
        "SELECT title, LENGTH(wikitext) FROM raw.pages WHERE wikitext IS NOT NULL")}
    records, by_kind = _records_per_page(archive)
    findings: dict[str, list[tuple[str, str]]] = {}
    for guard, page, detail in archive.db.execute(
        "SELECT guard, page, detail FROM guard_findings WHERE page IS NOT NULL "
        "ORDER BY severity <> ?, guard, page", (HIGH,)
    ):
        findings.setdefault(page, []).append((guard, detail))

    route_of: dict[str, str] = {}
    members: dict[str, list[str]] = {}
    layouts = {s.key: s.layout for s in pack.seeds if s.layout is not None}
    stratified: set[str] = set()
    layered: set[str] = set()
    in_roster = set(archive.person_of())
    dossier_pages = {r[0] for r in archive.db.execute("SELECT page FROM dossiers")}

    def produces(title: str) -> bool:
        if title in pack.alias_pages:  # its aliases are the output
            return True
        return title in producing if producing is not None else bool(records.get(title))

    def why_silent(title: str) -> tuple[str, tuple]:
        if title in dossier_pages and title not in in_roster:
            return ("roster", ())
        reason = next((d for g, d in findings.get(title, [])
                       if g == "G3" and not d.startswith(YIELD_DETAIL)), "")
        if not reason:  # a parser's own account of why the page has nothing to say
            reason = next((d for g, d in findings.get(title, []) if g == "G2"), "")
        return ("empty", (("reason", reason),)) if reason else ("none", ())

    for seed in pack.fetch_seeds:
        for title in archive.seed(seed):  # sorted by title
            route = pack.route_for(seed, title)
            if route is None or title not in held:
                continue
            label = f"{seed} · {route.label or seed}"
            route_of.setdefault(title, label)
            if not produces(title):
                sel.silent.setdefault(why_silent(title), []).append(title)
                continue
            members.setdefault(label, []).append(title)
            if seed in layouts:
                # Layers are drawn from pages that produced units.
                row = archive.page(title)
                klass = layouts[seed](title, (row["wikitext"] or "") if row else "")
                layered.add(label)
                layer = f"{label} · {klass}"
                members.setdefault(layer, []).append(title)
                stratified.add(layer)

    for label, titles in sorted(members.items()):
        if label not in layered:  # the layout classes below stand in for the set's own
            sel.add(label, titles[0], "first")
            sel.add(label, max(titles, key=lambda t: (sizes.get(t, 0), t)), "largest")
        if label in stratified:
            continue  # a layout class is read through its first and largest page
        if label not in layered:
            sel.add(label, min(titles, key=lambda t: (sizes.get(t, 0), t)), "smallest")
        busiest = max(titles, key=lambda t: (records.get(t, 0), t))
        if records.get(busiest):
            sel.add(label, busiest, "most_records")
        flagged = next((t for t in titles if t in findings), None)
        if flagged is not None:
            sel.add(label, flagged, "findings")

    for kind, pages in sorted(by_kind.items()):
        first = min((p for p in pages if p in route_of and produces(p)), default=None)
        if first is not None:
            sel.add(route_of[first], first, "kind", kind=kind)
    for title in sorted(pack.alias_pages):
        if title in route_of and produces(title):
            sel.add(route_of[title], title, "alias")
    person = archive.scalar(
        "SELECT person_id FROM persons WHERE form_count > 1 ORDER BY person_id LIMIT 1")
    if person in route_of and produces(person):
        sel.add(route_of[person], person, "multi_form")
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


def _last(archive: Archive) -> dict:
    last = archive.get_meta("last_update") or {}
    return {"kind": last.get("kind", ""), "first": bool(last.get("first")),
            **{k: list(last.get(k, [])) for k in CHANGE_KINDS}}


def _status(archive: Archive, pack: Pack, last: dict) -> list[str]:
    """What the last sync did and what state it left the archive in."""
    say = pack.say
    kind = ("first" if last["first"] else last["kind"]) or "none"
    lines = [say("samples.status"), "", say(
        "samples.sync", kind=say(f"samples.sync.{kind}"),
        at=archive.get_meta("synced_at", "—"), watermark=archive.get_meta("watermark", 0) or 0,
        pages=archive.pages_held())]
    lines += [say("samples.delta", **{k: len(last[k]) for k in CHANGE_KINDS}), ""]

    now = archive.get_meta("record_counts") or {}
    before = archive.get_meta("record_counts_previous") or {}
    lines += table_head(say("samples.records_head"))
    for kind in sorted(set(now) | set(before)):
        n, b = now.get(kind, 0), before.get(kind)
        delta = "—" if b is None else f"{n - b:+,}"
        lines.append(f"| `{kind}` | {say(f'record.{kind}')} | {n:,} | {delta} |")
    lines.append("")

    lines += table_head(say("samples.guards_head"))
    for guard, (high, low) in archive.tally().items():
        lines.append(f"| {guard} | {say(f'guard.{guard}')} | {high:,} | {low:,} |")
    return lines + [""]


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


def _row(pack: Pack, title: str, why: str, written: list[Path], outdir: Path) -> str:
    if not written:
        return f"| {cell(title)} | {cell(why)} | — |"
    rel = [p.relative_to(outdir).as_posix() for p in written]
    files = pack.say("samples.files", source=rel[0], records=rel[1], archived=rel[2])
    return f"| {cell(title)} | {cell(why)} | {files} |"


def _prune(outdir: Path, kept: set[Path]) -> None:
    """Remove every file under `outdir` that is not in `kept`, and the directories left empty."""
    for path in sorted(outdir.rglob("*"), key=lambda p: len(p.parts), reverse=True):
        if path.is_dir():
            if not any(path.iterdir()):
                path.rmdir()
        elif path not in kept:
            path.unlink()


def write(archive: Archive, pack: Pack, outdir: Path,
          folio_path: Path | None = None) -> tuple[Path, int]:
    """Dump every selected page and write `INDEX.md`, leaving nothing of an earlier run.

    Files are overwritten in place and what this run did not write is removed afterwards,
    rather than deleting the whole tree first: a synced folder that sees a directory vanish
    and reappear under the same name keeps both, as an empty "name 2".
    """
    outdir.mkdir(parents=True, exist_ok=True)
    kept: set[Path] = set()
    say = pack.say
    last = _last(archive)
    producing = None
    if folio_path is not None and folio_path.exists():
        with Folio(folio_path, readonly=True) as folio:
            producing = unit_pages(archive, folio)
    sel = select(archive, pack, producing)
    removed = {r["title"]: r for r in archive.removed()}
    changes = sum(len(last[k]) for k in CHANGE_KINDS)

    lines = [say("samples.title"), "", say("samples.intro"), ""]
    lines += _status(archive, pack, last)
    lines += _quality(archive, pack)

    lines += [say("samples.changes"), ""]
    if not changes:
        lines += [say("samples.changes_none"), ""]
    else:
        lines += table_head(say("samples.head"))
        for kind in CHANGE_KINDS:
            for title in last[kind]:
                if kind == "removed":
                    snap = removed.get(title)
                    written = (inspect_mod.dump_snapshot(pack, snap, outdir / CHANGES)
                               if snap is not None else [])
                else:
                    written = inspect_mod.dump(archive, pack, title, outdir / CHANGES)
                kept.update(written)
                lines.append(_row(pack, title, say(f"samples.why.{kind}"), written, outdir))
        lines.append("")

    lines += [say("samples.boundaries"), ""]
    for route in sorted(sel.routes):
        folder = outdir / inspect_mod.safe_name(route.replace(" · ", "-"))
        lines += [say("samples.route", route=route), "", *table_head(say("samples.head"))]
        for title, reasons in sorted(sel.routes[route].items()):
            why = "; ".join(say(f"samples.why.{key}", **fields) for key, fields in reasons)
            written = inspect_mod.dump(archive, pack, title, folder)
            kept.update(written)
            lines.append(_row(pack, title, why, written, outdir))
        lines.append("")

    if sel.silent:
        lines += [say("samples.silent"), "", say("samples.silent_note"), ""]
        for (key, fields), titles in sorted(sel.silent.items()):
            lines += [say(f"samples.silent.{key}", **dict(fields)), ""]
            lines += [f"- {cell(t)}" for t in sorted(set(titles))]
            lines.append("")

    index = outdir / "INDEX.md"
    index.write_text("\n".join(lines), encoding="utf-8")
    kept.add(index)
    _prune(outdir, kept)
    return index, sel.count + changes
