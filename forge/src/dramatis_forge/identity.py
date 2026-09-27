"""A page is not a person.

Wikis give one entity several pages when the fiction gives it several presentations
— a later self, a second guise, a variant. Treating each page as an agent produces two
contacts for one character, each holding half of its material and none of its
memories. It also poisons any speaker-discrimination metric, which
would be graded on telling a person apart from herself.

The engine owns the *model*: a Person with ordered Forms, canonical form first.
The pack owns the *signal*, because which template declares the relation is
site-specific. The contract in `IdentityRules.resolve` forbids inferred identity:
name similarity, shared prefixes, edit distance. That is not fastidiousness. An
inferred join has no authoritative source to reconcile against, so a wrong one can
never be found — whereas a declaration that stops parsing shows up as a guard
failure the same day.

Forms are kept rather than flattened because the difference between forms is
itself material: the same character's later lines can carry a different register, and
a persona generator that can see "this is how they sound now" can use it. Flattening
averages that away.

Form kinds other than `canonical` are the pack's own strings and are never interpreted
here. Expected counts are measured and accepted (`forge baseline accept`), not declared.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field

from .guards import HIGH, Finding
from .records import Alias
from .pack import IdentityRules

CANONICAL = "canonical"
#: Alias kind for "this page is one of that person's forms".
FORM_ALIAS = "form"


@dataclass(frozen=True, slots=True)
class Form:
    page: str
    kind: str
    ordinal: int = 0


@dataclass(slots=True)
class Person:
    person_id: str
    forms: list[Form] = field(default_factory=list)


@dataclass(slots=True)
class Roster:
    """The resolved identity map, plus the alias records it implies."""

    people: dict[str, Person] = field(default_factory=dict)
    #: page -> person_id, for every page including canonical ones.
    of_page: dict[str, str] = field(default_factory=dict)
    findings: list[Finding] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.people)

    def aliases(self) -> list[Alias]:
        """Non-canonical form names are site-curated synonyms for the person.

        This is the same class of free supervision as a redirect, and it has to
        exist for query normalisation to work: a user naming another form must
        reach the one person, and a line spoken under that name must attribute to
        them.
        """
        return [
            Alias(alias=f.page, target=p.person_id, kind=FORM_ALIAS)
            for p in self.people.values()
            for f in p.forms
            if f.kind != CANONICAL and f.page != p.person_id
        ]

    def counts(self) -> dict[str, int]:
        """What the identity baseline records: persons, and pages per non-canonical kind."""
        out: dict[str, int] = {"persons": len(self.people)}
        for person in self.people.values():
            for f in person.forms:
                if f.kind != CANONICAL:
                    out[f"form:{f.kind}"] = out.get(f"form:{f.kind}", 0) + 1
        return out

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

    Three guards, all G4 and all correctness: a declaration we cannot resolve is a
    parser that broke. The count check lives with the caller (`Roster.counts` against the
    accepted baseline): a change in any form-kind count is a **tripwire on novelty** — a
    new kind of grouping may have shipped, and a human should look before it silently
    reshapes the roster — while the person count is expected to grow and not to shrink.
    """
    out = Roster()
    declared = dict(rules.resolve(pages, roster_titles))

    # 1. every declaration must land inside the roster
    for page, (canonical, _kind) in sorted(declared.items()):
        if canonical not in roster_titles:
            out.findings.append(Finding(
                "G4", HIGH, f"{page} declares {canonical!r} but it is not in the roster", page))
            declared.pop(page, None)

    # 2. no chains: a declaration whose target is itself a declared non-canonical
    #    page would make identity depend on traversal order.
    for page, (canonical, _kind) in sorted(declared.items()):
        if canonical in declared and declared[canonical][0] != canonical:
            out.findings.append(Finding(
                "G4", HIGH,
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
                "G4", HIGH,
                f"{person.person_id} has forms but no canonical page — "
                "the roster entry and portrait would have no source",
                person.person_id,
            ))

    return out
