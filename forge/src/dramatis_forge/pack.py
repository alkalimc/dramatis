"""The engine/pack seam.

One rule decides where code goes: **the engine owns mechanism, the pack owns
rules.** If a line of code contains a wiki template name, a character name, a
game term, or a site URL, it belongs to a pack.

The seam is deliberately thin — declarative rule objects plus a handful of
parser callables, registered through one module-level `PACK`. There is no plugin
base-class hierarchy: with one pack in existence that would be fiction dressed as
architecture. What is load-bearing is that the *shape* of the contract is fixed,
so a second pack has something to conform to.

A pack that satisfies this contract gets, for free: rate-limited harvesting with
resumable fetch and an incremental watermark, the record vocabulary and its
storage, the three guards, the identity framework, chunking, FTS pre-tokenisation,
folio packing, the benchmark builders, and the probe runner.
"""

from __future__ import annotations

import importlib
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:  # pragma: no cover - typing only
    from .normalize.records import Record
    from .normalize.wikitext import Cleaner
    from .wiki import Wiki


# --------------------------------------------------------------------------- #
# Harvesting
# --------------------------------------------------------------------------- #


class SeedEnumerator(Protocol):
    """Enumerates the titles of one seed set.

    Receives the live wiki client and whatever the pack already put in `ctx`
    (typically Cargo query results fetched once and shared between seed sets).
    """

    def __call__(self, wiki: Wiki, ctx: HarvestContext) -> Iterable[str]: ...


@dataclass
class HarvestContext:
    """Scratch space shared by a pack's seed enumerators within one `scope` run.

    Exists so that seed sets can share an expensive query — enumerating operators
    and their voice pages from the same Cargo dump costs one round trip, not two.
    """

    tables: dict[str, list[dict]] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)

    def rows(self, table: str) -> list[dict]:
        return self.tables.get(table, [])


