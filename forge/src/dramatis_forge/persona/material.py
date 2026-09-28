"""One person's material, read from the folio, tagged with the form it comes from.

The union over every form of the person: the identity card, dossier sections and
archive quotes, character notes, voice lines (as text), and a bounded sample of their
own story lines. Each item carries `form_page`, so the generator can see that a second
form speaks differently rather than averaging the two.

What a unit is comes from the folio's own declarations: the builder shape of its
template (`template_shapes`) and the suffix of its span container (`#card`, `#dossier`,
`#ref`, `#voice`), both written by the chunk builders.

The input is bounded by `persona.material_tokens`, of which `persona.story_tokens` is the
story sample. Kinds are filled in priority order; within a kind, canonical form first,
then corpus order. Whatever does not fit is counted, not silently lost.
"""

from __future__ import annotations

import json
import math
import re
import sqlite3
import unicodedata
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field

from .generator import Generator, HostSource
from .params import Persona

HOST = "host"

#: Fill order: what says most about how a person speaks and who they are comes first.
KINDS = ("card", "voice", "dossier", "quote", "char_ref", "lore")
#: Builder shapes whose units hold a person's own words rather than words about them.
SPEECH_SHAPES = ("voice", "dialogue")

Counter = Callable[[str], int]


def estimate_tokens(text: str) -> int:
    """Token estimate when the pack's reference tokenizer is unavailable.

    One token per wide (East Asian full-width) character, one per four other characters.
    Subword tokenizers spend at most about one token on a wide character and about four
    characters of Latin text per token, so this errs on the long side, which is the safe
    side for a budget gate.
    """
    wide = sum(1 for ch in text if unicodedata.east_asian_width(ch) in ("W", "F"))
    return wide + math.ceil((len(text) - wide) / 4)


@dataclass(frozen=True)
class Item:
    kind: str
    form_page: str
    title: str
    text: str
    tokens: int


@dataclass(frozen=True)
class Line:
    form_page: str
    scene: str
    text: str
    #: Tokens of `text` and `before` together: both are sent.
    tokens: int
    #: The line just before it in the unit, when it is not their own: what the line
    #: answers (another speaker, narration, a caption, a branch option). A bare line of
    #: dialogue says little about how someone speaks until you see what it replies to.
    before: str = ""


