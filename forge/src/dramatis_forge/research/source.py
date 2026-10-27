"""A read-only view of a folio shaped for pair construction.

Everything the pair families need is already in the folio: aliases (redirects, declared
equivalents, disambiguation), heading paths (unit titles), speaker attribution
(`unit_persons`) and a person's forms (`persons.forms`). The archive is not read; the
folio is the artifact the retriever searches, so a pair can only name a unit that
exists there.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field

from ..folio import Folio


@dataclass(frozen=True)
class Unit:
    id: str
    ord: int
    template: str
    shape: str
    page: str
    title: str
    header: str
    text: str
    span_of: str


@dataclass
class Person:
    id: str
    primary_page: str
    display: str
    forms: list[dict]
    facets: dict[str, str]


@dataclass
class Corpus:
    units: dict[str, Unit] = field(default_factory=dict)
    order: list[str] = field(default_factory=list)
    persons: dict[str, Person] = field(default_factory=dict)
    #: unit id -> sorted person ids
    unit_persons: dict[str, tuple[str, ...]] = field(default_factory=dict)
    #: alias kind -> [(alias, target)]
    aliases: dict[str, list[tuple[str, str]]] = field(default_factory=dict)
    by_page: dict[str, list[str]] = field(default_factory=dict)
    by_person: dict[str, list[str]] = field(default_factory=dict)
    by_title: dict[str, list[str]] = field(default_factory=dict)
    fingerprint: str = ""
    chunk_count: int = 0

    def text_of(self, uid: str) -> str:
        u = self.units[uid]
        return f"{u.header}\n{u.text}"

    def persons_of(self, uid: str) -> tuple[str, ...]:
        return self.unit_persons.get(uid, ())

    def page_persons(self) -> dict[str, set[str]]:
        """Every person a page involves: speakers of its units, and the owner of any form
        page. A form page with no units still ties its person to that page."""
        out: dict[str, set[str]] = {p: set() for p in self.by_page}
        for uid, people in self.unit_persons.items():
            if uid in self.units:
                out.setdefault(self.units[uid].page, set()).update(people)
        for person in self.persons.values():
            for form in person.forms:
                out.setdefault(form["page"], set()).add(person.id)
            out.setdefault(person.primary_page, set()).add(person.id)
        return out

    def profile_unit(self, person_id: str) -> str | None:
        """The person's identity card: first profile-shaped unit on the primary page, else
        on any of their pages."""
        person = self.persons.get(person_id)
        if person is None:
            return None
        pages = [person.primary_page] + [f["page"] for f in person.forms]
        for page in pages:
            for uid in self.by_page.get(page, []):
                if self.units[uid].shape == "profile":
                    return uid
        return None

    def primary_unit(self, target: str) -> str | None:
        """The unit that stands for an entity: a person's card, else the first unit the
        entity's page or title names."""
        if target in self.persons:
            return self.profile_unit(target)
        for pool in (self.by_page, self.by_title):
            ids = pool.get(target)
            if ids:
                return ids[0]
        return None


def load(folio: Folio) -> Corpus:
    c = Corpus()
    declared = folio.get_meta("template_shapes") or {}
    c.fingerprint = str(folio.get_meta("build_fingerprint") or "")
    c.chunk_count = int(folio.get_meta("chunk_count") or 0)
    for r in folio.db.execute(
        "SELECT id,ord,template,page,title,header,text,span_of FROM chunks ORDER BY ord"
    ):
        u = Unit(r["id"], r["ord"], r["template"], declared.get(r["template"], r["template"]),
                 r["page"], r["title"] or "", r["header"] or "", r["text"],
                 r["span_of"] or r["id"])
        c.units[u.id] = u
        c.order.append(u.id)
        c.by_page.setdefault(u.page, []).append(u.id)
        if u.title:
            c.by_title.setdefault(u.title, []).append(u.id)
    tmp: dict[str, list[str]] = {}
    for r in folio.db.execute(
        "SELECT u.chunk_id, u.person_id FROM unit_persons u JOIN chunks c ON c.id = u.chunk_id "
        "ORDER BY c.ord, u.person_id"
    ):
        tmp.setdefault(r["chunk_id"], []).append(r["person_id"])
        c.by_person.setdefault(r["person_id"], []).append(r["chunk_id"])
    c.unit_persons = {k: tuple(v) for k, v in tmp.items()}
    for r in folio.db.execute("SELECT person_id,primary_page,display,forms,facets FROM persons "
                              "ORDER BY person_id"):
        c.persons[r["person_id"]] = Person(
            r["person_id"], r["primary_page"], r["display"],
            json.loads(r["forms"]), json.loads(r["facets"]))
    for r in folio.db.execute("SELECT alias,target,kind FROM aliases ORDER BY kind,alias,target"):
        c.aliases.setdefault(r["kind"], []).append((r["alias"], r["target"]))
    return c
