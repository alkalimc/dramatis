"""Normalisation: fetched pages in, records out, guards on the way.

Offline and deterministic. Changing a rule means re-running this, which is minutes,
which is why the rules are safe to keep changing.

The orchestration is small on purpose. Everything that decides *what a page means*
is in a pack parser; everything here decides *what happens to a parser's output* —
provenance, deduplication, reconciliation, identity, guard bookkeeping. Mixing the
two leaves the alias dictionary and the identity model with nowhere to live, and
they end up inside whichever parser happened to need them first.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from dataclasses import dataclass, field, replace

from . import baseline as baseline_mod
from . import guards
from . import identity as identity_mod
from .archive import Archive
from .guards import HIGH, Ledger, Reconciliation
from .pack import IdentityEvidence, Pack, PageContext
from .records import ORDER, PARSER_VERSION, SEQUENCED, Alias, Record
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
                rep.ledger.add("G3", "no body held for an in-scope page", page=title, high=True)
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
                rep.ledger.add("G2", detail, page=title, high=severity == HIGH)
            if not produced:
                rep.pages_empty += 1
                if ctx.expected_empty:
                    rep.ledger.add("G3", ctx.expected_empty, page=title)
                else:
                    rep.ledger.add(
                        "G3", f"produced no records and gave no reason "
                              f"(route {route.label or route.seed})",
                        page=title, high=True)
            else:
                _check_yield(ctx, route, produced, cleaner, rep)
            # Source order, recorded rather than implied: storage and readers order by
            # `seq`, so nothing downstream can re-sort a page's records by accident.
            position: dict[str, int] = {}
            for rec in produced:
                if rec.KIND in SEQUENCED:
                    rec = replace(rec, seq=position.get(rec.KIND, 0))
                    position[rec.KIND] = rec.seq + 1
                buckets.setdefault(rec.KIND, []).append(rec)
            if progress is not None and i and i % 500 == 0:
                progress(f"  {seed_key} {i:,}/{len(titles):,}")

    for seed_key, n in sorted(unrouted.items()):
        rep.ledger.add("G3", f"{seed_key}: {n} pages fetched with no route to parse them")

    # ---- seed-set drift, from the stored sets against the accepted baseline ----
    # Here rather than in `sync`, so the finding always reflects the current baseline:
    # `forge baseline accept` followed by `forge build` clears a reviewed shrink.
    accepted = baseline_mod.load(archive.path)
    rep.ledger.extend(guards.check_drift(
        archive.seed_counts(), accepted.seeds if accepted else None,
        labels=pack.seed_labels))

    # ---- identity, before aliases: alias targets resolve through it ----
    roster_titles = frozenset(archive.titles_in(pack.roster_seeds))
    roster = identity_mod.Roster()
    if roster_titles:
        pages = {t: (archive.page(t)["wikitext"] or "")
                 for t in roster_titles if archive.page(t) is not None}
        spoken: dict[str, int] = {}
        for rec in buckets.get("line", ()):
            if rec.speaker:
                spoken[rec.speaker] = spoken.get(rec.speaker, 0) + 1
        evidence = IdentityEvidence(spoken=spoken, redirects=archive.redirects())
        roster = identity_mod.resolve(pack.identity, pages, roster_titles, evidence)
        rep.ledger.extend(roster.findings)
        rep.ledger.extend(guards.check_drift(
            roster.counts(), accepted.identity if accepted else None, guard="G4"))
        rep.persons = len(roster)
        archive.write_identity(roster.storage_rows())
        buckets.setdefault("alias", []).extend(roster.aliases())

    # ---- aliases from the site's own naming relations ----
    buckets.setdefault("alias", []).extend(_site_aliases(archive, pack, roster, rep))
    buckets["alias"].extend(term_aliases(
        buckets.get("term", ()), roster.of_page, archive.redirects()))
    # Whatever relation produced an alias, its target is the canonical name.
    redirects = archive.redirects()
    buckets["alias"] = [
        replace(a, target=_canonical(a.target, roster.of_page, redirects))
        for a in buckets["alias"]]
    buckets["alias"] = [a for a in buckets["alias"] if a.alias != a.target]
    check_aliases(buckets["alias"], redirects, set(roster.of_page.values()), rep)

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

    # `record_counts_previous` is set by `harvest.apply`, so the sample's delta is what the
    # last sync changed, however many offline builds ran since.
    archive.set_meta("record_counts", rep.counts)
    archive.set_meta("record_chars", rep.chars)
    archive.set_meta("reconciliation", rep.reconciliation.as_dict())
    archive.set_meta("parser_version", PARSER_VERSION)
    archive.set_meta("pack_version", pack.version)
    archive.write_findings(rep.ledger.rows(), stage="normalize")
    archive.commit()
    return rep


#: How a G3 yield finding's detail starts. The page produced records, so readers that want
#: only the pages that produced nothing (the sample's "empty" boundary) skip these.
YIELD_DETAIL = "yield "

#: Pages with fewer visible letters than this are too small for a yield ratio to mean
#: anything.
MIN_VISIBLE = 200


def _check_yield(ctx: PageContext, route, produced: list[Record], cleaner: Cleaner,
                 rep: Report) -> None:
    """G3, the quiet half: a page that produced *something*, but far less than it shows.

    The loud half — a page that produced nothing — is caught above. This one is how a
    rule that reads the first of several layouts, or a length floor set too high, loses
    most of a page while every count still looks healthy.
    """
    if route.min_yield <= 0:
        return
    shown = cleaner.visible(ctx.wikitext)
    if shown < MIN_VISIBLE:
        return
    kept = cleaner.letters("".join(r.prose for r in produced))
    if kept < shown * route.min_yield:
        rep.ledger.add(
            "G3", f"{YIELD_DETAIL}{kept / shown:.0%} of visible text ({kept:,} of {shown:,} "
                  f"letters) is under the route's floor of {route.min_yield:.0%}",
            page=ctx.title)


#: Alias kind for a rendering of a glossary term in another script or spelling.
TERM_ALIAS = "term"
#: Where one cell of a glossary row holds several renderings.
_RE_RENDERINGS = re.compile(r"[/\n]")


def term_aliases(
    terms, to_person: dict[str, str], redirects: Mapping[str, str] | None = None
) -> list[Alias]:
    """Every rendering of a glossary term is a name for it.

    A glossary says which foreign spelling means which native term; as an alias table it
    lets a query in one spelling reach text written in another. The terms themselves are
    not units: a row of translations is a lookup, not something anyone says.

    The target is the term's canonical name: through the roster, then through redirects, so
    an alias never points at another alias (the page the term redirects to is what the
    rendering names). A rendering that comes out as its own target says nothing and is dropped.
    """
    redirects = redirects or {}
    out: dict[tuple[str, str], Alias] = {}
    for term in terms:
        target = _canonical(term.term, to_person, redirects)
        for value in term.translations.values():
            for form in _RE_RENDERINGS.split(value):
                form = form.strip()
                if form and form != term.term and form != target:
                    out.setdefault((form, target), Alias(alias=form, target=target, kind=TERM_ALIAS))
    return list(out.values())


def _canonical(name: str, to_person: Mapping[str, str], redirects: Mapping[str, str]) -> str:
    """A name's canonical target: roster person if it is one, else the end of its redirects."""
    seen = {name}
    while name not in to_person and name in redirects and redirects[name] not in seen:
        name = redirects[name]
        seen.add(name)
    return to_person.get(name, name)


