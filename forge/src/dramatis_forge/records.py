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

**Records keep source order.** Every kind that can hold several records per page
carries `seq`, its position among that page's records of the kind as the parser produced
them (normalisation assigns it). Readers order by it. Sorting by heading path, name or
signature instead reorders the source: a chronology reads out of sequence and a sample
cannot be read side by side with the page it came from.

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
PARSER_VERSION = 3


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

    @property
    def prose(self) -> str:
        """Every piece of source text the record carries, for measuring yield."""
        return ""


@dataclass(frozen=True, slots=True)
class Scene(Record):
    """One scripted scene: a unit of story with an ordered body of lines.

    `category` and `group` are the source's own two-level classification (what kind of
    story, which arc); `source_ref` is wherever the source says the script came from.
    All three are opaque strings the pack fills in.
    """

    KIND: ClassVar[str] = "scene"
    TABLE: ClassVar[str] = "scenes"
    COLUMNS: ClassVar[tuple[str, ...]] = (
        "id", "category", "grp", "source_ref", "revid", "source_page")

    page: str
    category: str = ""
    group: str = ""
    source_ref: str = ""
    revid: int | None = None
    #: The page whose text the body was parsed from, when it is not `page` itself: a
    #: body transcluded from a subpage is stored once, under the scene, and this says
    #: where it was read. `revid` is that page's revision.
    source_page: str = ""

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.category, self.group, self.source_ref, self.revid,
                self.source_page or self.page)


@dataclass(frozen=True, slots=True)
class Line(Record):
    """One utterance. `kind` distinguishes speech from narration from on-screen text.

    The values are the pack's own; `ChunkPolicy.kinds` says which one means what.
    """

    KIND: ClassVar[str] = "line"
    TABLE: ClassVar[str] = "lines"
    COLUMNS: ClassVar[tuple[str, ...]] = ("scene", "seq", "speaker", "text", "kind")

    scene: str
    seq: int
    text: str
    kind: str = "speech"
    speaker: str | None = None

    def row(self) -> tuple[Any, ...]:
        return (self.scene, self.seq, self.speaker, self.text, self.kind)

    @property
    def chars(self) -> int:
        return len(self.text)

    @property
    def prose(self) -> str:
        return self.text


@dataclass(frozen=True, slots=True)
class Choice(Record):
    """A branch point offered to the reader's own character.

    Kept because it is often the only place that character has a voice at all.
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

    @property
    def prose(self) -> str:
        return "".join(self.options)


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

    @property
    def prose(self) -> str:
        parts = [*self.fields.values()]
        parts += [s.get("title", "") + s.get("text", "") for s in self.sections]
        parts += [v if isinstance(v, str) else _j(v) for v in self.items.values()]
        return "".join(parts)


@dataclass(frozen=True, slots=True)
class Voice(Record):
    """A recorded line with its trigger context.

    The trigger is what makes these usable as tone material rather than as loose
    quotations: it says *under what circumstance* the character speaks this way.
    `condition` is whatever the source records about when the line becomes available.
    """

    KIND: ClassVar[str] = "voice"
    TABLE: ClassVar[str] = "voices"
    COLUMNS: ClassVar[tuple[str, ...]] = (
        "page", "subject", "idx", "variant", "seq", "title", "trigger", "text", "condition",
    )

    page: str
    subject: str
    idx: int
    text: str
    title: str = ""
    trigger: str = ""
    condition: str = ""
    #: A second recording of the same slot with different words (an outfit's own voice
    #: set, say): empty for the base line. The source's label, opaque here.
    variant: str = ""
    seq: int = 0

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.subject, self.idx, self.variant, self.seq, self.title,
                self.trigger, self.text, self.condition)

    @property
    def chars(self) -> int:
        return len(self.text)

    @property
    def prose(self) -> str:
        return self.title + self.text + self.condition


@dataclass(frozen=True, slots=True)
class Lore(Record):
    """A section of in-world encyclopaedic prose, addressed by its heading path."""

    KIND: ClassVar[str] = "lore"
    TABLE: ClassVar[str] = "lore"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "seq", "path", "sig", "text", "revid")

    page: str
    path: tuple[str, ...]
    text: str
    revid: int | None = None
    seq: int = 0

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.seq, _j(list(self.path)), sig(self.text), self.text,
                self.revid)

    @property
    def chars(self) -> int:
        return len(self.text)

    @property
    def prose(self) -> str:
        return "".join(self.path) + self.text


@dataclass(frozen=True, slots=True)
class Letter(Record):
    """In-world correspondence: a character's unmediated written voice."""

    KIND: ClassVar[str] = "letter"
    TABLE: ClassVar[str] = "letters"
    COLUMNS: ClassVar[tuple[str, ...]] = (
        "page", "seq", "sender", "date", "title", "sig", "body")

    page: str
    body: str
    sender: str = ""
    date: str = ""
    title: str = ""
    seq: int = 0

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.seq, self.sender, self.date, self.title, sig(self.body),
                self.body)

    @property
    def chars(self) -> int:
        return len(self.body)

    @property
    def prose(self) -> str:
        return self.sender + self.title + self.date + self.body


