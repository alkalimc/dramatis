"""Generate the attribution and notice files that accompany a published corpus.

These are not paperwork. Redistributing text from a volunteer-maintained wiki is only
defensible if every record can be traced to the page version it came from — and that
claim has to be *true*, not asserted. So the generator refuses to write when any unit
lacks a source revision.

Both files are generated rather than written. A hand-maintained page list drifts from the
archive within one update, and a drifting attribution file is worse than none: it makes a
false claim about provenance in a document whose only purpose is to be accurate about it.

The templates live with the pack, so the wording (a legal judgement) stays out of the code
(a mechanism). Everything a template needs that a tool can know — counts, dates, the
site's URL patterns, the contact channels the pack declares — is a placeholder:

    {SITE} {SOURCE_URL_PATTERN} {REVISION_URL_PATTERN} {HISTORY_URL_PATTERN}
    {PAGE_COUNT} {UNIT_COUNT} {REVID_MAX} {SYNCED_AT} {BUILD_FINGERPRINT}
    {PARSER_VERSION} {PACK_VERSION} {ISSUE_URL} {CONTACT_EMAIL} {PAGE_ROWS} {GENERATED_AT}
"""

from __future__ import annotations

import datetime as dt
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlsplit

from ..pack import Pack
from ..store.archive import Archive
from ..store.folio import Folio
from ..wiki import TITLES_LIMIT, Wiki


@dataclass
class Attribution:
    pages: list[dict] = field(default_factory=list)
    unit_count: int = 0
    revid_max: int = 0
    units_without_revid: int = 0
    synced_at: str = ""
    fingerprint: str = ""
    parser_version: int = 0
    pack_version: int = 0

    @property
    def page_count(self) -> int:
        return len(self.pages)

    @property
    def editors_resolved(self) -> int:
        return sum(1 for p in self.pages if p["editor"])

    @property
    def complete(self) -> bool:
        """Every unit traceable to a source revision. The precondition for publishing."""
        return self.unit_count > 0 and self.units_without_revid == 0


def collect(archive: Archive, folio: Folio) -> Attribution:
    """Gather provenance from the built corpus, with editors from the archive's cache.

    Reads the folio rather than the archive for unit counts, because the folio is what
    ships: attributing what was built is the point, not attributing what was parsed.
    """
    out = Attribution(
        synced_at=str(archive.get_meta("synced_at") or ""),
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

    cached = archive.editors()
    for r in folio.db.execute(
        "SELECT page, MAX(revid) AS revid, COUNT(*) AS units FROM chunks "
        "GROUP BY page ORDER BY page"
    ):
        known = cached.get(r["page"])
        out.pages.append({
            "page": r["page"],
            "revid": r["revid"],
            "units": r["units"],
            "editor": known[1] if known and known[0] == r["revid"] else "",
        })
    return out


def stale_editors(attribution: Attribution) -> list[dict]:
    """Pages whose cached editor is missing or belongs to a different revision."""
    return [p for p in attribution.pages if not p["editor"] and p["revid"]]


def resolve_editors(
    wiki: Wiki, archive: Archive, attribution: Attribution, *, progress=None
) -> int:
    """Ask the site who made each revision the cache cannot answer for.

    Asks by revision id, not by title: the author of the revision the corpus holds is
    the one to credit, whatever the page's current revision is. Only uncached (page,
    revid) pairs are requested, batched at the API's limit, and every answer is cached
    in the archive. Returns how many revisions were looked up.
    """
    # Several pages can carry one revision (a unit built from one page and filed under
    # another), so each revision maps to every entry waiting on it.
    todo: dict[int, list[dict]] = {}
    for entry in stale_editors(attribution):
        todo.setdefault(entry["revid"], []).append(entry)
    revids = sorted(todo)
    found: list[tuple[str, int, str]] = []
    for i in range(0, len(revids), TITLES_LIMIT):
        batch = revids[i: i + TITLES_LIMIT]
        data = wiki.get(action="query", prop="revisions", rvprop="user|ids",
                        revids="|".join(str(r) for r in batch))
        for page in data.get("query", {}).get("pages", []):
            for rev in page.get("revisions") or []:
                if not rev.get("user"):
                    continue  # hidden or deleted author: the page history still credits them
                for entry in todo.get(rev.get("revid"), []):
                    entry["editor"] = rev["user"]
                    found.append((entry["page"], entry["revid"], rev["user"]))
        if progress is not None:
            progress(f"editors {min(i + TITLES_LIMIT, len(revids)):,}/{len(revids):,}")
    archive.put_editors(found)
    archive.commit()
    return len(revids)


def _page_rows(attribution: Attribution, pack: Pack) -> str:
    unknown = pack.say("attribution.editor_unknown")
    link = pack.say("attribution.history")
    lines: list[str] = []
    for entry in attribution.pages:
        page = entry["page"]
        # The history link, not the current-version link: it is what actually satisfies
        # the attribution requirement for a collectively edited page.
        history = pack.wiki.history_url.format(page=page.replace(" ", "_"))
        lines.append(
            f"| {page} | {entry['revid'] or '—'} | {entry['units']:,} | "
            f"{entry['editor'] or unknown} | [{link}]({history}) |"
        )
    return "\n".join(lines)


def _site(pack: Pack) -> str:
    parts = urlsplit(pack.wiki.source_url or pack.wiki.api)
    return f"{parts.scheme}://{parts.netloc}" if parts.netloc else ""


def values(attribution: Attribution, pack: Pack) -> dict[str, str]:
    """Every placeholder a template may use, all derived from artifacts or pack config."""
    unset = pack.say("attribution.unset")
    title = "{" + pack.say("attribution.page_placeholder") + "}"
    return {
        "SITE": _site(pack) or unset,
        "SOURCE_URL_PATTERN": pack.wiki.source_url.replace("{page}", title) or unset,
        "REVISION_URL_PATTERN": pack.wiki.revision_url or unset,
        "HISTORY_URL_PATTERN": pack.wiki.history_url.replace("{page}", title) or unset,
        "PAGE_COUNT": f"{attribution.page_count:,}",
        "UNIT_COUNT": f"{attribution.unit_count:,}",
        "REVID_MAX": f"{attribution.revid_max:,}",
        "SYNCED_AT": attribution.synced_at or "—",
        "BUILD_FINGERPRINT": attribution.fingerprint or "—",
        "PARSER_VERSION": str(attribution.parser_version),
        "PACK_VERSION": str(attribution.pack_version),
        "ISSUE_URL": pack.publication.issue_url or unset,
        "CONTACT_EMAIL": pack.publication.contact_email or unset,
        "PAGE_ROWS": _page_rows(attribution, pack),
        "GENERATED_AT": dt.datetime.now(dt.UTC).isoformat(timespec="seconds"),
    }


def render(
    attribution: Attribution, pack: Pack, templates: Iterable[Path], outdir: Path,
) -> list[Path]:
    """Fill the templates and write them next to the corpus.

    Refuses to write when provenance is incomplete: a notice claiming every record is
    traceable, generated from a corpus where some are not, would be a false statement in
    the one document that must not contain any.
    """
    if not attribution.complete:
        raise ValueError(
            f"{attribution.units_without_revid:,} of {attribution.unit_count:,} units have "
            "no source revision. Attribution would claim a traceability that does not "
            "exist — fix the corpus before publishing it."
        )

    filled = values(attribution, pack)
    outdir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for template in templates:
        text = template.read_text(encoding="utf-8")
        for key, value in filled.items():
            text = text.replace("{" + key + "}", value)
        destination = outdir / template.name
        destination.write_text(text, encoding="utf-8")
        written.append(destination)
    return written
