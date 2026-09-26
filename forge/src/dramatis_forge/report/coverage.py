"""Render the coverage report.

Generic: the annotated table of contents comes from the pack, the counts come from the
archive, every word of prose comes from `Pack.say`, and this module only knows how to
evaluate a measure and lay out a page.
"""

from __future__ import annotations

from ..normalize.guards import GUARDS
from ..normalize.records import ORDER
from ..pack import Pack
from ..store.archive import Archive
from ..text import table_head

DISPOSITION_ORDER = {"archived": 0, "partial": 1, "excluded": 2}
#: Rows in the voice-trigger table.
TOP_TRIGGERS = 12


def _measure(archive: Archive, spec: str | None) -> str:
    if not spec:
        return "—"
    if spec.startswith("seed:"):
        return f"{archive.count('seeds', 'seed=?', (spec[5:],)):,}"
    if spec.startswith("sql:"):
        try:
            return f"{archive.scalar(spec[4:]) or 0:,}"
        except Exception:
            return "?"
    return "—"


def build(archive: Archive, pack: Pack) -> tuple[str, list[tuple[str, str, str, str]]]:
    say = pack.say
    rows = [
        (row.section, row.disposition, _measure(archive, row.measure), row.reason)
        for row in pack.coverage
    ]

    counts = archive.get_meta("record_counts") or {}
    chars = archive.get_meta("record_chars") or {}
    tally = archive.get_meta("guard_tally") or {}
    recon = archive.get_meta("reconciliation") or {}
    tally_of = {d: sum(1 for r in pack.coverage if r.disposition == d) for d in DISPOSITION_ORDER}

    md: list[str] = [say("coverage.title", pack=pack.name), "", say("coverage.intro"), ""]
    if say("coverage.policy"):
        md += [say("coverage.policy"), ""]
    md += [
        say("coverage.archive", name=archive.path.name),
        say("coverage.synced", at=archive.get_meta("synced_at", "—")),
        say("coverage.versions", parser=archive.get_meta("parser_version", "—"),
            pack=archive.get_meta("pack_version", "—")),
        say("coverage.pages", n=int(archive.get_meta("pages_held", 0) or 0)),
        say("coverage.dispositions", archived=tally_of["archived"],
            partial=tally_of["partial"], excluded=tally_of["excluded"]),
        "",
        say("coverage.sources"),
        "",
        *table_head(say("coverage.sources_head")),
    ]
    for section, disposition, count, reason in sorted(
        rows, key=lambda r: (DISPOSITION_ORDER.get(r[1], 9), r[0])
    ):
        md.append(f"| {section} | {disposition} | {count} | {reason} |")

    md += ["", say("coverage.records"), "", *table_head(say("coverage.records_head"))]
    for kind in ORDER:
        n = counts.get(kind, 0)
        c = chars.get(kind, 0)
        md.append(f"| `{kind}` | {say('record.' + kind)} | {n:,} | " + (f"{c:,} |" if c else "— |"))
    md += ["", say("coverage.total_chars", mchars=sum(chars.values()) / 1e6), ""]

    md += [say("coverage.identity"), "", *table_head(say("coverage.identity_head"))]
    md.append(f"| {say('coverage.roster_pages')} | {archive.count('forms'):,} |")
    md.append(f"| {say('coverage.persons')} | {archive.count('persons'):,} |")
    for r in archive.db.execute(
        "SELECT kind, COUNT(*) AS n FROM forms WHERE kind<>'canonical' GROUP BY kind ORDER BY kind"
    ):
        md.append(f"| {say('coverage.form_kind', kind=r['kind'])} | {r['n']:,} |")
    md.append(f"| {say('coverage.multi_form')} | {archive.count('persons', 'form_count>1'):,} |")
    md.append("")

    md += [say("coverage.scenes"), "", *table_head(say("coverage.scenes_head"))]
    for r in archive.db.execute(
        "SELECT s.category AS t, COUNT(DISTINCT s.id) AS n, COUNT(l.seq) AS m "
        "FROM scenes s LEFT JOIN lines l ON l.scene = s.id "
        "GROUP BY s.category ORDER BY m DESC"
    ):
        md.append(f"| {r['t'] or say('coverage.uncategorised')} | {r['n']:,} | {r['m']:,} |")

    md += ["", say("coverage.voices", n=TOP_TRIGGERS), "", *table_head(say("coverage.voices_head"))]
    for r in archive.db.execute(
        "SELECT trigger AS t, COUNT(*) AS n FROM voices GROUP BY trigger "
        "ORDER BY n DESC LIMIT ?", (TOP_TRIGGERS,)
    ):
        md.append(f"| {r['t'] or '—'} | {r['n']:,} |")

    md += ["", say("coverage.guards"), "", *table_head(say("coverage.guards_head"))]
    for guard in GUARDS:
        hi, lo = tally.get(guard, (0, 0))
        md.append(f"| {guard} {say('guard.' + guard)} | {hi:,} | {lo:,} |")
    md += ["", say("coverage.guards_note"), ""]

    if recon.get("produced"):
        md += [say("coverage.recon"), "", say("coverage.recon_intro"), "",
               *table_head(say("coverage.recon_head"))]
        for kind in ORDER:
            made = recon["produced"].get(kind)
            if made is None:
                continue
            md.append(
                f"| `{kind}` | {made:,} | {recon.get('stored', {}).get(kind, 0):,} | "
                f"{recon.get('ignored', {}).get(kind, 0):,} |")
        md.append("")

    return "\n".join(md), rows