@dataclass(frozen=True)
class SeedSet:
    """A closed, reproducibly countable set of page titles.

    `baseline` is the count measured on a real run. Guard G1 compares against it
    on every later run: growth is normal and reported, shrinkage is suspicious
    and reported louder. A seed set with `baseline = 0` is exempt (new set, no
    measurement yet) — which is itself visible in the guard output.

    `fetch = False` marks a set whose *titles* are the payload; alias sets are the
    case. Enumerating 7k redirects costs one paged list query; fetching 7k page
    bodies that contain nothing but `#REDIRECT` costs an hour and yields nothing.
    """

    key: str
    label: str
    source: str
    #: How to enumerate. Omit when `fixed` is given: a hand-listed set does not need
    #: a function, and expressing it as one hides the fact that it is a constant —
    #: which in turn means it cannot be re-applied without a network round trip.
    enumerate: SeedEnumerator | None = None
    #: A literal membership list. Declarative, so `scope` can apply it offline.
    fixed: tuple[str, ...] | None = None
    baseline: int = 0
    fetch: bool = True
    #: Titles in this set are not corpus material — they populate the roster,
    #: the alias dictionary, or a benchmark. Keeps `report` honest about the
    #: difference between "archived" and "used".
    corpus: bool = True
    #: Marks a set whose titles are naming relations to be resolved during
    #: `scope` rather than parsed during `normalize`. `"redirect"` resolves
    #: alias → target; `"disambig"` resolves word → candidate entities. Declaring
    #: it here rather than special-casing a seed key by name keeps the harvest
    #: stage free of any knowledge of which set is which.
    alias_kind: str | None = None
    #: Feeds the person roster: identity resolution reads exactly these titles.
    roster: bool = False
    #: Membership is discovered during `fetch` (see `Followup`), not enumerable in
    #: advance. `scope` neither queries nor clears these sets, so re-scoping does not
    #: forget what fetching found.
    discovered: bool = False

    def __post_init__(self) -> None:
        if self.enumerate is None and self.fixed is None and not self.discovered:
            raise ValueError(
                f"seed set {self.key!r} has no enumerator, no fixed list and is not "
                "marked discovered — nothing could ever populate it"
            )

    def titles(self, wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
        if self.fixed is not None:
            return self.fixed
        if self.enumerate is None:
            return ()
        return self.enumerate(wiki, ctx)


# --------------------------------------------------------------------------- #
# Normalisation
# --------------------------------------------------------------------------- #


@dataclass
class PageContext:
    """Everything a page parser is allowed to see.

    Parsers get no store handle and no wiki client on purpose: normalisation must
    be a pure function of already-fetched text, so that changing a rule means
    re-running `normalize` (minutes, offline) rather than re-harvesting (hours,
    over someone else's bandwidth).
    """

    title: str
    wikitext: str
    revid: int | None
    seed: str
    clean: Cleaner
    #: Structured side-channel the harvest stage captured (e.g. Cargo rows),
    #: keyed by table name then page title.
    tables: Mapping[str, Mapping[str, list[dict]]]
    _warnings: list[tuple[str, str]] = field(default_factory=list)
    #: Set by `empty()`. A parser that yields nothing and says why is reporting a
    #: fact; one that yields nothing silently is a failure. Guard G3 reads exactly
    #: this distinction, which is why it cannot be inferred by the engine.
    expected_empty: str | None = None

    def empty(self, reason: str) -> None:
        self.expected_empty = reason

    def warn(self, detail: str, *, high: bool = False) -> None:
        """Record a guard-G2 observation: a construct the rules did not predict.

        Severity is a judgement about *whether content may have been lost*, not
        about how odd the construct looks. Low-severity warnings are counted and
        summarised; high-severity ones are printed and block the design freeze.
        """
        self._warnings.append(("高" if high else "低", detail))

    def rows(self, table: str, title: str | None = None) -> list[dict]:
        return list(self.tables.get(table, {}).get(title or self.title, []))

    def row(self, table: str, title: str | None = None) -> dict:
        rows = self.rows(table, title)
        return rows[0] if rows else {}

    def drain_warnings(self) -> list[tuple[str, str]]:
        out, self._warnings[:] = list(self._warnings), []
        return out


class PageParser(Protocol):
    """Turns one page into records. May yield nothing; that is guard G3's business."""

    def __call__(self, ctx: PageContext) -> Iterable[Record]: ...


@dataclass(frozen=True)
class Followup:
    """A page whose body lives somewhere else, discovered only after fetching it.

    Enumeration is closed over what the site *declares*, and this is the one real
    hole in that: a page can transclude its content from a subpage that belongs to
    no seed set. Seen in practice: story pages whose entire text
    is elsewhere. Without this hook they parse to zero records and land in guard G3
    as an unexplained failure — the guard catches it, but nothing fixes it.

    `discover` sees one fetched page and returns extra titles to fetch, which are
    registered under `seed` so later stages can find them.
    """

    of_seed: str
    seed: str
    discover: Callable[[str, str], Iterable[str]]
    label: str = ""


@dataclass(frozen=True)
class Route:
    """Dispatch rule: which parser handles which pages.

    Matching is by seed set first because seed membership is *defined* by the
    site, whereas title patterns are guessed. `title` / `title_prefix` narrow
    within a seed set for hand-picked index pages, where each page genuinely
    needs its own reader.
    """

    seed: str
    parser: PageParser
    title: str | None = None
    title_prefix: str | None = None
    label: str = ""

    def matches(self, seed: str, title: str) -> bool:
        if seed != self.seed:
            return False
        if self.title is not None:
            return title == self.title
        if self.title_prefix is not None:
            return title.startswith(self.title_prefix)
        return True

    @property
    def specificity(self) -> int:
        """Exact title beats prefix beats bare seed, so route order is not load-bearing."""
        if self.title is not None:
            return 2
        if self.title_prefix is not None:
            return 1
        return 0


# --------------------------------------------------------------------------- #
# Inline markup
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class ContentSpec:
    """A template that *carries body text* rather than decorating it.

    Falling back to "keep the last positional parameter" — the right default for
    decoration templates — silently destroys these. Seen in practice on a
    pack: 992 story synopses and 59 encyclopaedia entries, all of them among the
    highest-value material in the corpus.

    `prefix` parameters are joined and prepended: a synopsis is far more
    retrievable with its stage and timepoint attached than without.
    """

    positional: tuple[int, ...] = ()
    named: tuple[str, ...] = ()
    prefix: tuple[int, ...] = ()


@dataclass(frozen=True)
class InlineRules:
    """A closed conversion table for inline markup, plus the promise to complain.

    The table cannot be complete — a live wiki grows templates. What makes it
    safe is that anything outside it is *reported* (guard G2) rather than guessed
    at quietly. Every entry below was added in response to a warning, not in
    anticipation of one.
    """

    #: Remove entirely, contents and all: real-world sourcing notes, wiki
    #: meta-apparatus, media, navigation.
    drop: frozenset[str] = frozenset()
    #: Replace with one positional parameter's text. -1 = last, 0 = first.
    text_param: Mapping[str, int] = field(default_factory=dict)
    #: Replace with assembled body text (see ContentSpec).
    content: Mapping[str, ContentSpec] = field(default_factory=dict)
    #: Replace with a fixed string, e.g. the player-name macro.
    literal: Mapping[str, str] = field(default_factory=dict)
    #: Plain-text substitutions applied after the template pass: engine-level macros
    #: that are not templates at all, so no template rule can reach them.
    macros: tuple[tuple[Any, str], ...] = ()
    #: Section headings whose subtree is dropped wherever it appears.
    drop_sections: frozenset[str] = frozenset()
    #: Page-title prefixes that are wiki meta-space rather than world content.
    meta_pages: tuple[str, ...] = ()


# --------------------------------------------------------------------------- #
# Identity
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class IdentityRules:
    """How page identity maps to person identity.

    A page is not a person. Handling that is the pack's job because the *signal*
    is site-specific (which template declares the relation), while the *model*
    (Person owning ordered Forms) is the engine's.

    `resolve` must only use signals the site declares explicitly. Inferred
    identity — name similarity, edit distance, shared prefixes — is forbidden by
    contract, not by taste: an inferred join has no authoritative source to
    reconcile against, so a wrong one is undetectable forever.
    """

    #: (page title -> wikitext, roster titles) -> {page: (canonical, form_kind)}
    resolve: Callable[[Mapping[str, str], frozenset[str]], Mapping[str, tuple[str, str]]]
    #: Ordering hint for a person's forms; lower sorts first.
    form_order: Mapping[str, int] = field(default_factory=dict)
    #: Expected counts for the identity guards, measured on a real run.
    baselines: Mapping[str, int] = field(default_factory=dict)


# --------------------------------------------------------------------------- #
# Chunking
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class ChunkTemplate:
    """One retrieval-unit shape.

    Typed units are a research claim, not just tidiness: prose, dialogue scripts,
    attribute tables and index tables differ in semantic density by an order of
    magnitude, and pooling them into one vector space degrades queries on both
    sides. The type label rides along on every chunk so the index can calibrate
    scores per type and report metrics per bucket.
    """

    name: str
    #: Which record kinds feed this template.
    sources: tuple[str, ...]
    #: Instruction prefix for the encoder's task-conditioned embedding.
    task: str = ""
    #: Soft character budget. Chunks over it are split at a natural boundary.
    max_chars: int = 900
    #: Sequence templates only: target unit length in records, and the hard cap
    #: while seeking a natural boundary.
    target: int = 0
    max_span: int = 0
    #: Absorb a trailing stub shorter than `target // 2` into the previous unit,
    #: rather than emitting a fragment that embeds as noise.
    absorb_tail: bool = True


@dataclass(frozen=True)
class ChunkPolicy:
    templates: tuple[ChunkTemplate, ...]
    #: Header line prepended to a chunk's embedded text, per template. Retrieval
    #: quality depends on the unit carrying its own context; a bare dialogue
    #: window with no scene or speaker is close to unsearchable.
    headers: Mapping[str, str] = field(default_factory=dict)

    def by_name(self, name: str) -> ChunkTemplate:
        for t in self.templates:
            if t.name == name:
                return t
        raise KeyError(name)


# --------------------------------------------------------------------------- #
# The pack itself
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class CoverageRow:
    """One line of the coverage report: a section of the source, and what we did with it.

    The report exists to make the *excluded* half legible. A report that lists only what
    was taken cannot answer "did we miss something", which is the only question anyone
    actually asks of it — so every exclusion carries its reason, and the reasons are
    reviewable by someone who has never read the code.

    `measure` is how to count it: a seed key, or a SQL scalar. Left empty for sections we
    do not archive, which is how the report stays honest about the difference between
    "zero" and "not applicable".
    """

    section: str
    disposition: str  # archived | partial | excluded
    reason: str
    measure: str | None = None


@dataclass(frozen=True)
class FigureSpec:
    """One quantity a design document is allowed to quote, and its retired renderings.

    Packs own this because both halves are domain knowledge. *Which* quantities are
    load-bearing depends on what the corpus is, and the list of values a document once
    claimed is design history — "94,458 was the overlapping-window build" belongs in a
    decision record, not in a regex table in an open repository.

    The framework only knows how to read a key out of a manifest, run a named derived
    query, compare, and report. It is told nothing about what the numbers mean.
    """

    key: str
    #: Where the value comes from: `manifest:<field>`, `folio:<field>`,
    #: `derived:<name>`, or `computed:<name>` for the few that are arithmetic.
    source: str
    #: Renderings that are no longer true. Plain integers are turned into
    #: comma-tolerant patterns; anything else is used as a regex verbatim.
    retired: tuple[str | int, ...] = ()
    note: str = ""
    #: Optional sub-key for mapping-valued manifest fields, e.g. a template name.
    member: str | None = None


@dataclass(frozen=True)
class WikiConfig:
    api: str
    #: Sent in User-Agent. A contactable maintainer is the minimum courtesy owed
    #: to a volunteer-run site we are about to make thousands of requests to.
    contact: str
    #: Human-readable page URL, `{page}` substituted. Stored in the folio so a reader
    #: can offer "see the original" without the framework knowing any site.
    source_url: str = ""
    #: Page-history URL, `{page}` substituted. Attribution for collectively edited pages
    #: points here rather than at a single revision, because a page history is what
    #: actually credits every contributor.
    history_url: str = ""
    rate: float = 8.0
    #: Namespaces whose changes can matter for incremental update.
    watch_namespaces: tuple[int, ...] = (0,)


@dataclass(frozen=True)
class Pack:
    name: str
    version: int
    wiki: WikiConfig
    seeds: tuple[SeedSet, ...]
    routes: tuple[Route, ...]
    followups: tuple[Followup, ...]
    inline: InlineRules
    identity: IdentityRules
    chunking: ChunkPolicy
    #: Cargo/structured tables to pull once during `scope`: table -> field spec.
    tables: Mapping[str, str] = field(default_factory=dict)
    #: Tables where one page legitimately has several rows. Getting this wrong is
    #: a silent data-loss bug: keying by page drops the extra rows without a word.
    multi_row_tables: frozenset[str] = frozenset()
    #: Machine field name -> the wiki's own wording. Downstream consumers (prompt
    #: synthesis, retrieval) should see the site's vocabulary, not a schema's.
    field_names: Mapping[str, str] = field(default_factory=dict)
    #: In-world phrasing for runtime surfaces the client renders.
    wording: Mapping[str, str] = field(default_factory=dict)
    #: The source's own table of contents, annotated with what we took and why not.
    coverage: tuple[CoverageRow, ...] = ()
    #: page title -> alias kind, for pages whose entire output is dictionary entries
    #: rather than corpus records. Without this a working parser reports "produced
    #: nothing", because aliases carry no source page of their own.
    alias_pages: Mapping[str, str] = field(default_factory=dict)
    #: Quantities the design documents may quote, and the renderings each has retired.
    #: Both halves are domain knowledge; see `FigureSpec`.
    figures: tuple[FigureSpec, ...] = ()

    def seed(self, key: str) -> SeedSet:
        for s in self.seeds:
            if s.key == key:
                return s
        raise KeyError(key)

    @property
    def fetch_seeds(self) -> tuple[str, ...]:
        return tuple(s.key for s in self.seeds if s.fetch)

    @property
    def discovered_seeds(self) -> tuple[str, ...]:
        return tuple(s.key for s in self.seeds if s.discovered)

    @property
    def fixed_seeds(self) -> dict[str, tuple[str, ...]]:
        """Seed sets whose membership is a constant, so it needs no network."""
        return {s.key: s.fixed for s in self.seeds if s.fixed is not None}

    @property
    def corpus_seeds(self) -> tuple[str, ...]:
        return tuple(s.key for s in self.seeds if s.fetch and s.corpus)

    @property
    def roster_seeds(self) -> tuple[str, ...]:
        return tuple(s.key for s in self.seeds if s.roster)

    def alias_seeds(self, kind: str) -> tuple[str, ...]:
        return tuple(s.key for s in self.seeds if s.alias_kind == kind)

    @property
    def baselines(self) -> dict[str, int]:
        return {s.key: s.baseline for s in self.seeds}

    @property
    def seed_labels(self) -> dict[str, str]:
        return {s.key: s.source for s in self.seeds}

    def route_for(self, seed: str, title: str) -> Route | None:
        best: Route | None = None
        for r in self.routes:
            if r.matches(seed, title) and (best is None or r.specificity > best.specificity):
                best = r
        return best


# --------------------------------------------------------------------------- #
# Loading
# --------------------------------------------------------------------------- #

_SEARCH: Sequence[str] = ("packs.{name}", "dramatis_packs.{name}", "{name}")


def pack_dir(name: str) -> Path:
    """Directory holding `packs.<name>`, for assets that sit next to the rules.

    Kept off `Pack` deliberately. `Pack` is declarative rules; giving it a filesystem
    path would make it describe where it was loaded from as well as what it says, and
    the rules are meant to be readable without reference to a checkout layout.

    What lives here is anything the code *reads* but a person *writes*: the attribution
    and notice templates, parser fixtures. That is the dividing line — a template is
    prose, but prose a generator consumes is an input to code, so it ships with the code
    rather than with the design documents.
    """
    for pattern in _SEARCH:
        try:
            mod = importlib.import_module(pattern.format(name=name))
        except ModuleNotFoundError:
            continue
        spec = getattr(mod, "__file__", None)
        if spec:
            return Path(spec).parent
    raise ModuleNotFoundError(f"no pack named {name!r}")


def load_pack(name: str) -> Pack:
    """Import `packs.<name>` and return its module-level `PACK`.

    Import by convention rather than entry points: entry points require the pack
    to be an installed distribution, which forbids the thing packs are for —
    keeping one in a directory next to the engine and editing it.
    """
    tried: list[str] = []
    for pattern in _SEARCH:
        module = pattern.format(name=name)
        tried.append(module)
        try:
            mod = importlib.import_module(module)
        except ModuleNotFoundError as exc:
            if exc.name not in {module, module.split(".")[0]}:
                raise  # a real import error inside the pack; do not mask it
            continue
        pack = getattr(mod, "PACK", None)
        if not isinstance(pack, Pack):
            raise TypeError(f"{module} defines no module-level PACK: Pack")
        return pack
    raise ModuleNotFoundError(f"no pack named {name!r} (tried: {', '.join(tried)})")
