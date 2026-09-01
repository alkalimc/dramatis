"""Scope: enumerate, do not classify.

The distinction is the single most consequential decision in the harvest stage.
An earlier design inferred a *page type* for every page from its template
signature and then applied a rule table per type. That approach has an
irreducible middle state — "reviewed / unreviewed / uncertain" — because
inference is never complete, and the uncertainty is unbounded: you cannot answer
"did we miss anything" without re-examining everything.

Enumeration replaces inference with membership. A seed set is a query whose answer
the *site itself* defines: rows of a structured table, members of a category, a
namespace, a hand-listed set of index pages. The result is closed, so "uncovered"
is not a state that exists, and a count drift is a signal rather than noise.

The cost is one hand-written enumerator per set. That cost is paid once and is
visible; the classification approach's cost is paid forever and is invisible.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from ..normalize.guards import Finding, check_drift
from ..pack import HarvestContext, Pack
from ..wiki import Wiki


@dataclass
class Scope:
    seeds: dict[str, list[str]] = field(default_factory=dict)
    tables: dict[str, list[dict]] = field(default_factory=dict)
    redirects: dict[str, str] = field(default_factory=dict)
    disambigs: dict[str, list[str]] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)

    def fetch_titles(self, pack: Pack) -> list[str]:
        out: set[str] = set()
        for key in pack.fetch_seeds:
            out.update(self.seeds.get(key, ()))
        return sorted(out)

    @property
    def counts(self) -> dict[str, int]:
        return {k: len(v) for k, v in self.seeds.items()}


def build(wiki: Wiki, pack: Pack, *, progress=None) -> Scope:
    """Run every enumerator, resolve alias sets, check drift."""
    sc = Scope()
    ctx = HarvestContext()

    # Structured tables first: enumerators are allowed to derive their membership
    # from them, and pulling each once keeps the request count proportional to the
    # number of tables rather than the number of seed sets.
    for table, fields_spec in pack.tables.items():
        if progress is not None:
            progress(f"table {table}")
        rows = wiki.cargo(table, fields_spec)
        ctx.tables[table] = rows
        sc.tables[table] = rows

    for seed in pack.seeds:
        if seed.discovered:
            continue  # membership comes from `fetch`; see SeedSet.discovered
        if progress is not None:
            progress(f"seed {seed.key} — {seed.source}")
        titles = sorted({t for t in seed.titles(wiki, ctx) if t})
        sc.seeds[seed.key] = titles

    for key in pack.alias_seeds("redirect"):
        titles = sc.seeds.get(key, [])
        if not titles:
            continue
        sc.redirects.update(wiki.redirect_targets(titles))
        resolved = sum(1 for t in titles if t in sc.redirects)
        if resolved < 0.9 * len(titles):
            sc.findings.append(Finding(
                "G1", "高",
                f"{key}: only {resolved}/{len(titles)} redirects resolved to a target — "
                "the alias dictionary is the wrong shape if most entries point nowhere",
            ))

    for key in pack.alias_seeds("disambig"):
        titles = sc.seeds.get(key, [])
        if titles:
            sc.disambigs.update(wiki.links(titles))

    # Discovered sets are deliberately absent here — `fetch` populates them — so
    # comparing them against a baseline now would report a shortfall that is simply the
    # order of the stages. They are checked after fetch instead.
    checked = {k: v for k, v in sc.counts.items() if k not in pack.discovered_seeds}
    baselines = {k: v for k, v in pack.baselines.items() if k not in pack.discovered_seeds}
    sc.findings += check_drift(checked, baselines, labels=pack.seed_labels)
    sc.findings += _overlap_findings(sc, pack)
    return sc


def _overlap_findings(sc: Scope, pack: Pack) -> list[Finding]:
    """A title in two corpus seed sets would be parsed twice, by two readers.

    Not automatically wrong — an operator page is legitimately both roster material
    and dossier material — but it must be a decision rather than an accident, so it
    is reported whenever both sets produce corpus records.
    """
    corpus = [k for k in pack.corpus_seeds if sc.seeds.get(k)]
    out: list[Finding] = []
    for i, a in enumerate(corpus):
        for b in corpus[i + 1:]:
            shared = set(sc.seeds[a]) & set(sc.seeds[b])
            if shared:
                sample = ", ".join(sorted(shared)[:3])
                out.append(Finding(
                    "G1", "低",
                    f"{a} ∩ {b}: {len(shared)} shared titles, each parsed by both "
                    f"routes ({sample}…)",
                ))
    return out


def write(archive, sc: Scope, pack: Pack) -> None:
    """Persist a scope. Alias sources go to their own tables, not the manifest."""
    archive.write_seeds(sc.seeds, preserve=pack.discovered_seeds)
    archive.write_source_rows(sc.tables)
    archive.write_aliases_source(sc.redirects, sc.disambigs)
    archive.write_findings([f.row() for f in sc.findings], stage="scope")
    archive.commit()
