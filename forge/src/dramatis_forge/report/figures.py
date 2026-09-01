"""Check a document tree against the artifact it claims to describe.

Guards check the corpus. `G6` checks that an artifact agrees with itself. Neither
checks the third thing that can be wrong: **a document quoting a number the artifact
never produced.** That is the gap this closes.

Two passes, because two different things rot:

*Figures* — a document that quotes a superseded measurement is not merely out of date,
it is making a checkable claim that is false. Every figure is computed here from the
artifact, and the renderings a document has previously claimed are scanned for. Which
figures matter, and which renderings are retired, are supplied by the pack: both are
domain knowledge, and the retired list is design history.

*References* — a pointer to another document, or to a decision id, is only worth
writing if the target exists. Cross-references are how a document set stays consistent,
so a dangling one is a consistency claim nobody checked.

Neither pass understands prose. Both report file, line and offending text and leave the
judgement to a reader; the aim is a short list of places worth looking at.
"""

from __future__ import annotations

import json
import re
import sqlite3
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path

from ..pack import FigureSpec, Pack


@dataclass(frozen=True)
class Figure:
    """A spec resolved against an artifact."""

    key: str
    value: object
    shown: str
    retired: tuple[str, ...] = ()
    note: str = ""


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

    The boundary must reject digits *and* commas on both sides. `\\b553\\b` matches
    inside `552,553`, which is how this checker reported a character total as a stale
    few-shot count on its first run.
    """
    raw = f"{int(n):,}"
    plain = raw.replace(",", "")
    return rf"(?<![\d,])(?:{re.escape(raw)}|{re.escape(plain)})(?![\d,])"


#: Phrases marking a number as a *recorded* past value rather than a live claim. A
#: decision record is supposed to contain superseded figures — that is its job — so a
#: blanket file exemption would hide real staleness where a marker test does not.
_HISTORICAL = (
    "此前", "原本", "曾", "推翻", "已改", "改为", "旧", "当时", "第一次实测",
    "过时", "被否", "作废", "~~", "原表述", "而实际", "设计估算", "纸上",
    "先测出", "改切分", "被探针改掉", "更早", "原设计", "原判", "预期", "原",
    "supersed", "previously", "was ",
)


def _current_token(shown: str) -> str:
    """The numeric part of a rendered value, as prose would write it.

    Decimals have to survive intact. Stripping non-digits turns `120.5 MB` into `1205`,
    which then fails to match a line that says `120.5` — so a correction table reading
    `193 MB | 120.5 MB` gets reported as stale. That was this checker's own bug, and it
    is the same class of defect it exists to catch.
    """
    match = re.search(r"\d[\d,]*(?:\.\d+)?", shown)
    return match.group(0) if match else ""


def _is_recorded(line: str, previous: str, heading: str, current: str) -> bool:
    """Whether a retired value here is being recorded rather than asserted.

    Two signals. The second carries the most weight: a line showing the retired value
    *and* the current one is a correction table, and correction tables are how these
    documents stay honest. Flagging them would punish the discipline the project runs on.
    """
    if any(marker in f"{previous} {line} {heading}" for marker in _HISTORICAL):
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

def _meta(db: Path) -> dict[str, object]:
    if not db.exists():
        return {}
    con = sqlite3.connect(f"file:{db}?immutable=1", uri=True)
    try:
        out: dict[str, object] = {}
        for key, raw in con.execute("select * from manifest"):
            try:
                out[key] = json.loads(raw) if isinstance(raw, str) else raw
            except (json.JSONDecodeError, TypeError):
                out[key] = raw
        return out
    finally:
        con.close()


def _derived(folio: Path) -> dict[str, tuple[object, str]]:
    """Quantities that need a query rather than a manifest read.

    Named rather than parameterised: a pack asks for `derived:units_per_person`, and the
    framework decides how to get it. Packs describing SQL would put schema knowledge on
    the wrong side of the seam.
    """
    if not folio.exists():
        return {}
    con = sqlite3.connect(f"file:{folio}?immutable=1", uri=True)
    try:
        out: dict[str, tuple[object, str]] = {}
        attributed = con.execute(
            "select count(*) from chunks where person is not null").fetchone()[0]
        out["units_attributed"] = (attributed, f"{attributed:,}")
        per = sorted(r[0] for r in con.execute(
            "select count(*) from chunks where person is not null group by person"))
        if per:
            mean, median = sum(per) / len(per), per[len(per) // 2]
            out["units_per_person"] = (
                (round(mean, 1), median), f"mean {mean:.1f} / median {median}")
        size = folio.stat().st_size / 1e6
        out["folio_mb"] = (round(size, 1), f"{size:.1f} MB")
        units = con.execute("select count(*) from chunks").fetchone()[0]
        vectors = units * 1024 * 2 / 1e6
        out["vectors_mb"] = (round(vectors, 1), f"{vectors:.1f} MB")
        return out
    finally:
        con.close()


def resolve(specs: Iterable[FigureSpec], archive: Path, folio: Path) -> list[Figure]:
    """Turn pack specs into measured figures. Values come only from artifacts."""
    manifests = {"manifest": _meta(archive), "folio": _meta(folio)}
    derived = _derived(folio)
    out: list[Figure] = []

    for spec in specs:
        kind, _, name = spec.source.partition(":")
        value: object = None
        shown = ""
        if kind in manifests:
            raw = manifests[kind].get(name)
            if isinstance(raw, Mapping) and spec.member is not None:
                raw = raw.get(spec.member)
            if isinstance(raw, Mapping):
                raw = sum(v for v in raw.values() if isinstance(v, int))
            if raw is None:
                continue
            value = raw
            shown = f"{raw:,}" if isinstance(raw, int) else str(raw)
        elif kind == "derived":
            if name not in derived:
                continue
            value, shown = derived[name]
        else:
            continue

        retired = tuple(
            num_pattern(r) if isinstance(r, int) else r for r in spec.retired)
        out.append(Figure(spec.key, value, shown, retired, spec.note))
    return out


# --------------------------------------------------------------------------- #
# Scanning
# --------------------------------------------------------------------------- #

_REF = re.compile(r"[`(]((?:\.\./)?[\w./-]+\.md)(?:#[\w-]*)?[`)]")
_IDS = {kind: re.compile(rf"\b{kind}(\d{{1,3}})\b") for kind in ("D", "M", "F", "P")}


@dataclass
class Report:
    figures: list[Figure] = field(default_factory=list)
    stale: list[Hit] = field(default_factory=list)
    dangling: list[Hit] = field(default_factory=list)
    scanned: int = 0

    @property
    def clean(self) -> bool:
        return not self.stale and not self.dangling


def _defined_ids(docs: Mapping[Path, list[str]]) -> dict[str, set[str]]:
    """Ids each document actually defines, by table-row or heading position.

    Deliberately loose: the aim is to catch an id nothing anywhere defines, not to
    police where definitions live.
    """
    defined: dict[str, set[str]] = {k: set() for k in _IDS}
    for lines in docs.values():
        for line in lines:
            stripped = line.lstrip()
            for kind in _IDS:
                for m in re.finditer(
                    rf"(?:^\|\s*|^#+\s*|^)\**~*{kind}(\d{{1,3}})~*\**\s*(?:\||·|—|~|\s|$)",
                    stripped,
                ):
                    defined[kind].add(m.group(1))
    return defined


def scan(root: Path, figures: Iterable[Figure]) -> Report:
    """Read every markdown file under `root` and report both kinds of rot."""
    figures = list(figures)
    paths = sorted(p for p in root.rglob("*.md") if ".git" not in p.parts)
    docs = {p: p.read_text(encoding="utf-8").splitlines() for p in paths}
    rep = Report(figures=figures, scanned=len(paths))
    defined = _defined_ids(docs)
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
                # `any` rather than a loop with `break`: several retired patterns can
                # match one line, and a `break` inside the pattern loop still lets the
                # outer loop re-append. That made the count exceed the number of hits
                # printed — a miscount in the tool built to catch miscounts.
                matched = any(re.search(pattern, line) for pattern in fig.retired)
                if matched and not _is_recorded(
                    line, previous, heading, current[fig.key]
                ):
                    rep.stale.append(Hit(
                        path, i, line.strip(), fig.key,
                        f"retired rendering; current value is {fig.shown}"))
                    seen.add(token)

            for m in _REF.finditer(line):
                target = (path.parent / m.group(1)).resolve()
                if target not in known and not target.exists():
                    rep.dangling.append(
                        Hit(path, i, line.strip(), "path", m.group(1)))
            for kind, pattern in _IDS.items():
                for m in pattern.finditer(line):
                    n = m.group(1)
                    if len(n) >= 2 and n not in defined[kind]:
                        rep.dangling.append(Hit(
                            path, i, line.strip(), f"{kind}-id",
                            f"{kind}{n} is referenced but never defined"))
    return rep


def render(figures: Iterable[Figure], *, fingerprint: str = "") -> str:
    """The generated figure report a design document links to instead of quoting.

    This file existing is what lets the documents stop carrying measurements: a
    document cites a key, a reader follows the link, and the value here came from the
    artifact rather than from someone's memory of it.
    """
    lines = [
        "# 实测数值",
        "",
        "本文由 `dramatis-forge report figures` 生成。**请勿手工编辑。**",
        "",
        "设计文档不写实测数字，只引用本表的**键名**。分界线是：",
        "设计说「应该是多少」，本表说「实际是多少」。",
        "",
    ]
    if fingerprint:
        lines += [f"- 构建指纹：`{fingerprint}`", ""]
    lines += ["| 键 | 值 | 含义 |", "| --- | --- | --- |"]
    for fig in figures:
        lines.append(f"| `{fig.key}` | {fig.shown} | {fig.note} |")
    return "\n".join(lines) + "\n"


def figures_for(pack: Pack) -> tuple[FigureSpec, ...]:
    """A pack's figure registry, or empty if it declares none."""
    return tuple(getattr(pack, "figures", ()) or ())
