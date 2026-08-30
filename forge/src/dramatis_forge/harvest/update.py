"""Incremental update: re-fetch what changed, re-normalise everything.

The watermark is the site's own monotonic change id. The decision procedure is
short because enumeration made it short:

    1. pull change titles newer than the watermark
    2. intersect with the seed sets — **anything outside is ignored without being
       identified**
    3. re-fetch the intersection, plus whatever the seed sets gained
    4. re-run the guards

Step 2 is where the enumerate-don't-classify choice pays a second time. Under a
page-type scheme every changed title would first have to be classified to decide
whether it is wanted, and classification is precisely the unreliable step.

Normalisation is always full, never incremental. It is offline and takes minutes,
whereas incremental normalisation needs invalidation logic — "which records did
this page own" — which is one more place to fail silently. Trading minutes of CPU
for the removal of a whole class of bug is not a close call.
"""

from __future__ import annotations

import datetime as dt
from dataclasses import dataclass, field

from ..normalize.guards import Finding
from ..pack import Pack
from ..store.archive import Archive
from ..wiki import Wiki
from . import scope as scope_stage
from .scope import Scope


@dataclass
class Plan:
    watermark: int = 0
    new_watermark: int = 0
    changed: list[str] = field(default_factory=list)
    ignored: int = 0
    added: list[str] = field(default_factory=list)
    gone: list[str] = field(default_factory=list)
    findings: list[Finding] = field(default_factory=list)
    #: Re-enumeration result, carried so `apply` need not repeat it.
    scope: Scope | None = None

    @property
    def nothing_to_do(self) -> bool:
        return not (self.changed or self.added or self.gone)


def plan(
    wiki: Wiki, archive: Archive, pack: Pack, *, limit: int = 5000, rescope: bool = True
) -> Plan:
    """Work out what to do without touching the archive.

    Separate from `apply` so a human can look first. An update that silently
    removes 400 pages because an enumerator broke is exactly the event this
    separation exists to catch.
    """
    p = Plan(watermark=int(archive.get_meta("watermark", 0) or 0))

    touched: dict[str, int] = {}
    reached = False
    for rc in wiki.recentchanges(pack.wiki.watch_namespaces):
        rcid = int(rc.get("rcid", 0))
        p.new_watermark = max(p.new_watermark, rcid)
        if rcid <= p.watermark:
            reached = True
            break
        touched[rc["title"]] = max(touched.get(rc["title"], 0), rcid)
        if len(touched) > limit:
            break
    if not reached and p.watermark:
        p.findings.append(Finding(
            "G1", "高",
            f"never reached watermark rcid={p.watermark} within {limit} changed pages — "
            "the increment is incomplete; rebuild rather than trust it",
        ))
    p.new_watermark = max(p.new_watermark, p.watermark)

    in_scope = set(archive.titles_in(pack.fetch_seeds))
    p.changed = sorted(t for t in touched if t in in_scope)
    p.ignored = len(touched) - len(p.changed)

    if rescope:
        fresh = scope_stage.build(wiki, pack)
        p.scope = fresh
        fresh_titles = set(fresh.fetch_titles(pack))
        p.added = sorted(fresh_titles - in_scope)
        p.gone = sorted(in_scope - fresh_titles)
        p.findings += fresh.findings
        if p.gone:
            p.findings.append(Finding(
                "G1", "低" if len(p.gone) < 20 else "高",
                f"{len(p.gone)} titles left the seed sets — deletion or a broken enumerator",
            ))
    return p


def apply(wiki: Wiki, archive: Archive, pack: Pack, p: Plan, *, progress=None) -> dict:
    from ..normalize.runner import run as normalize
    from .fetch import _into, FetchResult

    if p.scope is not None:
        scope_stage.write(archive, p.scope, pack)

    titles = sorted(set(p.changed) | set(p.added))
    res = FetchResult()
    if titles:
        _into(wiki, archive, titles, res, progress)
    for t in p.gone:
        archive.drop_page(t)

    archive.set_meta("watermark", p.new_watermark)
    archive.set_meta("updated_at", dt.datetime.now(dt.UTC).isoformat(timespec="seconds"))
    archive.commit()

    report = normalize(archive, pack, progress=progress)
    return {
        "refetched": len(titles),
        "removed": len(p.gone),
        "watermark": p.new_watermark,
        "report": report,
    }


def init_watermark(wiki: Wiki, archive: Archive, pack: Pack) -> int:
    """Pin the watermark to the site's newest change.

    Called once after the first full harvest. Without it the first `update` treats
    the site's entire change history as pending.
    """
    top = wiki.latest_change_id(pack.wiki.watch_namespaces)
    archive.set_meta("watermark", top)
    archive.commit()
    return top
