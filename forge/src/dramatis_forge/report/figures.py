"""Measure an artifact, report the figures, and check a document tree against them.

Documents cite measured values by key (`units`, `tokens.line`) rather than quoting them,
so a number cannot go stale in prose. This module is what makes that workable.

Three parts:

*Measuring* — every figure is computed here from the artifacts, from a manifest key or a
named derived measurement. Which figures exist is the pack's choice (`Pack.figures`); how
each derived one is computed is the framework's, because it is schema knowledge.

*Targets* — a figure may carry an acceptance condition (`FigureSpec.target`), evaluated
here and shown as pass or fail. A condition a document states in prose cannot be checked;
one declared beside the figure is checked on every build.

*Scanning* — a relative link to a file that does not exist, a cross-reference id nothing
defines, and a backticked key under the root of a computed figure that nothing computes
(and the pack does not declare planned) are all claims nobody checked. Placeholders are
understood: `a.{x,y}` names two keys, and `a.{name}` (one word, any script) stands for
any registered member. The scan understands no prose; it reports file, line and what it
found, and leaves the judgement to a reader.
"""

from __future__ import annotations

import json
import operator
import re
import sqlite3
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path

from ..config import Paths
from ..guards import GUARDS, HIGH
from ..pack import Pack, pack_dir
from ..text import table_head
from . import tokens as tokens_mod

#: Share of held-out lines used by `heldout_lines`.
HOLDOUT_FRACTION = 0.2
#: Default line threshold for `scorable_at` / `heldout_lines` / `name_leak`.
MIN_LINES = 25
#: Shortest name counted by `name_leak`; single characters match everywhere.
MIN_NAME_CHARS = 2


@dataclass(frozen=True)
class Figure:
    """A spec resolved against an artifact."""

    key: str
    value: object
    shown: str
    note: str = ""
    target: str = ""
    #: None when there is no target; otherwise whether the value meets it.
    passed: bool | None = None


# --------------------------------------------------------------------------- #
# Measuring
# --------------------------------------------------------------------------- #


def _connect(db: Path) -> sqlite3.Connection | None:
    if not db.exists():
        return None
    con = sqlite3.connect(f"file:{db}?immutable=1", uri=True)
    con.row_factory = sqlite3.Row
    return con


def _meta(con: sqlite3.Connection | None) -> dict[str, object]:
    if con is None:
        return {}
    out: dict[str, object] = {}
    for key, raw in con.execute("select key, value from manifest"):
        try:
            out[key] = json.loads(raw) if isinstance(raw, str) else raw
        except (json.JSONDecodeError, TypeError):
            out[key] = raw
    return out


@dataclass
class _Sources:
    """Open artifacts plus the few intermediate results several measurements share."""

    archive: sqlite3.Connection | None
    folio: sqlite3.Connection | None
    folio_path: Path
    meta: dict[str, dict[str, object]]
    pack: Pack
    suite: Path
    has_raw: bool = False
    _cache: dict[str, object] = field(default_factory=dict)

    def counter(self) -> Callable[[str], int] | None:
        """The pack's reference tokenizer, loaded once. None when unavailable."""
        def compute():
            return tokens_mod.load_counter(self.pack.tokenizer, pack_dir(self.pack.name))
        return self.once("counter", compute)  # type: ignore[return-value]

    def once(self, key: str, compute: Callable[[], object]) -> object:
        if key not in self._cache:
            self._cache[key] = compute()
        return self._cache[key]

    def shaped(self, shape: str) -> list[str]:
        """Template names built by one builder, from the folio's own declaration."""
        declared = self.meta["folio"].get("template_shapes") or {}
        if not isinstance(declared, Mapping):
            return []
        return sorted(name for name, s in declared.items() if s == shape)

    def speech_by_person(self) -> dict[str, list[str]]:
        """Spoken lines per person: speaker names resolved through identity."""
        def compute() -> dict[str, list[str]]:
            assert self.archive is not None
            person_of = {r[0]: r[1] for r in self.archive.execute(
                "select page, person_id from forms")}
            out: dict[str, list[str]] = {}
            for speaker, text in self.archive.execute(
                "select speaker, text from lines where speaker is not null and speaker <> ''"
            ):
                out.setdefault(person_of.get(speaker, speaker), []).append(text)
            return out
        return self.once("speech", compute)  # type: ignore[return-value]

    def roster(self) -> set[str]:
        assert self.archive is not None
        return self.once("roster", lambda: {r[0] for r in self.archive.execute(
            "select person_id from persons")})  # type: ignore[return-value]


