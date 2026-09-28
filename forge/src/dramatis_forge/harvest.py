"""Harvest: the only part of the forge that talks to the wiki.

Three ideas carry it.

**Enumerate, do not classify.** A seed set is a query whose answer the *site itself*
defines: rows of a structured table, members of a category, a namespace, a hand-listed
set of index pages. The result is closed, so "uncovered" is not a state that exists and a
count drift is a signal rather than noise. Inferring a page type from its markup instead
leaves an unbounded "unsure" middle that can only be audited by re-reading everything.

**Fetch politely and once.** Bodies are kept with their revision ids, so changing a parse
rule never costs a request and every record can name the version it came from. A run that
is interrupted resumes where it stopped: held pages are skipped.

**Stay level through the change feed.** The watermark is the site's own monotonic change
id. An incremental sync pulls the titles changed since it, intersects them with the seed
sets (anything outside is ignored without being identified), re-fetches the intersection
plus whatever the sets gained, and drops what left them. Normalisation afterwards is always
a full rebuild: it is offline and takes minutes, and incremental normalisation would need
"which records did this page own" bookkeeping, one more place to fail silently.

Both sync kinds share one `apply`, so a full sync and an increment leave the archive in the
same shape: watermark, sync time, and a record of what changed for the sample to show.
Pages that leave scope are snapshotted into `raw.removed` before they are deleted.
"""

from __future__ import annotations

import datetime as dt
from collections.abc import Sequence
from dataclasses import dataclass, field

from .archive import Archive
from .guards import HIGH, LOW, Finding
from .pack import HarvestContext, Pack
from .report import inspect as inspect_mod
from .wiki import TITLES_LIMIT, Wiki

#: More titles than this leaving scope in one sync reads as a broken enumerator.
GONE_ALARM = 20
#: Changed titles read from the feed before giving up on reaching the watermark.
FEED_LIMIT = 5000


# --------------------------------------------------------------------------- #
# Scope
# --------------------------------------------------------------------------- #


@dataclass
class Scope:
    seeds: dict[str, list[str]] = field(default_factory=dict)
    tables: dict[str, list[dict]] = field(default_factory=dict)
    redirects: dict[str, str] = field(default_factory=dict)
    disambigs: dict[str, list[str]] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)

    def fetch_titles(self, pack: Pack) -> set[str]:
        return {t for key in pack.fetch_seeds for t in self.seeds.get(key, ())}


def scope(wiki: Wiki, pack: Pack, *, progress=None) -> Scope:
    """Run every enumerator and resolve alias sets."""
    sc = Scope()
    ctx = HarvestContext()
    say = progress or (lambda _m: None)

    # Structured tables first: enumerators may derive their membership from them, and
    # pulling each once keeps requests proportional to tables rather than seed sets.
    for table, fields_spec in pack.tables.items():
        say(f"table {table}")
        ctx.tables[table] = sc.tables[table] = wiki.cargo(table, fields_spec)

    for seed in pack.seeds:
        if seed.discovered:
            continue  # membership comes from followups; see SeedSet.discovered
        say(f"seed {seed.key} — {seed.source}")
        sc.seeds[seed.key] = sorted({t for t in seed.titles(wiki, ctx) if t})

    for key in pack.alias_seeds("redirect"):
        titles = sc.seeds.get(key, [])
        if not titles:
            continue
        sc.redirects.update(wiki.redirect_targets(titles))
        resolved = sum(1 for t in titles if t in sc.redirects)
        if resolved < 0.9 * len(titles):
            sc.findings.append(Finding(
                "G1", HIGH,
                f"{key}: only {resolved}/{len(titles)} redirects resolved to a target — "
                "the alias dictionary is the wrong shape if most entries point nowhere"))

    for key in pack.alias_seeds("disambig"):
        if sc.seeds.get(key):
            sc.disambigs.update(wiki.links(sc.seeds[key]))

    # Seed-set drift is checked at build time from the stored sets (`normalize`), so an
    # accepted baseline takes effect on the next build without another sync.
    sc.findings += _overlap_findings(sc, pack)
    return sc


def _overlap_findings(sc: Scope, pack: Pack) -> list[Finding]:
    """A title in two corpus seed sets is parsed twice, by two readers.

    Not automatically wrong, but it must be a decision rather than an accident, so it is
    reported whenever both sets produce corpus records.
    """
    corpus = [k for k in pack.corpus_seeds if sc.seeds.get(k)]
    out: list[Finding] = []
    for i, a in enumerate(corpus):
        for b in corpus[i + 1:]:
            shared = set(sc.seeds[a]) & set(sc.seeds[b])
            if shared:
                out.append(Finding(
                    "G1", LOW,
                    f"{a} ∩ {b}: {len(shared)} shared titles, each parsed by both routes "
                    f"({', '.join(sorted(shared)[:3])}…)"))
    return out


