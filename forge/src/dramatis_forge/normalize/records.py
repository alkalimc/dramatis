"""The record vocabulary: ten shapes that a narrative corpus reduces to.

These live in the engine rather than in a pack because they are not domain
specific — any corpus built from a story-bearing wiki decomposes into scenes and
their lines, character dossiers, recorded voice lines, encyclopaedic prose,
in-world correspondence, glossaries, and an alias dictionary. A pack decides
*which page yields which record*; it does not get to invent a new record kind
casually, because every kind costs a table, a chunk template, a guard and a
metric.

Two storage rules are load-bearing and were both learnt from data loss:

**Uniqueness must include the text.** Keying `lore` by `(page, section_path)`
means two different paragraphs at the same heading path silently overwrite one
another. Keying it by `(page, section_path, sig)` lets genuine duplicates
collapse while genuine distinct text coexists — and, crucially, makes the two
cases *distinguishable*, which a bare count never is.

**Insert must not replace.** `INSERT OR REPLACE` on a colliding key destroys a
row and reports success. `INSERT OR IGNORE` plus a count of what was ignored
turns the same event into a number that has to be explained.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import Any, ClassVar


#: Bumped when normalisation changes what comes out. It lives with the record
#: vocabulary rather than with the store because it describes the *rules*, not the
#: file: a pack can add a seed set without invalidating any parse, and a parser fix
#: can change every record while the schema stands still.
PARSER_VERSION = 2


def sig(*parts: object) -> str:
    """Short content signature. Not a security boundary — a collision detector."""
    h = hashlib.blake2b(digest_size=8)
    for p in parts:
        h.update(str(p).encode("utf-8"))
        h.update(b"\x1f")
    return h.hexdigest()


def _j(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=False)


@dataclass(frozen=True, slots=True)
class Record:
    """Base for all record kinds.

    Subclasses declare their table, its columns, and how to project themselves
    onto a row. `chars` is what the coverage report sums: the amount of *usable
    text* a record contributes, which is not the same as its serialised size.
    """

    KIND: ClassVar[str] = ""
    TABLE: ClassVar[str] = ""
    COLUMNS: ClassVar[tuple[str, ...]] = ()

    def row(self) -> tuple[Any, ...]:  # pragma: no cover - abstract
        raise NotImplementedError

    @property
    def chars(self) -> int:
        return 0


@dataclass(frozen=True, slots=True)
class Scene(Record):
    """One AVG script: a unit of story with an ordered body of lines."""

    KIND: ClassVar[str] = "scene"
    TABLE: ClassVar[str] = "scenes"
    COLUMNS: ClassVar[tuple[str, ...]] = ("id", "story_type", "story_group", "text_path", "revid")

    page: str
    story_type: str = ""
    story_group: str = ""
    text_path: str = ""
    revid: int | None = None

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.story_type, self.story_group, self.text_path, self.revid)


@dataclass(frozen=True, slots=True)
class Line(Record):
    """One utterance. `kind` distinguishes speech from narration from on-screen text."""

    KIND: ClassVar[str] = "line"
    TABLE: ClassVar[str] = "lines"
    COLUMNS: ClassVar[tuple[str, ...]] = ("scene", "seq", "speaker", "text", "kind")

    scene: str
    seq: int
    text: str
    kind: str = "对白"
    speaker: str | None = None

    def row(self) -> tuple[Any, ...]:
        return (self.scene, self.seq, self.speaker, self.text, self.kind)

    @property
    def chars(self) -> int:
        return len(self.text)


@dataclass(frozen=True, slots=True)
class Choice(Record):
    """A branch point offered to the player character.

    Kept because it is the only place the protagonist has a voice, and because
    the option texts are the sole first-person material for the player's own role.
    """

    KIND: ClassVar[str] = "choice"
    TABLE: ClassVar[str] = "choices"
    COLUMNS: ClassVar[tuple[str, ...]] = ("scene", "seq", "options")

    scene: str
    seq: int
    options: tuple[str, ...] = ()

    def row(self) -> tuple[Any, ...]:
        return (self.scene, self.seq, _j(list(self.options)))

    @property
    def chars(self) -> int:
        return sum(len(o) for o in self.options)


@dataclass(frozen=True, slots=True)
class Dossier(Record):
    """A character's own file: attributes, narrative sections, incidental prose."""

    KIND: ClassVar[str] = "dossier"
    TABLE: ClassVar[str] = "dossiers"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "fields", "sections", "items", "revid")

    page: str
    fields: dict[str, str] = field(default_factory=dict)
    sections: tuple[dict[str, str], ...] = ()
    items: dict[str, Any] = field(default_factory=dict)
    revid: int | None = None

    def row(self) -> tuple[Any, ...]:
        return (self.page, _j(self.fields), _j(list(self.sections)), _j(self.items), self.revid)

    @property
    def chars(self) -> int:
        n = sum(len(s.get("text", "")) for s in self.sections)
        return n + sum(len(v) for v in self.items.values() if isinstance(v, str))


