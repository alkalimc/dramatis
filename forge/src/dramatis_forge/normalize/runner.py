"""Normalisation: fetched pages in, records out, guards on the way.

Offline and deterministic. Changing a rule means re-running this, which is minutes,
which is why the rules are safe to keep changing.

The orchestration is small on purpose. Everything that decides *what a page means*
is in a pack parser; everything here decides *what happens to a parser's output* —
provenance, deduplication, reconciliation, identity, guard bookkeeping. When those
two got mixed in the previous version, the effect was that the alias dictionary and
the identity model had nowhere to live and ended up inside a story parser.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .. import identity as identity_mod
from ..pack import Pack, PageContext
from ..store.archive import Archive
from .guards import Ledger, Reconciliation
from .records import ORDER, PARSER_VERSION, Alias, Record
from .wikitext import Cleaner


@dataclass
class Report:
    counts: dict[str, int] = field(default_factory=dict)
    chars: dict[str, int] = field(default_factory=dict)
    reconciliation: Reconciliation = field(default_factory=Reconciliation)
    ledger: Ledger = field(default_factory=Ledger)
    persons: int = 0
    pages_seen: int = 0
    pages_empty: int = 0

    def rows(self) -> list[tuple[str, int, int]]:
        return [(k, self.counts.get(k, 0), self.chars.get(k, 0)) for k in ORDER]

    @property
    def total_chars(self) -> int:
        return sum(self.chars.values())

    @property
    def clean(self) -> bool:
        return self.ledger.clean


def run(archive: Archive, pack: Pack, *, progress=None) -> Report:
    rep = Report()
    cleaner = Cleaner(pack.inline)
    tables = archive.all_source_rows()
    archive.reset_records()

    buckets: dict[str, list[Record]] = {}
    unrouted: dict[str, int] = {}

    for seed_key in pack.fetch_seeds:
        titles = archive.seed(seed_key)
        if progress is not None:
            progress(f"normalize {seed_key}: {len(titles):,} pages")
        for i, title in enumerate(titles):
            row = archive.page(title)
            if row is None or not row["wikitext"]:
                rep.ledger.add("G3", f"no body held for an in-scope page", page=title, high=True)
                continue
            route = pack.route_for(seed_key, title)
            if route is None:
                unrouted[seed_key] = unrouted.get(seed_key, 0) + 1
                continue

            ctx = PageContext(
                title=title, wikitext=row["wikitext"], revid=row["revid"],
                seed=seed_key, clean=cleaner, tables=tables,
            )
            produced = list(route.parser(ctx))
            rep.pages_seen += 1
            for severity, detail in ctx.drain_warnings():
                rep.ledger.add("G2", detail, page=title, high=severity == "高")
            if not produced:
                rep.pages_empty += 1
                if ctx.expected_empty:
                    rep.ledger.add("G3", ctx.expected_empty, page=title)
                else:
                    rep.ledger.add(
                        "G3", f"produced no records and gave no reason "
                              f"(route {route.label or route.seed})",
                        page=title, high=True)
            for rec in produced:
                buckets.setdefault(rec.KIND, []).append(rec)
            if progress is not None and i and i % 500 == 0:
                progress(f"  {seed_key} {i:,}/{len(titles):,}")

    for seed_key, n in sorted(unrouted.items()):
        rep.ledger.add("G3", f"{seed_key}: {n} pages fetched with no route to parse them")

    # ---- identity, before aliases: alias targets resolve through it ----
    roster_titles = frozenset(archive.titles_in(pack.roster_seeds))
    roster = identity_mod.Roster()
    if roster_titles:
        pages = {t: (archive.page(t)["wikitext"] or "")
                 for t in roster_titles if archive.page(t) is not None}
        roster = identity_mod.resolve(pack.identity, pages, roster_titles)
        rep.ledger.extend(roster.findings)
        rep.persons = len(roster)
        archive.write_identity(roster.storage_rows())
        buckets.setdefault("alias", []).extend(roster.aliases())

    # ---- aliases from the site's own naming relations ----
    buckets.setdefault("alias", []).extend(_site_aliases(archive, pack, roster, rep))

    # ---- store, reconciling produced against stored ----
    for kind in ORDER:
        records = buckets.get(kind, [])
        if not records:
            continue
        rep.counts[kind] = len(records)
        chars = sum(r.chars for r in records)
        if chars:
            rep.chars[kind] = chars
        stored, ignored = archive.insert_records(records)
        rep.reconciliation.note(kind, produced=len(records), stored=stored, ignored=ignored)
    rep.ledger.extend(rep.reconciliation.check())

    archive.set_meta("record_counts", rep.counts)
    archive.set_meta("record_chars", rep.chars)
    archive.set_meta("reconciliation", rep.reconciliation.as_dict())
    archive.set_meta("parser_version", PARSER_VERSION)
    archive.set_meta("pack_version", pack.version)
    archive.set_meta("alias_page_kinds", dict(pack.alias_pages))
    archive.set_meta("guard_tally", {g: list(v) for g, v in rep.ledger.tally().items()})
    archive.write_findings(rep.ledger.rows())
    archive.commit()
    return rep


def _site_aliases(
    archive: Archive, pack: Pack, roster: identity_mod.Roster, rep: Report
) -> list[Alias]:
    """Turn redirects and disambiguation pages into an alias dictionary.

    Two filters, both of which make the dictionary smaller and better:

    **The target must be in the archive.** An alias resolving to a page we
    deliberately did not archive normalises a user's query onto something
    unretrievable, which is worse than not recognising the alias at all. Measured
    on the Arknights pack: most redirects point at levels and items, all of them
    out of scope by decision.

    **The target is resolved through identity.** A redirect aimed at an alternate
    form should land on the person, not on one of her pages. Without this the alias
    dictionary silently disagrees with the roster.
    """
    in_scope = set(archive.titles_in(pack.corpus_seeds))
    to_person = roster.of_page

    out: list[Alias] = []
    redirects = archive.redirects()
    dropped = 0
    for alias, target in sorted(redirects.items()):
        resolved = to_person.get(target, target)
        if resolved in in_scope or resolved in to_person:
            out.append(Alias(alias=alias, target=resolved, kind="redirect"))
        else:
            dropped += 1
    if dropped:
        rep.ledger.add(
            "G3",
            f"{dropped} of {len(redirects)} redirects dropped: target not archived "
            "(levels, items and other out-of-scope pages)",
        )

    for word, candidates in sorted(archive.disambigs().items()):
        for cand in candidates:
            resolved = to_person.get(cand, cand)
            if resolved in in_scope or resolved in to_person:
                out.append(Alias(alias=word, target=resolved, kind="disambig"))

    unresolved = [t for t in archive.seed(next(iter(pack.alias_seeds("redirect")), ""))
                  if t not in redirects] if pack.alias_seeds("redirect") else []
    if unresolved:
        rep.ledger.add(
            "G3",
            f"{len(unresolved)} redirects had no resolvable target "
            "(missing page or cross-namespace)",
        )
    return out