def _write_scope(archive: Archive, sc: Scope, pack: Pack) -> None:
    archive.write_seeds(sc.seeds, preserve=pack.discovered_seeds)
    archive.write_source_rows(sc.tables)
    archive.write_aliases_source(sc.redirects, sc.disambigs)
    archive.write_findings([f.row() for f in sc.findings], stage="scope")


# --------------------------------------------------------------------------- #
# Plan
# --------------------------------------------------------------------------- #


@dataclass
class Plan:
    #: Every page in scope is fetched: the first sync, or `--full`.
    full: bool
    #: No sync has completed yet (no watermark). Interrupted first syncs resume from here:
    #: pages already held are not fetched again.
    first: bool
    watermark: int = 0
    new_watermark: int = 0
    #: Held pages to re-fetch: edited since the watermark, or every one on `--full`.
    changed: list[str] = field(default_factory=list)
    #: Changed titles outside every seed set, ignored without being identified.
    ignored: int = 0
    #: Newly in scope.
    added: list[str] = field(default_factory=list)
    #: Left scope; dropped (after a snapshot) when the plan is applied.
    gone: list[str] = field(default_factory=list)
    #: In scope but no body held: a previous fetch failed or was interrupted. Repaired
    #: here because nothing else will — the change feed never mentions a page that did
    #: not change.
    missing: list[str] = field(default_factory=list)
    #: Change-feed and scope-change findings. Enumeration's own are on `scope`.
    findings: list[Finding] = field(default_factory=list)
    scope: Scope | None = None

    @property
    def fetch(self) -> list[str]:
        return sorted({*self.changed, *self.added, *self.missing})

    @property
    def nothing_to_do(self) -> bool:
        return not (self.changed or self.added or self.gone or self.missing)


def plan(wiki: Wiki, archive: Archive, pack: Pack, *, full: bool = False,
         rescope: bool = True, progress=None) -> Plan:
    """Work out what a sync would do without touching the archive.

    Separate from `apply` so `--dry-run` can show it: an update that silently removes
    hundreds of pages because an enumerator broke is exactly the event to look at first.
    """
    say = progress or (lambda _m: None)
    watermark = int(archive.get_meta("watermark", 0) or 0)
    p = Plan(full=full or not watermark, first=not watermark, watermark=watermark)
    held = archive.have_pages()
    before = set(archive.titles_in(pack.fetch_seeds))

    if p.full:
        # Pinned before anything is fetched, so an edit made during a long fetch is
        # picked up by the next increment rather than falling between the two.
        p.new_watermark = wiki.latest_change_id(pack.wiki.watch_namespaces)
    else:
        say(f"change feed from rcid={watermark:,}")
        touched: set[str] = set()
        reached = False
        for rc in wiki.recentchanges(pack.wiki.watch_namespaces):
            rcid = int(rc.get("rcid", 0))
            p.new_watermark = max(p.new_watermark, rcid)
            if rcid <= watermark:
                reached = True
                break
            touched.add(rc["title"])
            if len(touched) > FEED_LIMIT:
                break
        if not reached:
            p.findings.append(Finding(
                "G1", HIGH,
                f"never reached watermark rcid={watermark} within {FEED_LIMIT} changed "
                "pages — the increment is incomplete; run `sync --full`"))
        p.new_watermark = max(p.new_watermark, watermark)
        p.changed = sorted(t for t in touched & before if t in held)
        p.ignored = len(touched - before)
        say(f"changed titles: {len(touched):,}, in scope: {len(p.changed):,}")

    after = set(before)
    if p.full or rescope:
        say("enumerating seed sets (paged index queries; the slow part)")
        p.scope = scope(wiki, pack, progress=progress)
        fresh = p.scope.fetch_titles(pack)
        # `gone` only over the seeds enumeration covers: discovered sets are never
        # enumerated, so subtracting them would drop every discovered page each sync.
        enumerated = [k for k in pack.fetch_seeds if k not in pack.discovered_seeds]
        p.gone = sorted(set(archive.titles_in(enumerated)) - fresh)
        after = (before - set(p.gone)) | fresh
        if p.gone:
            p.findings.append(Finding(
                "G1", LOW if len(p.gone) < GONE_ALARM else HIGH,
                f"{len(p.gone)} titles left the seed sets — deletion or a broken enumerator"))

    p.added = sorted(t for t in after - before if t not in held)
    p.missing = sorted(t for t in after & before if t not in held)
    if p.full and not p.first:
        p.changed = sorted(t for t in after if t in held)
    return p


# --------------------------------------------------------------------------- #
# Apply
# --------------------------------------------------------------------------- #


@dataclass
class Result:
    fetched: int = 0
    #: Titles the site returned no content for: a scope problem, not a transport one.
    missing: list[str] = field(default_factory=list)
    followups: dict[str, int] = field(default_factory=dict)
    #: What the sample shows: held pages whose revision changed, pages added, pages
    #: removed. Empty on a first sync, where every page is new.
    delta: dict[str, list[str]] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)


