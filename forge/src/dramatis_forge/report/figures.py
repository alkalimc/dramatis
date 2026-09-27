"""Measure an artifact, report the figures, and check a document tree against them.

Guards check the corpus. `G6` checks that an artifact agrees with itself. Neither checks
the third thing that can be wrong: **a document quoting a number the artifact never
produced.** That is the gap this closes.

Three parts:

*Measuring* — every figure is computed here from the artifacts, from a manifest key or a
named derived measurement. Which figures exist is the pack's choice (`Pack.figures`); how
each derived one is computed is the framework's, because it is schema knowledge.

*Targets* — a figure may carry an acceptance condition (`FigureSpec.target`), evaluated
here and shown as pass or fail. A condition a document states in prose cannot be checked;
one declared beside the figure is checked on every build.

*Scanning* — a document that quotes a superseded measurement is making a checkable claim
that is false, and a cross-reference to something undefined is a consistency claim nobody
checked. Neither pass understands prose; both report file, line and text and leave the
judgement to a reader.
"""

from __future__ import annotations

import json
import operator
import re
import sqlite3
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path

from . import tokens as tokens_mod
from ..pack import DocAudit, FigureSpec, Pack
from ..text import table_head

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
    retired: tuple[str, ...] = ()
    note: str = ""
    target: str = ""
    #: None when there is no target; otherwise whether the value meets it.
    passed: bool | None = None


@dataclass
class Hit:
    path: Path
    line_no: int
    line: str
    what: str
    detail: str

    @property
    def where(self) -> str:
        return f"{self.path}:{self.line_no}"


def num_pattern(n: object) -> str:
    """Regex for an integer as prose writes it, with or without thousands commas.

    The boundary must reject digits *and* commas on both sides: `\\b553\\b` matches
    inside `552,553`, which would report one figure as a stale rendering of another.
    """
    raw = f"{int(n):,}"
    plain = raw.replace(",", "")
    return rf"(?<![\d,])(?:{re.escape(raw)}|{re.escape(plain)})(?![\d,])"


def _current_token(shown: str) -> str:
    """The numeric part of a rendered value, as prose would write it.

    Decimals have to survive intact: stripping non-digits turns `12.5 MB` into `125`,
    which then fails to match a line that says `12.5`.
    """
    match = re.search(r"\d[\d,]*(?:\.\d+)?", shown)
    return match.group(0) if match else ""


def _is_recorded(
    line: str, previous: str, heading: str, current: str, markers: Iterable[str]
) -> bool:
    """Whether a retired value here is being recorded rather than asserted.

    Two signals. A history marker nearby, or — the stronger one — the line also shows the
    current value, which makes it a correction table. Flagging correction tables would
    punish exactly the discipline that keeps documents honest.
    """
    if any(marker in f"{previous} {line} {heading}" for marker in markers):
        return True
    if not current:
        return False
    plain = current.replace(",", "")
    return any(
        re.search(rf"(?<![\d.]){re.escape(form)}(?![\d,])", line)
        for form in {current, plain}
    )


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
    has_raw: bool = False
    pack: Pack | None = None
    suite: Path | None = None
    _cache: dict[str, object] = field(default_factory=dict)

    def counter(self) -> Callable[[str], int] | None:
        """The pack's reference tokenizer, loaded once. None when unavailable."""
        def compute():
            if self.pack is None:
                return None
            from ..pack import pack_dir
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
        "select count(*) from chunks where person is not null").fetchone()[0])


def _d_units_per_person(src: _Sources, _m: str | None):
    assert src.folio is not None
    per = sorted(r[0] for r in src.folio.execute(
        "select count(*) from chunks where person is not null group by person"))
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
    """Distinct pairs of speakers who share at least one scene."""
    assert src.archive is not None
    return _count(src.archive.execute(
        "select count(*) from (select distinct l1.speaker, l2.speaker from lines l1 "
        "join lines l2 on l1.scene = l2.scene and l1.speaker < l2.speaker)").fetchone()[0])


def _d_cooccur_scenes_per_person(src: _Sources, _m: str | None):
    """Mean number of scenes a roster person speaks in."""
    assert src.archive is not None
    per = [r[0] for r in src.archive.execute(
        "select count(distinct l.scene) from lines l join forms f on f.page = l.speaker "
        "group by f.person_id")]
    if not per:
        return None
    mean = sum(per) / len(per)
    return round(mean, 1), f"{mean:.1f}"


def _d_people_with_material(src: _Sources, member: str | None):
    """Persons with at least N retrieval units of their own (default 5)."""
    assert src.folio is not None
    n = int(member) if member else 5
    return _count(src.folio.execute(
        "select count(*) from (select person from chunks where person is not null "
        "group by person having count(*) >= ?)", (n,)).fetchone()[0])


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


def _guard_sum(src: _Sources, member: str | None, column: int):
    assert src.archive is not None
    from ..normalize.guards import GUARDS, HIGH

    counts = {g: [0, 0] for g in GUARDS}
    for guard, severity, n in src.archive.execute(
        "select guard, severity, count(*) from guard_findings group by guard, severity"
    ):
        counts.setdefault(guard, [0, 0])[0 if severity == HIGH else 1] += n
    if member:
        return _count(counts.get(member, [0, 0])[column])
    return _count(sum(v[column] for v in counts.values()))


def _d_guards_high(src: _Sources, member: str | None):
    """High-severity findings across every guard, or one guard's (member)."""
    return _guard_sum(src, member, 0)