def _threshold(member: str | None) -> int:
    return int(member) if member else MIN_LINES


def _boundaries(src: _Sources) -> tuple[int, int]:
    """(at a speaker change, mid-turn) over every dialogue unit's end boundary."""
    def compute() -> tuple[int, int]:
        assert src.archive is not None and src.folio is not None
        speakers: dict[str, dict[int, str | None]] = {}
        for scene, seq, speaker in src.archive.execute("select scene, seq, speaker from lines"):
            speakers.setdefault(scene, {})[seq] = speaker
        names = src.shaped("dialogue")
        clean = split = 0
        if names:
            marks = ",".join("?" * len(names))
            for span_of, span_to in src.folio.execute(
                f"select span_of, span_to from chunks where template in ({marks})", names
            ):
                by_seq = speakers.get(span_of, {})
                last, nxt = by_seq.get(span_to), by_seq.get(span_to + 1)
                if nxt is None or last != nxt:
                    clean += 1
                else:
                    split += 1
        return clean, split
    return src.once("boundaries", compute)  # type: ignore[return-value]


def _ratio(n: int, d: int) -> tuple[object, str]:
    value = round(n / d, 4) if d else 0.0
    return value, f"{value:.2%}"


def _count(n: int) -> tuple[object, str]:
    return n, f"{n:,}"


def _mb(n_bytes: float) -> tuple[object, str]:
    size = n_bytes / 1e6
    return round(size, 1), f"{size:.1f} MB"


# ---- derived measurements: name -> (sources needed, fn(sources, member) -> (value, shown)) ----

def _d_units_attributed(src: _Sources, _m: str | None):
    assert src.folio is not None
    return _count(src.folio.execute(
        "select count(distinct chunk_id) from unit_persons").fetchone()[0])


