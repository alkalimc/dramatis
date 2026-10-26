"""Mechanisms of the data foundation: forms, exclusion, attribution, rendering."""

from __future__ import annotations

from dataclasses import replace

import pytest

from dramatis_forge import chunk
from dramatis_forge.archive import Archive
from dramatis_forge.guards import LOW
from dramatis_forge.identity import resolve
from dramatis_forge.pack import IdentityEvidence
from dramatis_forge.records import Dossier, Voice


def test_pages_the_pack_excludes_leave_the_roster_with_their_forms(pack):
    seen = []

    def excluded(pages, roster, evidence):
        seen.append(evidence)
        return {"Ghost"}

    rules = replace(pack.identity, excluded=excluded)
    pages = {"Alice": "", "Ghost": "", "Ghost (Winter)": "{{AltForm|Ghost}}"}
    ev = IdentityEvidence(spoken={"Alice": 3})
    roster = resolve(rules, pages, frozenset(pages), ev)
    assert set(roster.people) == {"Alice"}
    assert sorted(roster.excluded) == ["Ghost", "Ghost (Winter)"]
    assert set(roster.of_page) == {"Alice"}
    assert [f.severity for f in roster.findings] == [LOW]
    assert seen == [ev]


@pytest.fixture
def forms_pack(pack):
    policy = replace(
        pack.chunking,
        headers={**pack.chunking.headers, "voice": "[voice] {person} · {name}{variant}"},
        templates=(*pack.chunking.templates,
                   replace(pack.chunking.templates[0], name="voice", sources=("voice",))),
        separators={"form": " <{}>"},
        card_omit=("rarity",),
        shared_min=2,
    )
    return replace(pack, chunking=policy, identity=replace(pack.identity, display_facet="name"))


def insert(archive, records):
    for kind in dict.fromkeys(type(r) for r in records):
        archive.insert_records([r for r in records if type(r) is kind])


def make(tmp_path, pack):
    a = Archive(tmp_path / "t.archive")
    a.write_identity({
        "Alice": [("Alice", "canonical", 0), ("Alice (Winter)", "alt", 1)],
        "Bob": [("Bob", "canonical", 0)],
    })
    insert(a, [
        Dossier(page="Alice", fields={"name": "Alice", "rarity": "6", "age": "30"},
                sections=({"title": "Life", "text": "Born in the north."},),
                items={"outfit": "A plain coat.", "memo": "Own words."}, revid=10),
        Dossier(page="Alice (Winter)", fields={"name": "Winter", "rarity": "6", "age": "31"},
                sections=({"title": "Life", "text": "Older now."},),
                items={"outfit": "A plain coat."}, revid=20),
        Dossier(page="Bob", fields={"name": "Bob"}, items={"outfit": "A plain coat."},
                revid=30),
        Voice(page="Alice/v", subject="Alice", idx=0, text="Hello.", title="Greeting"),
        Voice(page="Alice (Winter)/v", subject="Alice (Winter)", idx=0, text="Hi.",
              title="Greeting"),
    ])
    return a


def test_forms_are_told_apart_in_headers_and_cite_their_own_revision(tmp_path, forms_pack):
    with make(tmp_path, forms_pack) as a:
        units = list(chunk.build(a, forms_pack))
    voices = {c.header: c.text for c in units if c.template == "voice"}
    assert voices == {"[voice] Alice · Greeting": "Hello.",
                      "[voice] Alice <Winter> · Greeting": "Hi."}
    life = {c.header: c.revid for c in units if c.header.endswith("Life")}
    assert life == {"[file] Alice · Life": 10, "[file] Alice <Winter> · Life": 20}


def test_a_form_card_holds_only_what_differs_and_omits_mechanics(tmp_path, forms_pack):
    with make(tmp_path, forms_pack) as a:
        units = [c for c in chunk.build(a, forms_pack) if c.span_of and c.span_of.endswith("#card")]
    cards = {c.header: c.text for c in units}
    assert cards["[file] Alice · identity card"] == "name: Alice\nage: 30\nother forms: Winter"
    assert cards["[file] Alice <Winter> · identity card"] == "name: Winter\nage: 31"
    assert all("rarity" not in c.text for c in units)


def test_a_text_every_person_shares_is_stored_once_and_owned_by_nobody(tmp_path, forms_pack):
    with make(tmp_path, forms_pack) as a:
        units = list(chunk.build(a, forms_pack))
    coat = [c for c in units if c.text == "A plain coat."]
    assert len(coat) == 1 and coat[0].persons == ()
    assert any(c.text == "Own words." and c.persons == ("Alice",) for c in units)


def test_markup_left_in_a_unit_is_a_high_finding(tmp_path, pack):
    from dramatis_forge import corpus
    from dramatis_forge.records import Lore
    with Archive(tmp_path / "t.archive") as a:
        insert(a, [Lore(page="P", path=("S",), text="Fine words. --> hidden tail", revid=1)])
        rep = corpus.run(a, pack, tmp_path / "t.folio", segmenter="bigram")
    assert rep.markup_residue == {"-->": 1}
    assert any(f.guard == "G5" and f.severity == "high" and "markup" in f.detail
               for f in rep.ledger.findings)
