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
    #: In scope but no body held. A previous fetch failed, or a followup registered a
    #: title without retrieving it. Repaired here because nothing else will: once the
    #: title is a seed, the followup loop filters it out as already-known, and the change
    #: feed never mentions a page that did not change.
    missing: list[str] = field(default_factory=list)
    findings: list[Finding] = field(default_factory=list)
    #: Re-enumeration result, carried so `apply` need not repeat it.
    scope: Scope | None = None

    @property
    def nothing_to_do(self) -> bool:
        return not (self.changed or self.added or self.gone or self.missing)


def plan(
    wiki: Wiki, archive: Archive, pack: Pack, *, limit: int = 5000, rescope: bool = True,
    progress=None,
) -> Plan:
    """Work out what to do without touching the archive.

    Separate from `apply` so a human can look first. An update that silently
    removes 400 pages because an enumerator broke is exactly the event this
    separation exists to catch.

    `progress` is not decoration. Re-enumerating the seed sets costs about a minute of
    paged index queries, while walking the change feed for a short increment costs under
    a second — so without a callback the only slow stage is also the only silent one, and
    it reads as a hang.
    """
    def say(msg: str) -> None:
        if progress is not None:
            progress(msg)
    p = Plan(watermark=int(archive.get_meta("watermark", 0) or 0))

    say(f"change feed from rcid={p.watermark:,}")
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

    say(f"changed titles: {len(touched)}")
    in_scope = set(archive.titles_in(pack.fetch_seeds))

    # Gaps first: an archive that claims a page is in scope while holding nothing for it
    # is the state G3 reports as high severity, and it blocks a corpus build. It has to be
    # repairable without a full re-sync.
    held = archive.have_pages()
    p.missing = sorted(t for t in in_scope if t not in held)
    if p.missing:
        say(f"in scope but no body held: {len(p.missing)} — will re-fetch")
    p.changed = sorted(t for t in touched if t in in_scope)
    p.ignored = len(touched) - len(p.changed)

    if rescope:
        say("re-enumerating seed sets (paged index queries; the slow part)")
        fresh = scope_stage.build(wiki, pack, progress=progress)
        p.scope = fresh
        fresh_titles = set(fresh.fetch_titles(pack))
        p.added = sorted(fresh_titles - in_scope)
        # `gone` is computed only over seeds that re-enumeration actually covers.
        # Discovered sets are populated by `fetch` reading a parent page, so `scope` never
        # yields them (`SeedSet.discovered`) — and subtracting a set that was never
        # enumerated makes every discovered title look like it left scope. That deleted
        # them on every incremental run: fetched by the followup, then dropped a few lines
        # later, which is the same silent loss the discovered-seed machinery exists to
        # prevent.
        discovered = set(pack.discovered_seeds)
        enumerated = tuple(k for k in pack.fetch_seeds if k not in discovered)
        p.gone = sorted(set(archive.titles_in(enumerated)) - fresh_titles)
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

    titles = sorted(set(p.changed) | set(p.added) | set(p.missing))
    res = FetchResult()
    if titles:
        _into(wiki, archive, titles, res, progress)

    # Followups have to run here too, not only in `fetch`. A page whose body is
    # transcluded from a subpage is discovered *by reading its parent*, so re-fetching a
    # changed parent can reveal a subpage that no enumeration knows about. Skipping this
    # left those titles registered as seeds with no body held — which `G3` then reports as
    # a high-severity finding, correctly: the archive claimed a page was in scope and had
    # nothing for it.
    for hook in pack.followups:
        discovered: set[str] = set()
        for row in archive.pages(archive.seed(hook.of_seed)):
            discovered.update(hook.discover(row["title"], row["wikitext"]))
        pending = sorted(t for t in discovered if t not in archive.have_pages())
        if not pending:
            continue
        if progress is not None:
            progress(f"followup {hook.label or hook.seed}: {len(pending)} pages")
        _into(wiki, archive, pending, res, progress)
        archive.add_seeds(hook.seed, pending)

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