def _d_guards_low(src: _Sources, member: str | None):
    return _guard_sum(src, member, 1)


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
    if count is None or src.pack is None or src.suite is None:
        return None
    from ..corpus.tokenize import load as load_segmenter
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
    "cooccur_pairs": (("archive",), _d_cooccur_pairs),
    "cooccur_scenes_per_person": (("archive",), _d_cooccur_scenes_per_person),
    "people_with_material": (("folio",), _d_people_with_material),
    "roster_pages": (("archive",), _d_roster_pages),
    "forms_by_kind": (("archive",), _d_forms),
    "form_body_links": (("archive",), _d_form_body_links),
    "multi_form_persons": (("archive",), _d_multi_form_persons),
    "aliases": (("archive",), _d_aliases),
    "guards_high": (("archive",), _d_guards_high),
    "guards_low": (("archive",), _d_guards_low),
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


def resolve(specs: Iterable[FigureSpec], archive: Path, folio: Path, *,
            pack: Pack | None = None, suite: Path | None = None) -> list[Figure]:
    """Turn pack specs into measured figures. Values come only from artifacts."""
    a, f = _connect(archive), _connect(folio)
    rawcache = archive.with_suffix(".rawcache")
    has_raw = a is not None and rawcache.exists()
    if has_raw:
        a.execute("ATTACH DATABASE ? AS raw", (f"file:{rawcache}?immutable=1",))
    try:
        src = _Sources(archive=a, folio=f, folio_path=folio,
                       meta={"manifest": _meta(a), "folio": _meta(f)}, has_raw=has_raw,
                       pack=pack, suite=suite)
        have = {"archive": a is not None, "folio": f is not None}
        out: list[Figure] = []
        for spec in specs:
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
            retired = tuple(num_pattern(r) if isinstance(r, int) else r for r in spec.retired)
            passed = meets(value, spec.target) if spec.target else None
            out.append(Figure(spec.key, value, shown, retired, spec.note, spec.target, passed))
        return out
    finally:
        for con in (a, f):
            if con is not None:
                con.close()


# --------------------------------------------------------------------------- #
# Scanning
# --------------------------------------------------------------------------- #

_REF = re.compile(r"[`(]((?:\.\./)?[\w./-]+\.md)(?:#[\w-]*)?[`)]")


@dataclass
class Report:
    figures: list[Figure] = field(default_factory=list)
    stale: list[Hit] = field(default_factory=list)
    dangling: list[Hit] = field(default_factory=list)
    scanned: int = 0

    @property
    def clean(self) -> bool:
        return not self.stale and not self.dangling


def _defined_ids(docs: Mapping[Path, list[str]], prefixes: Iterable[str]) -> dict[str, set[str]]:
    """Ids each document actually defines, by table-row or heading position.

    Deliberately loose: the aim is to catch an id nothing anywhere defines, not to
    police where definitions live.
    """
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


def scan(root: Path, figures: Iterable[Figure], audit: DocAudit | None = None) -> Report:
    """Read every markdown file under `root` and report both kinds of rot."""
    audit = audit or DocAudit()
    figures = list(figures)
    paths = sorted(p for p in root.rglob("*.md") if ".git" not in p.parts)
    docs = {p: p.read_text(encoding="utf-8").splitlines() for p in paths}
    rep = Report(figures=figures, scanned=len(paths))
    ids = {k: re.compile(rf"\b{re.escape(k)}(\d{{1,3}})\b") for k in audit.id_prefixes}
    defined = _defined_ids(docs, audit.id_prefixes)
    known = {p.resolve() for p in paths}
    current = {f.key: _current_token(f.shown) for f in figures}
    seen: set[tuple[Path, int, str]] = set()

    for path, lines in docs.items():
        heading = ""
        for i, line in enumerate(lines, 1):
            if line.lstrip().startswith("#"):
                heading = line
            previous = lines[i - 2] if i >= 2 else ""

            for fig in figures:
                token = (path, i, fig.key)
                if token in seen:
                    continue
                matched = any(re.search(pattern, line) for pattern in fig.retired)
                if matched and not _is_recorded(
                    line, previous, heading, current[fig.key], audit.history_markers
                ):
                    rep.stale.append(Hit(
                        path, i, line.strip(), fig.key,
                        f"retired rendering; current value is {fig.shown}"))
                    seen.add(token)

            for m in _REF.finditer(line):
                target = (path.parent / m.group(1)).resolve()
                if target not in known and not target.exists():
                    rep.dangling.append(Hit(path, i, line.strip(), "path", m.group(1)))
            for kind, pattern in ids.items():
                for m in pattern.finditer(line):
                    n = m.group(1)
                    if len(n) >= 2 and n not in defined[kind]:
                        rep.dangling.append(Hit(
                            path, i, line.strip(), f"{kind}-id",
                            f"{kind}{n} is referenced but never defined"))
    return rep


# --------------------------------------------------------------------------- #
# Rendering
# --------------------------------------------------------------------------- #


def verdict(fig: Figure) -> str:
    if fig.passed is None:
        return fig.target
    return f"{'✅' if fig.passed else '❌'} {fig.target}"


def render(figures: Iterable[Figure], pack: Pack, *, fingerprint: str = "") -> str:
    """The generated figure report a document links to instead of quoting."""
    lines = [pack.say("figures.title"), "", pack.say("figures.intro"), ""]
    if fingerprint:
        lines += [pack.say("figures.fingerprint", fingerprint=fingerprint), ""]
    lines += table_head(pack.say("figures.columns"))
    for fig in figures:
        lines.append(f"| `{fig.key}` | {fig.shown} | {verdict(fig)} | {fig.note} |")
    return "\n".join(lines) + "\n"


def figures_for(pack: Pack) -> tuple[FigureSpec, ...]:
    """A pack's figure registry, or empty if it declares none."""
    return tuple(pack.figures)
