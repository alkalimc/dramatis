"""Fetch: the only stage that costs someone else money.

Consequences of that, all of them deliberate:

* **Resumable by default.** Pages already held are skipped, so an interrupted run
  costs nothing to restart and a re-run after a rule change costs nothing at all.
* **Raw text is kept.** Changing a normalisation rule must never require
  re-fetching. This is what makes the parse rules safe to iterate on.
* **Revision ids are kept per page.** Every downstream record can name the exact
  version it came from, which is what the attribution obligation actually requires.
* **Follow-ups are fetched in the same pass**, so a page whose body is transcluded
  from elsewhere does not have to be noticed by a human first.
"""

from __future__ import annotations

import datetime as dt
from collections.abc import Sequence
from dataclasses import dataclass, field

from .. import baseline as baseline_mod
from ..normalize.guards import Finding, check_drift
from ..pack import Pack
from ..store.archive import Archive
from ..wiki import Wiki


@dataclass
class FetchResult:
    fetched: int = 0
    skipped: int = 0
    missing: list[str] = field(default_factory=list)
    followups: dict[str, int] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)

    @property
    def total(self) -> int:
        return self.fetched + self.skipped


def run(
    wiki: Wiki,
    archive: Archive,
    pack: Pack,
    *,
    limit: int = 0,
    refetch: bool = False,
    progress=None,
) -> FetchResult:
    res = FetchResult()
    titles = archive.titles_in(pack.fetch_seeds)
    if limit:
        titles = titles[:limit]

    have = set() if refetch else archive.have_pages()
    todo = [t for t in titles if t not in have]
    res.skipped = len(titles) - len(todo)
    _into(wiki, archive, todo, res, progress)

    for hook in pack.followups:
        discovered: set[str] = set()
        for row in archive.pages(archive.seed(hook.of_seed)):
            discovered.update(hook.discover(row["title"], row["wikitext"]))
        pending = sorted(t for t in discovered if refetch or t not in archive.have_pages())
        if not pending:
            continue
        if progress is not None:
            progress(f"followup {hook.label or hook.seed}: {len(pending)} pages")
        _into(wiki, archive, pending, res, progress)
        archive.add_seeds(hook.seed, pending)
        res.followups[hook.seed] = res.followups.get(hook.seed, 0) + len(pending)

    # Discovered sets exist only after this stage, so their baselines are checked here
    # rather than in `scope` (where they are legitimately empty). A missing baseline file
    # was already reported by `scope`; saying it again here would only add noise.
    accepted = baseline_mod.load(archive.path)
    if accepted is not None:
        found = {k: archive.count("seeds", "seed=?", (k,)) for k in pack.discovered_seeds}
        res.findings += check_drift(
            found, {k: v for k, v in accepted.seeds.items() if k in found},
            what="discovered", labels={s.key: s.label for s in pack.seeds})

    archive.mark_synced()
    # A full fetch is not an increment; sampling should not present its pages as "changed".
    archive.db.execute("DELETE FROM manifest WHERE key='last_update'")
    publish_holdings(archive, res.missing)
    archive.set_meta("wiki_requests", wiki.requests)
    if res.findings:
        archive.write_findings([f.row() for f in res.findings], stage="fetch")
    archive.commit()
    return res


def publish_holdings(archive: Archive, missing: Sequence[str]) -> None:
    """Republish what the raw cache holds. Every path that adds or drops a body calls this.

    It was once only called here, so an incremental update left the manifest reporting the
    previous run's page count while the cache held more — every report reading it was off
    by exactly the increment, and nothing disagreed loudly enough to notice.
    """
    archive.set_meta("pages_held", archive.count("raw.pages", "wikitext IS NOT NULL"))
    archive.set_meta("pages_missing", sorted(missing)[:200])


def _into(
    wiki: Wiki, archive: Archive, titles: Sequence[str], res: FetchResult, progress=None
) -> None:
    now = _now()
    for i, (title, text, revid) in enumerate(wiki.content(titles)):
        archive.put_page(title, text, revid, now)
        if text is None:
            # A title that was enumerated but has no content is a scope bug, not a
            # transport failure: something claimed the page exists.
            res.missing.append(title)
        else:
            res.fetched += 1
        if i and i % 200 == 0:
            archive.commit()
            if progress is not None:
                progress(f"fetched {i:,}/{len(titles):,}")
    archive.commit()


def _now() -> str:
    return dt.datetime.now(dt.UTC).isoformat(timespec="seconds")