@dataclass
class Material:
    subject: str
    display: str
    forms: list[str]
    items: list[Item] = field(default_factory=list)
    story: list[Line] = field(default_factory=list)
    #: Items and lines left out for budget, per kind.
    dropped: dict[str, int] = field(default_factory=dict)
    #: Story lines found before sampling.
    story_found: int = 0

    @property
    def empty(self) -> bool:
        return not self.items and not self.story

    @property
    def tokens(self) -> int:
        return sum(i.tokens for i in self.items) + sum(s.tokens for s in self.story)

    def counts(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for i in self.items:
            out[i.kind] = out.get(i.kind, 0) + 1
        if self.story:
            out["story"] = len(self.story)
        return out

    def render(self, gen: Generator, sep: str) -> str:
        """The material block as the generator sees it: one section per kind, one item
        per line group, each prefixed with its form page in brackets."""
        blocks: list[str] = []
        for kind in KINDS:
            rows = [i for i in self.items if i.kind == kind]
            if not rows:
                continue
            body = []
            for i in rows:
                head = f"[{i.form_page}] " + (f"{i.title}{sep}" if i.title else "")
                body.append(head + i.text)
            blocks.append(f"## {gen.label_for(kind)}\n" + "\n\n".join(body))
        if self.story:
            body, scene = [], None
            for s in self.story:
                if s.scene != scene:
                    scene = s.scene
                    body.append(f"### {scene}")
                if s.before:
                    body.append(f"  {s.before}")
                body.append(f"[{s.form_page}] {s.text}")
            blocks.append(f"## {gen.label_for('story')}\n" + "\n".join(body))
        return "\n\n".join(blocks)


def _shapes(db: sqlite3.Connection) -> dict[str, str]:
    row = db.execute("SELECT value FROM manifest WHERE key='template_shapes'").fetchone()
    return json.loads(row[0]) if row else {}


def speech_units(db: sqlite3.Connection) -> dict[str, int]:
    """Per person, the voice and dialogue units attributed to them. A dialogue unit is
    attributed to every speaker in it, so each one counted holds at least a line of theirs.
    Templates are matched by their declared shape, as everywhere in this module."""
    shapes = _shapes(db)
    templates = [t for (t,) in db.execute("SELECT DISTINCT template FROM chunks")
                 if shapes.get(t, t) in SPEECH_SHAPES]
    if not templates:
        return {}
    marks = ",".join("?" * len(templates))
    return dict(db.execute(
        "SELECT u.person_id, COUNT(*) FROM unit_persons u JOIN chunks c ON c.id = u.chunk_id "
        f"WHERE c.template IN ({marks}) GROUP BY u.person_id", templates))


def _kind(shape: str, span_of: str | None) -> str | None:
    suffix = (span_of or "").rpartition("#")[2]
    if shape == "profile":
        return {"card": "card", "ref": "char_ref"}.get(suffix, "dossier")
    if shape == "voice":
        return "voice"
    if shape == "lore":
        return "lore"
    return None  # dialogue is sampled line by line; letters are not persona material


class _Rules:
    """The generator's reclassification and line-drop rules, compiled once per run."""

    def __init__(self, gen: Generator) -> None:
        self.kinds = [(src, re.compile(rx), dst) for src, rx, dst in gen.title_kinds]
        self.drops = [re.compile(rx) for rx in gen.line_drops]

    def kind(self, kind: str | None, title: str) -> str | None:
        if kind is None:
            return None
        for src, rx, dst in self.kinds:
            if src == kind and rx.fullmatch(title or ""):
                return None if dst == "drop" else dst
        return kind

    @staticmethod
    def clean(text: str) -> str:
        # Verbatim, placeholder included: the user's name is substituted only when the
        # runtime assembles a session.
        return text.strip()

    def body(self, text: str) -> str:
        """A unit body without the lines the generator drops (a stat row on an identity
        card, a provenance note), otherwise verbatim."""
        if not self.drops:
            return self.clean(text)
        kept = [ln for ln in text.splitlines()
                if not any(rx.fullmatch(ln.strip()) for rx in self.drops)]
        return self.clean("\n".join(kept))


def _speaker_lines(text: str, names: set[str],
                   sep: str) -> Iterable[tuple[str, str, str]]:
    """(speaker, words, before) for each line of a dialogue unit spoken by one of `names`;
    `before` is the preceding line unless that one is theirs too."""
    before = ""
    for raw in text.splitlines():
        raw = raw.strip()
        if not raw:
            continue
        speaker, found, words = raw.partition(sep)
        if found and speaker in names and words.strip():
            yield speaker, words.strip(), before
            before = ""
        else:
            before = raw


def _sample(lines: list[Line], budget: int) -> list[Line]:
    """An evenly spaced sample that fits `budget`, in corpus order.

    Evenly spaced rather than the first N: a person's early scenes and late scenes are
    often different people in all but name, and the brief should hear both.
    """
    total = sum(s.tokens for s in lines)
    if total <= budget:
        return lines
    mean = total / len(lines)
    k = max(1, int(budget / mean))
    while k > 0:
        step = len(lines) / k
        picked = [lines[int(i * step)] for i in range(k)]
        if sum(s.tokens for s in picked) <= budget:
            return picked
        k -= 1
    return []


def _budget(mat: Material, items: list[Item], story: list[Line], p: Persona) -> None:
    story_budget = min(p.story_tokens, sum(s.tokens for s in story))
    mat.story_found = len(story)
    mat.story = _sample(story, story_budget) if story else []
    if len(mat.story) < len(story):
        mat.dropped["story"] = len(story) - len(mat.story)
    room = p.material_tokens - sum(s.tokens for s in mat.story)
    order = {k: i for i, k in enumerate(KINDS)}
    for item in sorted(items, key=lambda i: order[i.kind]):  # stable: form order kept
        if item.tokens <= room:
            mat.items.append(item)
            room -= item.tokens
        else:
            mat.dropped[item.kind] = mat.dropped.get(item.kind, 0) + 1


def for_person(db: sqlite3.Connection, person_id: str, gen: Generator, p: Persona,
               count: Counter, sep: str) -> Material:
    row = db.execute("SELECT display, primary_page, forms FROM persons WHERE person_id=?",
                     (person_id,)).fetchone()
    if row is None:
        raise KeyError(person_id)
    display, primary, forms_json = row
    forms = [f["page"] for f in json.loads(forms_json)] or [primary]
    rank = {page: i for i, page in enumerate(forms)}
    names = {person_id, display, *forms} | {
        a for (a,) in db.execute(
            "SELECT alias FROM aliases WHERE target=? AND kind<>'disambig'", (person_id,))}
    shapes = _shapes(db)
    rules = _Rules(gen)

    items: list[Item] = []
    story: list[Line] = []
    seen: set[str] = set()
    units = db.execute(
        "SELECT c.template, c.page, c.title, c.text, c.span_of FROM unit_persons u "
        "JOIN chunks c ON c.id = u.chunk_id WHERE u.person_id=? ORDER BY c.ord", (person_id,))
    for template, page, title, text, span_of in units:
        shape = shapes.get(template, template)
        if shape == "dialogue":
            for speaker, words, before in _speaker_lines(text, names, sep):
                words = rules.clean(words)
                if not words or words in seen:
                    continue
                seen.add(words)
                form = speaker if speaker in rank else primary
                story.append(Line(form, title or page, words, count(before + words), before))
            continue
        kind = rules.kind(_kind(shape, span_of), title)
        if kind is None:
            continue
        body = rules.body(text)
        if not body:
            continue
        form = span_of.partition("#")[0] if kind in ("voice", "char_ref") and span_of else page
        shown = "" if kind == "card" else (title or "")
        items.append(Item(kind, form, shown, body, count(shown + body)))
    items.sort(key=lambda i: rank.get(i.form_page, len(rank)))  # canonical form first

    mat = Material(subject=person_id, display=display, forms=forms)
    _budget(mat, items, story, p)
    return mat


def for_host(db: sqlite3.Connection, src: HostSource, gen: Generator, p: Persona,
             count: Counter, sep: str) -> Material:
    """The host is not a roster person, so its material is found by name: its lines in
    dialogue units, and units whose title (or last breadcrumb segment) names it."""
    shapes = _shapes(db)
    dialogue = [t for t, s in shapes.items() if s == "dialogue"]
    names = set(src.names)
    rules = _Rules(gen)
    display = src.names[0] if src.names else HOST
    story: list[Line] = []
    seen: set[str] = set()
    if dialogue and names:
        marks = ",".join("?" * len(dialogue))
        likes = " OR ".join("instr(text, ?) > 0" for _ in names)
        for title, page, text in db.execute(
            f"SELECT title, page, text FROM chunks WHERE template IN ({marks}) "
            f"AND ({likes}) ORDER BY ord", [*dialogue, *(f"{n}{sep}" for n in names)],
        ):
            for _speaker, words, before in _speaker_lines(text, names, sep):
                words = rules.clean(words)
                if words and words not in seen:
                    seen.add(words)
                    # Tagged with the host, like a person's lines with their form: the
                    # scene is already the group heading.
                    story.append(Line(display, title or page, words, count(before + words),
                                      before))
    items: list[Item] = []
    for template, page, title, text, span_of in db.execute(
        "SELECT template, page, title, text, span_of FROM chunks ORDER BY ord"
    ):
        if not title or not any(title == t or title.endswith(f" › {t}") for t in src.titles):
            continue
        kind = rules.kind(_kind(shapes.get(template, template), span_of), title)
        if kind is None:
            continue
        body = rules.body(text)
        if body:
            items.append(Item(kind, page, title, body, count(title + body)))
    mat = Material(subject=HOST, display=display, forms=list(src.names))
    _budget(mat, items, story, p)
    return mat