#: A rendering with more characters of the corpus language than this, or with a sentence
#: stop, is a gloss and not a name. (A long name in another script is still a name.)
ALIAS_MAX_CHARS = 25
_RE_CJK = re.compile(r"[\u4e00-\u9fff]")


def check_aliases(
    aliases: list[Alias], redirects: Mapping[str, str], persons: set[str], rep: Report
) -> None:
    """G4: the alias table must not disagree with itself.

    * a target that is itself a redirect is a hop too many (`persons` are exempt: a person
      is the end of the chain by definition);
    * a term rendering that is a sentence is a note taken for a spelling;
    * a term rendering that another relation (redirect, real name, disambiguation) sends
      elsewhere is a real conflict, left for a person to read.
    """
    hops = [a for a in aliases if a.target in redirects and a.target not in persons]
    if hops:
        rep.ledger.add(
            "G4", f"{len(hops)} alias(es) point at another alias, not at its canonical name "
                  "(first: " + ", ".join(f"{a.alias}->{a.target}" for a in hops[:5]) + ")")
    wordy = [a for a in aliases if a.kind == TERM_ALIAS
             and (len(_RE_CJK.findall(a.alias)) > ALIAS_MAX_CHARS or "\u3002" in a.alias)]
    if wordy:
        rep.ledger.add(
            "G4", f"{len(wordy)} glossary rendering(s) read as a sentence, not a name "
                  "(first: " + ", ".join(a.alias[:20] for a in wordy[:5]) + ")")
    other: dict[str, set[str]] = {}
    for a in aliases:
        if a.kind != TERM_ALIAS:
            other.setdefault(a.alias, set()).add(a.target)
    for a in aliases:
        if a.kind == TERM_ALIAS and a.alias in other and a.target not in other[a.alias]:
            rep.ledger.add(
                "G4", f"glossary rendering {a.alias!r} names {a.target!r}; the site's own "
                      f"relations send it to {', '.join(sorted(other[a.alias]))}")


def _site_aliases(
    archive: Archive, pack: Pack, roster: identity_mod.Roster, rep: Report
) -> list[Alias]:
    """Turn redirects and disambiguation pages into an alias dictionary.

    Two filters, both of which make the dictionary smaller and better:

    **The target must be in the archive.** An alias resolving to a page we
    deliberately did not archive normalises a user's query onto something
    unretrievable, which is worse than not recognising the alias at all. On a large
    wiki most redirects point at out-of-scope pages.

    **The target is resolved through identity.** A redirect aimed at one of a
    person's forms should land on the person, not on one of their pages. Without this the alias
    dictionary silently disagrees with the roster.
    """
    in_scope = set(archive.titles_in(pack.corpus_seeds)) - set(roster.excluded)
    to_person = roster.of_page

    out: list[Alias] = []
    redirects = archive.redirects()
    dropped = 0
    for alias, target in sorted(redirects.items()):
        resolved = _canonical(target, to_person, redirects)
        if resolved in in_scope or resolved in to_person:
            out.append(Alias(alias=alias, target=resolved, kind="redirect"))
        else:
            dropped += 1
    if dropped:
        rep.ledger.add(
            "G3",
            f"{dropped} of {len(redirects)} redirects dropped: target not archived "
            "(out-of-scope pages)",
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