def apply(wiki: Wiki, archive: Archive, pack: Pack, p: Plan, *, progress=None) -> Result:
    """Carry out a plan: fetch, run followups, drop what left scope, record the sync."""
    res = Result()
    revid_before = _held_revids(archive)
    archive.clear_removed()
    if p.scope is not None:
        _write_scope(archive, p.scope, pack)

    _fetch(wiki, archive, pack, p.fetch, res, progress)

    # A page whose body is transcluded from a subpage is discovered by reading its parent,
    # so a changed parent can reveal a subpage no enumeration knows about. Reads held
    # pages only, and fetches just what is not held yet.
    for hook in pack.followups:
        discovered: set[str] = set()
        for row in archive.pages(archive.seed(hook.of_seed)):
            discovered.update(hook.discover(row["title"], row["wikitext"]))
        held = archive.have_pages()
        pending = sorted(t for t in discovered if t not in held)
        if pending:
            if progress is not None:
                progress(f"followup {hook.label or hook.seed}: {len(pending)} pages")
            _fetch(wiki, archive, pack, pending, res, progress)
            res.followups[hook.seed] = len(pending)
        archive.add_seeds(hook.seed, discovered)

    for title in p.gone:
        if archive.page(title) is not None:
            archive.drop_page(title, inspect_mod.collect(archive, title, pack))

    res.findings = list(p.findings)
    archive.write_findings([f.row() for f in res.findings], stage="sync")

    revid_after = _held_revids(archive)
    res.delta = {"changed": [], "added": [], "removed": []}
    if not p.first:
        res.delta = {
            "changed": sorted(t for t, r in revid_after.items()
                              if t in revid_before and r != revid_before[t]),
            "added": sorted(t for t in revid_after if t not in revid_before),
            "removed": sorted(t for t in revid_before if t not in revid_after),
        }

    # The counts this sync started from, so the sample can say what it changed.
    previous = archive.get_meta("record_counts")
    if previous is not None:
        archive.set_meta("record_counts_previous", previous)
    archive.set_meta("watermark", p.new_watermark)
    archive.set_meta("synced_at", dt.datetime.now(dt.UTC).isoformat(timespec="seconds"))
    archive.set_meta("last_update", {"kind": "full" if p.full else "incremental",
                                     "first": p.first, **res.delta})
    archive.commit()
    return res


def _held_revids(archive: Archive) -> dict[str, int]:
    return {r["title"]: r["revid"] for r in archive.db.execute(
        "SELECT title, revid FROM raw.pages WHERE wikitext IS NOT NULL")}


def _fetch(wiki: Wiki, archive: Archive, pack: Pack, titles: Sequence[str], res: Result,
           progress) -> None:
    now = dt.datetime.now(dt.UTC).isoformat(timespec="seconds")
    for i, (title, text, revid) in enumerate(wiki.content(titles), 1):
        if text is None:
            res.missing.append(title)
            held = archive.page(title)
            if held is not None and held["wikitext"]:
                # The site no longer has a page we hold: keep what it was, for the sample.
                archive.drop_page(title, inspect_mod.collect(archive, title, pack))
                continue
        else:
            res.fetched += 1
        archive.put_page(title, text, revid, now)
        if i % 200 == 0:
            archive.commit()
            if progress is not None:
                progress(f"fetched {i:,}/{len(titles):,}")
    archive.commit()


# --------------------------------------------------------------------------- #
# Attribution: who made each held revision
# --------------------------------------------------------------------------- #


def refresh_editors(wiki: Wiki, archive: Archive, *, progress=None) -> int:
    """Look up the author of every held revision the cache does not know yet.

    Asked by revision id: the author of the revision the corpus holds is the one to
    credit, whatever the page's current revision is. Only revisions new since the last
    lookup cost a request, batched at the API's limit. Returns how many were looked up.
    """
    known = {r[0] for r in archive.db.execute("SELECT revid FROM editors")}
    todo = {r["revid"]: r["title"] for r in archive.db.execute(
        "SELECT title, revid FROM raw.pages WHERE revid IS NOT NULL AND wikitext IS NOT NULL")
        if r["revid"] not in known}
    revids = sorted(todo)
    found: list[tuple[str, int, str]] = []
    for i in range(0, len(revids), TITLES_LIMIT):
        batch = revids[i: i + TITLES_LIMIT]
        data = wiki.get(action="query", prop="revisions", rvprop="user|ids",
                        revids="|".join(str(r) for r in batch))
        for page in data.get("query", {}).get("pages", []):
            for rev in page.get("revisions") or []:
                if rev.get("user"):  # hidden authors are still credited by the history link
                    found.append((todo.get(rev["revid"], page.get("title", "")),
                                  rev["revid"], rev["user"]))
        if progress is not None:
            progress(f"editors {min(i + TITLES_LIMIT, len(revids)):,}/{len(revids):,}")
    archive.put_editors(found)
    archive.commit()
    return len(revids)
