"""Generate the attribution and notice files that accompany a published corpus.

These are not paperwork. Redistributing text from a volunteer-maintained wiki, over
fiction belonging to a rights holder, is only defensible if every record can be traced to
the page version it came from — and that claim has to be *true*, not asserted. It was
false once: 83% of units had no revision id until sampling caught it.

Both files are generated rather than written. A hand-maintained list of three thousand
pages drifts from the archive within one update, and a drifting attribution file is worse
than none: it makes a false claim about provenance in a document whose only purpose is to
be accurate about provenance.

The templates live in the design repository and are passed in, so the wording (which is a
legal judgement) stays out of the code (which is a mechanism).
"""

from __future__ import annotations

import datetime as dt
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path

from ..store.archive import Archive
from ..store.folio import Folio
from ..wiki import Wiki

#: A page's last editor, from the API. The full contributor list lives in the page
#: history; this is the traceable entry point, not a complete credit — the templates say
#: so explicitly rather than implying the single name is sufficient attribution.
LAST_EDITOR_BATCH = 50


@dataclass
class Attribution:
    pages: list[dict] = field(default_factory=list)
    unit_count: int = 0
    revid_max: int = 0
    units_without_revid: int = 0
    fetched_at: str = ""
    fingerprint: str = ""
    parser_version: int = 0
    pack_version: int = 0
    editors_resolved: int = 0

    @property
    def page_count(self) -> int:
        return len(self.pages)

    @property
    def complete(self) -> bool:
        """Every unit traceable to a source revision. The precondition for publishing."""
        return self.unit_count > 0 and self.units_without_revid == 0


def collect(archive: Archive, folio: Folio) -> Attribution:
    """Gather provenance from the built corpus.

    Reads the folio rather than the archive for unit counts, because the folio is what
    ships: attributing what was built is the point, not attributing what was parsed.
    """
    out = Attribution(
        fetched_at=str(archive.get_meta("fetched_at") or ""),
        fingerprint=str(folio.get_meta("build_fingerprint") or ""),
        parser_version=int(folio.get_meta("parser_version") or 0),
        pack_version=int(folio.get_meta("pack_version") or 0),
    )

    row = folio.db.execute(
        "SELECT COUNT(*) AS n, SUM(revid IS NULL) AS missing, MAX(revid) AS top FROM chunks"
    ).fetchone()
    out.unit_count = int(row["n"] or 0)
    out.units_without_revid = int(row["missing"] or 0)
    out.revid_max = int(row["top"] or 0)

    for r in folio.db.execute(
        "SELECT page, MAX(revid) AS revid, COUNT(*) AS units FROM chunks "
        "GROUP BY page ORDER BY page"
    ):
        out.pages.append({
            "page": r["page"],
            "revid": r["revid"],
            "units": r["units"],
            "editor": "",
        })
    return out


def resolve_editors(wiki: Wiki, attribution: Attribution, *, progress=None) -> int:
    """Fill in each page's last editor.

    Optional: the files are publishable without it, since the source link already points
    at the page history. Worth doing because a named editor is the courteous form of
    credit even when it is not the complete one.
    """
    titles = [p["page"] for p in attribution.pages]
    by_title: dict[str, str] = {}
    for i in range(0, len(titles), LAST_EDITOR_BATCH):
        batch = titles[i: i + LAST_EDITOR_BATCH]
        data = wiki.get(
            action="query", prop="revisions", rvprop="user", titles="|".join(batch))
        for page in data.get("query", {}).get("pages", []):
            revisions = page.get("revisions") or []
            if revisions:
                by_title[page.get("title", "")] = revisions[0].get("user", "")
        if progress is not None:
            progress(f"editors {min(i + LAST_EDITOR_BATCH, len(titles)):,}/{len(titles):,}")

    for entry in attribution.pages:
        entry["editor"] = by_title.get(entry["page"], "")
    attribution.editors_resolved = sum(1 for p in attribution.pages if p["editor"])
    return attribution.editors_resolved


def _page_rows(attribution: Attribution, url_pattern: str) -> str:
    lines: list[str] = []
    for entry in attribution.pages:
        page = entry["page"]
        revid = entry["revid"]
        # The history link, not the current-version link: it is what actually satisfies
        # the attribution requirement for a collectively edited page.
        history = f"https://prts.wiki/index.php?title={page.replace(' ', '_')}&action=history"
        lines.append(
            f"| {page} | {revid or '—'} | {entry['units']:,} | "
            f"{entry['editor'] or '（见页面历史）'} | [历史]({history}) |"
        )
    return "\n".join(lines)


def render(
    attribution: Attribution,
    templates: Iterable[Path],
    outdir: Path,
    *,
    url_pattern: str = "https://prts.wiki/w/{page}",
    issue_url: str = "",
    contact_email: str = "",
) -> list[Path]:
    """Fill the templates and write them next to the corpus.

    Refuses to write when provenance is incomplete. That refusal is the whole point: a
    notice claiming every record is traceable, generated from a corpus where 83% were not,
    would be a false statement in the one document that must not contain any.
    """
    if not attribution.complete:
        raise ValueError(
            f"{attribution.units_without_revid:,} of {attribution.unit_count:,} units have "
            "no source revision. Attribution would claim a traceability that does not "
            "exist — fix the corpus before publishing it."
        )

    values = {
        "PAGE_COUNT": f"{attribution.page_count:,}",
        "UNIT_COUNT": f"{attribution.unit_count:,}",
        "REVID_MAX": f"{attribution.revid_max:,}",
        "FETCHED_AT": attribution.fetched_at or "—",
        "BUILD_FINGERPRINT": attribution.fingerprint or "—",
        "PARSER_VERSION": str(attribution.parser_version),
        "PACK_VERSION": str(attribution.pack_version),
        "ISSUE_URL": issue_url or "（待填）",
        "CONTACT_EMAIL": contact_email or "（待填）",
        "PAGE_ROWS": _page_rows(attribution, url_pattern),
        "GENERATED_AT": dt.datetime.now(dt.UTC).isoformat(timespec="seconds"),
    }

    outdir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for template in templates:
        text = template.read_text(encoding="utf-8")
        for key, value in values.items():
            text = text.replace("{" + key + "}", value)
        destination = outdir / template.name
        destination.write_text(text, encoding="utf-8")
        written.append(destination)
    return written