@dataclass(frozen=True, slots=True)
class Voice(Record):
    """A recorded line with its trigger context.

    The trigger is what makes these usable as tone material rather than as loose
    quotations: it says *under what circumstance* the character speaks this way.
    """

    KIND: ClassVar[str] = "voice"
    TABLE: ClassVar[str] = "voices"
    COLUMNS: ClassVar[tuple[str, ...]] = (
        "page", "subject", "idx", "title", "trigger", "text", "unlock",
    )

    page: str
    subject: str
    idx: int
    text: str
    title: str = ""
    trigger: str = ""
    unlock: str = ""

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.subject, self.idx, self.title, self.trigger, self.text, self.unlock)

    @property
    def chars(self) -> int:
        return len(self.text)


@dataclass(frozen=True, slots=True)
class Lore(Record):
    """A section of in-world encyclopaedic prose, addressed by its heading path."""

    KIND: ClassVar[str] = "lore"
    TABLE: ClassVar[str] = "lore"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "path", "sig", "text", "revid")

    page: str
    path: tuple[str, ...]
    text: str
    revid: int | None = None

    def row(self) -> tuple[Any, ...]:
        return (self.page, _j(list(self.path)), sig(self.text), self.text, self.revid)

    @property
    def chars(self) -> int:
        return len(self.text)


@dataclass(frozen=True, slots=True)
class Letter(Record):
    """In-world correspondence: a character's unmediated written voice."""

    KIND: ClassVar[str] = "letter"
    TABLE: ClassVar[str] = "letters"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "sender", "date", "title", "sig", "body")

    page: str
    body: str
    sender: str = ""
    date: str = ""
    title: str = ""

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.sender, self.date, self.title, sig(self.body), self.body)

    @property
    def chars(self) -> int:
        return len(self.body)


@dataclass(frozen=True, slots=True)
class Term(Record):
    """A glossary entry. Feeds query normalisation, not the corpus proper."""

    KIND: ClassVar[str] = "term"
    TABLE: ClassVar[str] = "terms"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "zh", "en", "other", "category")

    page: str
    zh: str
    en: str = ""
    other: str = ""
    category: str = ""

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.zh, self.en, self.other, self.category)

    @property
    def chars(self) -> int:
        return len(self.zh) + len(self.en)


@dataclass(frozen=True, slots=True)
class CharRef(Record):
    """A hand-written description of a character who has no page of their own.

    The most information-dense material in the corpus per character, and the only
    coverage for everyone outside the playable roster. Also the natural few-shot
    and hold-out reference set for persona synthesis, since a human wrote it.
    """

    KIND: ClassVar[str] = "char_ref"
    TABLE: ClassVar[str] = "char_refs"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "name", "story_group", "sig", "description", "source")

    page: str
    name: str
    description: str
    story_group: str = ""
    source: str = ""

    def row(self) -> tuple[Any, ...]:
        return (
            self.page, self.name, self.story_group,
            sig(self.description), self.description, self.source,
        )

    @property
    def chars(self) -> int:
        return len(self.description)


@dataclass(frozen=True, slots=True)
class Alias(Record):
    """A site-declared naming equivalence.

    Redirects, disambiguation candidates, real names and alternate-form names are
    human-curated synonymy that already exists in the wiki. Harvesting it is the
    cheapest labelled data in the project — and, turned around, it is also the
    source of the structural benchmark.
    """

    KIND: ClassVar[str] = "alias"
    TABLE: ClassVar[str] = "aliases"
    COLUMNS: ClassVar[tuple[str, ...]] = ("alias", "target", "kind")

    alias: str
    target: str
    kind: str  # redirect | disambig | realname | alter

    def row(self) -> tuple[Any, ...]:
        return (self.alias, self.target, self.kind)


KINDS: tuple[type[Record], ...] = (
    Scene, Line, Choice, Dossier, Voice, Lore, Letter, Term, CharRef, Alias,
)
BY_KIND: dict[str, type[Record]] = {k.KIND: k for k in KINDS}
ORDER: tuple[str, ...] = tuple(k.KIND for k in KINDS)

#: Human labels for the coverage report, in the pack's language-neutral position:
#: these are English keys; the pack supplies display wording.
LABELS: dict[str, str] = {
    "scene": "scenes",
    "line": "lines (speech / narration / caption)",
    "choice": "branch points",
    "dossier": "character dossiers",
    "voice": "recorded voice lines",
    "lore": "encyclopaedia sections",
    "letter": "letters",
    "term": "glossary terms",
    "char_ref": "non-roster character descriptions",
    "alias": "alias entries",
}
