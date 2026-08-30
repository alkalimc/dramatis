"""A page is not a person.

Wikis give one entity several pages when the fiction gives it several presentations
— an alternate incarnation, a second class, a later self. Treating each page as an
agent produces two contacts for one character, each holding half of her material
and none of her memories. It also poisons any speaker-discrimination metric, which
would be graded on telling a person apart from herself.

The engine owns the *model*: a Person with ordered Forms, canonical form first.
The pack owns the *signal*, because which template declares the relation is
site-specific. The contract in `IdentityRules.resolve` forbids inferred identity:
name similarity, shared prefixes, edit distance. That is not fastidiousness. An
inferred join has no authoritative source to reconcile against, so a wrong one can
never be found — whereas a declaration that stops parsing shows up as a guard
failure the same day.

Forms are kept rather than flattened because the difference between forms is
itself material: the same character's later voice lines carry a different register,
and a persona generator that can see "this is how she sounds now" can use it.
Flattening averages that away.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field

from .normalize.guards import Finding
from .normalize.records import Alias
from .pack import IdentityRules

CANONICAL = "canonical"


@dataclass(frozen=True, slots=True)
class Form:
    page: str
    kind: str
    ordinal: int = 0


@dataclass(slots=True)
class Person:
    person_id: str
    forms: list[Form] = field(default_factory=list)

    @property
    def primary_page(self) -> str:
        return self.person_id

    @property
    def pages(self) -> list[str]:
        return [f.page for f in self.forms]


@dataclass(slots=True)
class Roster:
    """The resolved identity map, plus the alias records it implies."""

    people: dict[str, Person] = field(default_factory=dict)
    #: page -> person_id, for every page including canonical ones.
    of_page: dict[str, str] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.people)

    @property
    def multi_form(self) -> list[Person]:
        return [p for p in self.people.values() if len(p.forms) > 1]

    def aliases(self) -> list[Alias]:
        """Non-canonical form names are site-curated synonyms for the person.

        This is the same class of free supervision as a redirect, and it has to
        exist for query normalisation to work: a user naming an alternate form
        must reach the one person, and a line spoken under an alternate name must
        attribute to her.
        """
        return [
            Alias(alias=f.page, target=p.person_id, kind="alter")
            for p in self.people.values()
            for f in p.forms
            if f.kind != CANONICAL and f.page != p.person_id
        ]

    def storage_rows(self) -> dict[str, list[tuple[str, str, int]]]:
        return {
            pid: [(f.page, f.kind, f.ordinal) for f in p.forms]
            for pid, p in self.people.items()
        }


def resolve(
    rules: IdentityRules,
    pages: Mapping[str, str],
    roster_titles: frozenset[str],
) -> Roster:
    """Apply a pack's declarations, then check the invariants the model needs.

    Four guards, all G4. The first two are correctness (a declaration we cannot
    resolve is a parser that broke). The third is a **tripwire on novelty**: if the
    number of pages carrying the variant marker changes, a new kind of grouping has
    shipped and a human should look before it silently reshapes the roster. The
    fourth watches the person count, where growth is expected and shrinkage is not.
    """
    out = Roster()
    declared = dict(rules.resolve(pages, roster_titles))

    # 1. every declaration must land inside the roster
    for page, (canonical, kind) in sorted(declared.items()):
        if canonical not in roster_titles:
            out.findings.append(Finding(
                "G4", "高", f"{page} declares {canonical!r} but it is not in the roster", page))
            declared.pop(page, None)

    # 2. no chains: a declaration whose target is itself a declared non-canonical
    #    page would make identity depend on traversal order.
    for page, (canonical, _kind) in sorted(declared.items()):
        if canonical in declared and declared[canonical][0] != canonical:
            out.findings.append(Finding(
                "G4", "高",
                f"{page} → {canonical} → {declared[canonical][0]}: identity chains are not allowed",
                page,
            ))

    for title in sorted(roster_titles):
        canonical, kind = declared.get(title, (title, CANONICAL))
        person = out.people.setdefault(canonical, Person(person_id=canonical))
        ordinal = 0 if kind == CANONICAL else rules.form_order.get(kind, 50)
        person.forms.append(Form(page=title, kind=kind, ordinal=ordinal))
        out.of_page[title] = canonical

    for person in out.people.values():
        person.forms.sort(key=lambda f: (f.ordinal, f.page))
        if not any(f.kind == CANONICAL for f in person.forms):
            out.findings.append(Finding(
                "G4", "高",
                f"{person.person_id} has forms but no canonical page — "
                "the roster entry and portrait would have no source",
                person.person_id,
            ))

    # 3/4. counts against measured baselines
    kinds: dict[str, int] = {}
    for page, (_canonical, kind) in declared.items():
        kinds[kind] = kinds.get(kind, 0) + 1
    for kind, expected in rules.baselines.items():
        if kind == "persons":
            continue
        actual = kinds.get(kind, 0)
        if actual != expected:
            out.findings.append(Finding(
                "G4", "高" if actual < expected else "低",
                f"form kind {kind!r}: expected {expected}, found {actual} — "
                "a new grouping may have shipped; confirm before it reshapes the roster",
            ))
    expected_people = rules.baselines.get("persons", 0)
    if expected_people:
        n = len(out.people)
        if n < expected_people:
            out.findings.append(Finding(
                "G4", "高", f"person count fell: {expected_people} → {n}"))
        elif n > expected_people:
            out.findings.append(Finding(
                "G4", "低", f"person count grew: {expected_people} → {n} (new characters?)"))
    return out