def _d_units_per_person(src: _Sources, _m: str | None):
    assert src.folio is not None
    per = sorted(r[0] for r in src.folio.execute(
        "select count(*) from unit_persons group by person_id"))
    if not per:
        return None
    mean, median = sum(per) / len(per), per[len(per) // 2]
    return (round(mean, 1), median), f"mean {mean:.1f} / median {median}"


def _d_folio_mb(src: _Sources, _m: str | None):
    return _mb(src.folio_path.stat().st_size)


def _d_vectors_mb(src: _Sources, member: str | None):
    """f16 vectors at the given dimension (default 1024): computed, not measured."""
    assert src.folio is not None
    dim = int(member) if member else 1024
    return _mb(src.folio.execute("select count(*) from chunks").fetchone()[0] * dim * 2)


def _d_redundancy(src: _Sources, member: str | None):
    """Highest positional redundancy over all templates, or one template's."""
    table = src.meta["folio"].get("redundancy") or {}
    if not isinstance(table, Mapping) or not table:
        return None
    value = float(table.get(member, 0.0)) if member else max(float(v) for v in table.values())
    return value, f"{value:.3f}"


def _d_boundary_aligned(src: _Sources, _m: str | None):
    clean, split = _boundaries(src)
    return _ratio(clean, clean + split)


def _d_boundary_split(src: _Sources, _m: str | None):
    _clean, split = _boundaries(src)
    return _count(split)


def _d_speakers_distinct(src: _Sources, _m: str | None):
    return _count(len(src.speech_by_person()))


def _d_scorable_at(src: _Sources, member: str | None):
    """Roster persons with at least N spoken lines (N = member)."""
    n = _threshold(member)
    roster = src.roster()
    return _count(sum(1 for p, v in src.speech_by_person().items() if p in roster and len(v) >= n))


def _d_heldout_lines(src: _Sources, member: str | None):
    """Held-out lines at `HOLDOUT_FRACTION` over the persons `scorable_at` counts."""
    n = _threshold(member)
    roster = src.roster()
    return _count(sum(int(len(v) * HOLDOUT_FRACTION)
                      for p, v in src.speech_by_person().items()
                      if p in roster and len(v) >= n))


def _d_name_leak(src: _Sources, member: str | None):
    """Share of spoken lines that contain their own speaker's name or an alias of it."""
    assert src.archive is not None
    n = _threshold(member)
    names: dict[str, set[str]] = {}
    for alias, target in src.archive.execute("select alias, target from aliases"):
        names.setdefault(target, set()).add(alias)
    leaked = total = 0
    for person, lines in src.speech_by_person().items():
        if len(lines) < n:
            continue
        own = {x for x in names.get(person, set()) | {person} if len(x) >= MIN_NAME_CHARS}
        leaked += sum(1 for text in lines if any(x in text for x in own))
        total += len(lines)
    return _ratio(leaked, total)


def _d_cooccur_pairs(src: _Sources, _m: str | None):
    """Pairs of persons who speak in at least one scene together, read from `cooccur`."""
    assert src.folio is not None
    return _count(src.folio.execute("select count(*) from cooccur").fetchone()[0])


def _d_cooccur_scenes_per_person(src: _Sources, _m: str | None):
    """Mean number of scenes a person speaks in, over persons who speak in any."""
    assert src.folio is not None
    names = src.shaped("dialogue")
    if not names:
        return None
    marks = ",".join("?" * len(names))
    per = [r[0] for r in src.folio.execute(
        "select count(distinct c.span_of) from unit_persons u join chunks c on c.id = u.chunk_id "
        f"where c.template in ({marks}) group by u.person_id", names)]
    if not per:
        return None
    mean = sum(per) / len(per)
    return round(mean, 1), f"{mean:.1f}"


def _d_birthdays(src: _Sources, _m: str | None):
    """Persons whose birthday the builder could read as `MM-DD`."""
    assert src.folio is not None
    return _count(src.folio.execute(
        "select count(*) from persons where birthday is not null").fetchone()[0])


def _d_stopword_top_df(src: _Sources, member: str | None):
    """Share of units containing the most widespread query-side stopword (or `member`).

    Read from the lexical index itself, so it measures the tokens retrieval sees.
    """
    assert src.folio is not None
    words = [member] if member else list(src.meta["folio"].get("stopwords") or ())
    units = src.folio.execute("select count(*) from chunks").fetchone()[0]
    if not words or not units:
        return None
    df = {w: src.folio.execute(
        "select count(*) from chunks_fts where chunks_fts match ?",
        ('"' + w.replace('"', '""') + '"',)).fetchone()[0] for w in words}
    word = max(df, key=lambda w: (df[w], w))
    value, shown = _ratio(df[word], units)
    return value, f"{shown} ({word})"


def _d_pipeline_hours(src: _Sources, _m: str | None):
    """Wall-clock hours of the pipeline: the last full sync plus the last build.

    Stage times are stamped into the archive manifest (`timings`) by the CLI. Until a
    full sync has been timed, the value is the build alone and says so.
    """
    timings = src.meta["manifest"].get("timings") or {}
    if not isinstance(timings, Mapping) or "build" not in timings:
        return None
    sync = timings.get("sync_full")
    hours = (float(timings["build"]) + float(sync or 0)) / 3600
    shown = f"{hours:.2f} h" + ("" if sync is not None else " (build only; no timed full sync)")
    return round(hours, 2), shown


def _d_people_with_material(src: _Sources, member: str | None):
    """Persons with at least N retrieval units of their own (default 5)."""
    assert src.folio is not None
    n = int(member) if member else 5
    return _count(src.folio.execute(
        "select count(*) from (select person_id from unit_persons "
        "group by person_id having count(*) >= ?)", (n,)).fetchone()[0])


def _d_pages(src: _Sources, _m: str | None):
    """Pages whose body the raw cache holds."""
    assert src.archive is not None
    if not src.has_raw:
        return None
    return _count(src.archive.execute(
        "select count(*) from raw.pages where wikitext is not null").fetchone()[0])


def _d_persons(src: _Sources, _m: str | None):
    assert src.archive is not None
    return _count(src.archive.execute("select count(*) from persons").fetchone()[0])


def _d_units_with_revid(src: _Sources, _m: str | None):
    """Units that name the source revision they came from."""
    assert src.folio is not None
    return _count(src.folio.execute(
        "select count(*) from chunks where revid is not null").fetchone()[0])


def _d_pack_lines(src: _Sources, _m: str | None):
    """Non-blank lines of Python in the pack: how much rule code a wiki needs."""
    root = pack_dir(src.pack.name)
    return _count(sum(1 for path in root.rglob("*.py")
                      for line in path.read_text(encoding="utf-8").splitlines() if line.strip()))


def _d_roster_pages(src: _Sources, _m: str | None):
    assert src.archive is not None
    return _count(src.archive.execute("select count(*) from forms").fetchone()[0])


def _d_forms(src: _Sources, member: str | None):
    """Pages of one form kind (member), or all non-canonical pages."""
    assert src.archive is not None
    if member:
        n = src.archive.execute("select count(*) from forms where kind = ?", (member,)).fetchone()[0]
    else:
        n = src.archive.execute(
            "select count(*) from forms where kind <> 'canonical'").fetchone()[0]
    return _count(n)


def _d_form_body_links(src: _Sources, member: str | None):
    """Pages of one form kind whose body text links their own person's canonical page.

    Checks the claim that a form relation is declared only by markup: if it were also
    written as an ordinary link, a link scan would be an alternative signal.
    """
    assert src.archive is not None
    if not src.has_raw:
        return None
    sql = ("select f.page, f.person_id, p.wikitext from forms f join raw.pages p "
           "on p.title = f.page where f.kind <> 'canonical'")
    args: tuple = ()
    if member:
        sql += " and f.kind = ?"
        args = (member,)
    n = 0
    for _page, person, text in src.archive.execute(sql, args):
        if re.search(r"\[\[\s*" + re.escape(person) + r"\s*[\]|]", text or ""):
            n += 1
    return _count(n)


def _d_multi_form_persons(src: _Sources, _m: str | None):
    assert src.archive is not None
    return _count(src.archive.execute(
        "select count(*) from persons where form_count > 1").fetchone()[0])


def _d_aliases(src: _Sources, member: str | None):
    assert src.archive is not None
    if member:
        return _count(src.archive.execute(
            "select count(*) from aliases where kind = ?", (member,)).fetchone()[0])
    return _count(src.archive.execute("select count(*) from aliases").fetchone()[0])


def _findings(src: _Sources) -> list[tuple[str, str, str]]:
    """(guard, severity, detail) for every finding of the latest run of each stage."""
    assert src.archive is not None
    return src.once("findings", lambda: [tuple(r) for r in src.archive.execute(
        "select guard, severity, detail from guard_findings")])  # type: ignore[return-value]


def _by_guard(src: _Sources) -> dict[str, list[int]]:
    """[high, low] per guard, every guard listed, including those with nothing to report:
    a guard absent from the counts is otherwise indistinguishable from one that never ran."""
    counts = {g: [0, 0] for g in GUARDS}
    for guard, severity, _detail in _findings(src):
        counts.setdefault(guard, [0, 0])[0 if severity == HIGH else 1] += 1
    return counts


def _guard_sum(src: _Sources, member: str | None, column: int):
    counts = _by_guard(src)
    if member:
        return _count(counts.get(member, [0, 0])[column])
    return _count(sum(v[column] for v in counts.values()))


def _d_guards_high(src: _Sources, member: str | None):
    """High-severity findings across every guard, or one guard's (member)."""
    return _guard_sum(src, member, 0)


def _d_guards_low(src: _Sources, member: str | None):
    return _guard_sum(src, member, 1)


def _d_guards_by_guard(src: _Sources, member: str | None):
    """Findings per guard as `high/low`; `member` picks one guard's total."""
    counts = _by_guard(src)
    if member:
        return _count(sum(counts.get(member, [0, 0])))
    shown = " · ".join(f"{g} {h}/{lo}" for g, (h, lo) in counts.items())
    return {g: {"high": h, "low": lo} for g, (h, lo) in counts.items()}, shown


def _d_guards_unattributed(src: _Sources, _m: str | None):
    """Low-severity findings no reviewed note in the pack explains (`Pack.finding_notes`)."""
    notes = src.pack.finding_notes
    return _count(sum(
        1 for guard, severity, detail in _findings(src)
        if severity != HIGH and not any(n.explains(guard, detail) for n in notes)))


def _d_tokens_unit(src: _Sources, member: str | None):
    """Median tokens of one injected unit; `member` is a builder shape."""
    count = src.counter()
    if count is None or not member:
        return None
    n = tokens_mod.unit_tokens(src.folio, src.shaped(member), count)
    return None if n is None else _count(n)


def _d_tokens_line(src: _Sources, _m: str | None):
    count = src.counter()
    if count is None:
        return None
    n = tokens_mod.line_tokens(src.archive, count)
    return None if n is None else _count(n)


def _d_tokens_retrieval(src: _Sources, member: str | None):
    """Median tokens of a lexical top-k block; `member` is k."""
    count = src.counter()
    if count is None:
        return None
    from ..segment import load as load_segmenter
    name = str(src.meta["folio"].get("segmenter") or "")
    try:
        seg = load_segmenter("jieba" if name.startswith("jieba") else "none")
    except ImportError:
        return None
    stop = set(src.meta["folio"].get("stopwords") or ())
    n = tokens_mod.retrieval_tokens(src.folio, src.suite, int(member or 6),
                                    segment=seg, stopwords=stop, count=count)
    return None if n is None else _count(n)


#: name -> (artifacts it reads, how). A figure whose inputs are missing is skipped.
DERIVED: dict[str, tuple[tuple[str, ...], Callable[[_Sources, str | None], object]]] = {
    "pages": (("archive",), _d_pages),
    "persons": (("archive",), _d_persons),
    "pack_lines": ((), _d_pack_lines),
    "units_with_revid": (("folio",), _d_units_with_revid),
    "units_attributed": (("folio",), _d_units_attributed),
    "units_per_person": (("folio",), _d_units_per_person),
    "folio_mb": (("folio",), _d_folio_mb),
    "vectors_mb": (("folio",), _d_vectors_mb),
    "redundancy": (("folio",), _d_redundancy),
    "boundary_aligned": (("archive", "folio"), _d_boundary_aligned),
    "boundary_split": (("archive", "folio"), _d_boundary_split),
    "speakers_distinct": (("archive",), _d_speakers_distinct),
    "scorable_at": (("archive",), _d_scorable_at),
    "heldout_lines": (("archive",), _d_heldout_lines),
    "name_leak": (("archive",), _d_name_leak),
    "cooccur_pairs": (("folio",), _d_cooccur_pairs),
    "cooccur_scenes_per_person": (("folio",), _d_cooccur_scenes_per_person),
    "birthdays": (("folio",), _d_birthdays),
    "stopword_top_df": (("folio",), _d_stopword_top_df),
    "pipeline_hours": (("archive",), _d_pipeline_hours),
    "people_with_material": (("folio",), _d_people_with_material),
    "roster_pages": (("archive",), _d_roster_pages),
    "forms_by_kind": (("archive",), _d_forms),
    "form_body_links": (("archive",), _d_form_body_links),
    "multi_form_persons": (("archive",), _d_multi_form_persons),
    "aliases": (("archive",), _d_aliases),
    "guards_high": (("archive",), _d_guards_high),
    "guards_low": (("archive",), _d_guards_low),
    "guards_by_guard": (("archive",), _d_guards_by_guard),
    "guards_unattributed": (("archive",), _d_guards_unattributed),
    "tokens_unit": (("folio",), _d_tokens_unit),
    "tokens_line": (("archive",), _d_tokens_line),
    "tokens_retrieval": (("folio",), _d_tokens_retrieval),
}

_OPS: dict[str, Callable[[float, float], bool]] = {
    "<=": operator.le, ">=": operator.ge, "==": operator.eq, "!=": operator.ne,
    "<": operator.lt, ">": operator.gt,
}
_TARGET = re.compile(r"^\s*(<=|>=|==|!=|<|>)\s*(-?\d+(?:\.\d+)?)\s*$")


def meets(value: object, target: str) -> bool | None:
    """Evaluate `target` against a measured value. None if either is not numeric."""
    m = _TARGET.match(target)
    if m is None:
        raise ValueError(f"unreadable figure target {target!r}; expected e.g. '< 512'")
    if isinstance(value, tuple):
        value = value[0]
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return _OPS[m.group(1)](float(value), float(m.group(2)))


def resolve(pack: Pack, paths: Paths) -> list[Figure]:
    """Turn the pack's specs into measured figures. Values come only from artifacts."""
    a, f = _connect(paths.archive), _connect(paths.folio)
    has_raw = a is not None and paths.rawcache.exists()
    if has_raw:
        a.execute("ATTACH DATABASE ? AS raw", (f"file:{paths.rawcache}?immutable=1",))
    try:
        src = _Sources(archive=a, folio=f, folio_path=paths.folio,
                       meta={"manifest": _meta(a), "folio": _meta(f)}, has_raw=has_raw,
                       pack=pack, suite=paths.evals / "structural.queries.jsonl")
        have = {"archive": a is not None, "folio": f is not None}
        out: list[Figure] = []
        for spec in pack.figures:
            kind, _, name = spec.source.partition(":")
            measured: object = None
            if kind in src.meta:
                raw = src.meta[kind].get(name)
                if isinstance(raw, Mapping) and spec.member is not None:
                    raw = raw.get(spec.member)
                if isinstance(raw, Mapping):
                    raw = sum(v for v in raw.values() if isinstance(v, int))
                if raw is not None:
                    measured = (raw, f"{raw:,}" if isinstance(raw, int) else str(raw))
            elif kind == "derived":
                if name not in DERIVED:
                    raise KeyError(f"figure {spec.key!r}: no derived measurement {name!r}")
                needs, fn = DERIVED[name]
                if all(have[n] for n in needs):
                    measured = fn(src, spec.member)
            if measured is None:
                continue
            value, shown = measured  # type: ignore[misc]
            passed = meets(value, spec.target) if spec.target else None
            out.append(Figure(spec.key, value, shown, spec.note, spec.target, passed))
        return out
    finally:
        for con in (a, f):
            if con is not None:
                con.close()


# --------------------------------------------------------------------------- #
# Scanning
# --------------------------------------------------------------------------- #

_LINK = re.compile(r"[`(]((?:\.\./)*[\w./-]+\.md)(?:#[^`)]*)?[`)]")
_SPAN = re.compile(r"`([^`\n]+)`")
#: A dotted key, optionally with one `{…}` group: `a.b`, `a.{x,y}`, `a.{template}.c`.
_KEY = re.compile(r"[a-z][a-z0-9_]*(?:\.[a-z0-9_]*(?:\{[^{}]+\}[a-z0-9_]*)?)+")
_GROUP = re.compile(r"\{([^{}]+)\}")
#: Alternatives inside a group: plain key segments, or `…` meaning "and so on".
_MEMBER = re.compile(r"[a-z0-9_]+")
#: A backticked dotted name ending in one of these is a file, not a figure key.
_FILE_SUFFIXES = frozenset({
    "md", "py", "rs", "js", "json", "jsonl", "toml", "yaml", "yml", "txt", "gguf", "sql",
    "archive", "rawcache", "folio", "html", "css", "ts", "sh",
})


@dataclass
class Hit:
    path: Path
    line_no: int
    what: str
    detail: str

    @property
    def where(self) -> str:
        return f"{self.path}:{self.line_no}"


@dataclass
class Report:
    hits: list[Hit] = field(default_factory=list)
    scanned: int = 0


def _keys_in(span: str) -> list[str]:
    """The figure keys a backticked span names, as patterns.

    `a.b` is one key. `a.{x,y}` expands to `a.x` and `a.y`; an alternative written as
    `…` or `...` is dropped. A group holding a single word (`a.{name}`, in any script)
    is a placeholder, returned as `*`: it matches any registered member there.
    """
    if not _KEY.fullmatch(span):
        return []
    m = _GROUP.search(span)
    if m is None:
        return [span]
    head, tail = span[:m.start()], span[m.end():]
    parts = [p.strip() for p in re.split("[,\uff0c]", m.group(1))]
    members = [p for p in parts if _MEMBER.fullmatch(p)]
    if len(parts) == 1 or not members:
        # A placeholder stands for a whole segment, so it can only sit between dots.
        return [f"{head}*{tail}"] if head.endswith(".") and tail[:1] in ("", ".") else []
    return [f"{head}{member}{tail}" for member in members]


def _known(key: str, known: set[str]) -> bool:
    """Whether `key` names a known key. `*` on either side matches one segment."""
    if key in known:
        return True
    parts = key.split(".")
    return any(
        len(k.split(".")) == len(parts)
        and all(a == b or "*" in (a, b) for a, b in zip(parts, k.split("."), strict=True))
        for k in known)


def _defined_ids(docs: Mapping[Path, list[str]], prefixes: Iterable[str]) -> dict[str, set[str]]:
    """Ids the tree defines, by table-row or heading position. Deliberately loose: the aim
    is to catch an id nothing anywhere defines, not to police where definitions live."""
    defined: dict[str, set[str]] = {k: set() for k in prefixes}
    for lines in docs.values():
        for line in lines:
            stripped = line.lstrip()
            for kind in defined:
                for m in re.finditer(
                    rf"(?:^\|\s*|^#+\s*|^)\**~*{re.escape(kind)}(\d{{1,3}})~*\**\s*(?:\||·|—|~|\s|$)",
                    stripped,
                ):
                    defined[kind].add(m.group(1))
    return defined


def scan(root: Path, pack: Pack) -> Report:
    """Read every markdown file under `root` and report what does not resolve."""
    audit = pack.audit
    paths = sorted(p for p in root.rglob("*.md") if ".git" not in p.parts)
    docs = {p: p.read_text(encoding="utf-8").splitlines() for p in paths}
    rep = Report(scanned=len(paths))
    ids = {k: re.compile(rf"\b{re.escape(k)}(\d{{1,3}})\b") for k in audit.id_prefixes}
    defined = _defined_ids(docs, audit.id_prefixes)
    known_keys = {f.key for f in pack.figures} | set(audit.planned_keys)
    # Only roots of computed figures are checked: other dotted names in a document are
    # parameters or paths, not report keys, even when they share a planned key's root.
    roots = {f.key.split(".", 1)[0] for f in pack.figures}

    for path, lines in docs.items():
        for i, line in enumerate(lines, 1):
            for m in _LINK.finditer(line):
                if not (path.parent / m.group(1)).resolve().exists():
                    rep.hits.append(Hit(path, i, "dangling link", m.group(1)))
            for kind, pattern in ids.items():
                for m in pattern.finditer(line):
                    if len(m.group(1)) >= 2 and m.group(1) not in defined[kind]:
                        rep.hits.append(Hit(path, i, "undefined id",
                                            f"{kind}{m.group(1)} is referenced but never defined"))
            for m in _SPAN.finditer(line):
                for key in _keys_in(m.group(1).strip()):
                    if key.rsplit(".", 1)[-1] in _FILE_SUFFIXES:
                        continue
                    # Only roots that have a registered key are report keys; any other
                    # dotted name (a calibration parameter, a module path) is not ours.
                    if key.split(".", 1)[0] in roots and not _known(key, known_keys):
                        rep.hits.append(Hit(path, i, "unknown key",
                                            f"`{key}` is neither computed nor planned"))
    return rep


# --------------------------------------------------------------------------- #
# Rendering
# --------------------------------------------------------------------------- #


def verdict(fig: Figure) -> str:
    if fig.passed is None:
        return fig.target
    return f"{'✅' if fig.passed else '❌'} {fig.target}"


def render(figures: Iterable[Figure], pack: Pack) -> str:
    """The generated figure report a document links to instead of quoting."""
    figures = list(figures)
    fingerprint = next((f.shown for f in figures if f.key == "fingerprint"), "")
    lines = [pack.say("figures.title"), "", pack.say("figures.intro"), ""]
    if fingerprint:
        lines += [pack.say("figures.fingerprint", fingerprint=fingerprint), ""]
    lines += table_head(pack.say("figures.columns"))
    for fig in figures:
        lines.append(f"| `{fig.key}` | {fig.shown} | {verdict(fig)} | {fig.note} |")
    return "\n".join(lines) + "\n"