@dataclass(frozen=True, slots=True)
class Term(Record):
    """A glossary entry. Feeds query normalisation, not the corpus proper.

    `term` is the entry in the corpus language; `translations` maps whatever label the
    source uses for a rendering (a language, a region) to that rendering.
    """

    KIND: ClassVar[str] = "term"
    TABLE: ClassVar[str] = "terms"
    COLUMNS: ClassVar[tuple[str, ...]] = ("page", "seq", "term", "translations", "category")

    page: str
    term: str
    #: Label -> rendering, in the source's column order.
    translations: dict[str, str] = field(default_factory=dict)
    category: str = ""
    seq: int = 0

    def row(self) -> tuple[Any, ...]:
        return (self.page, self.seq, self.term, _j(self.translations), self.category)

    @property
    def chars(self) -> int:
        return len(self.term) + sum(len(v) for v in self.translations.values())

    @property
    def prose(self) -> str:
        return self.term + "".join(self.translations.values())


@dataclass(frozen=True, slots=True)
class CharRef(Record):
    """A hand-written description of a character who has no page of their own.

    Often the most information-dense material per character, and the only coverage
    for everyone outside the roster. Also a natural few-shot and hold-out reference
    set for persona synthesis, since a human wrote it. `group` is the arc or section
    the description is filed under.
    """

    KIND: ClassVar[str] = "char_ref"
    TABLE: ClassVar[str] = "char_refs"
    COLUMNS: ClassVar[tuple[str, ...]] = (
        "page", "seq", "name", "grp", "sig", "description", "source")

    page: str
    name: str
    description: str
    group: str = ""
    source: str = ""
    seq: int = 0

    def row(self) -> tuple[Any, ...]:
        return (
            self.page, self.seq, self.name, self.group,
            sig(self.description), self.description, self.source,
        )

    @property
    def chars(self) -> int:
        return len(self.description)

    @property
    def prose(self) -> str:
        return self.name + self.description + self.source


@dataclass(frozen=True, slots=True)
class Alias(Record):
    """A site-declared naming equivalence.

    Redirects, disambiguation candidates, names declared by index pages and the names
    of a person's other forms are human-curated synonymy that already exists in the wiki. Harvesting it is the
    cheapest labelled data in the project — and, turned around, it is also the
    source of the structural benchmark.
    """

    KIND: ClassVar[str] = "alias"
    TABLE: ClassVar[str] = "aliases"
    COLUMNS: ClassVar[tuple[str, ...]] = ("alias", "target", "kind")

    alias: str
    target: str
    #: `redirect`, `disambig` and `form` are written by the framework; anything else is
    #: a pack's own alias kind, opaque here.
    kind: str

    def row(self) -> tuple[Any, ...]:
        return (self.alias, self.target, self.kind)


KINDS: tuple[type[Record], ...] = (
    Scene, Line, Choice, Dossier, Voice, Lore, Letter, Term, CharRef, Alias,
)
ORDER: tuple[str, ...] = tuple(k.KIND for k in KINDS)
#: Kinds whose records carry `seq`, their order among one page's records.
SEQUENCED: frozenset[str] = frozenset(
    k.KIND for k in KINDS if "seq" in k.COLUMNS and k.KIND not in ("line", "choice"))
