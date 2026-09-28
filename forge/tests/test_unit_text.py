"""Unit headers and bodies carry no game-engine metadata: trigger keys, page-part suffixes."""

from __future__ import annotations

from dataclasses import replace

import pytest

from dramatis_forge import chunk, corpus
from dramatis_forge.archive import Archive
from dramatis_forge.records import Choice, Line, Scene, Voice


@pytest.fixture
def tuned(pack):
    """The toy pack with a voice template, a trigger table and a page-part rule."""
    policy = pack.chunking
    policy = replace(
        policy,
        templates=(*policy.templates, replace(policy.templates[0], name="voice",
                                              sources=("voice",))),
        headers={**policy.headers, "voice": "[voice] {person} · {name}{variant}"},
        labels={"options": "possible replies"},
        triggers={"HELLO": "greeting"},
        page_part=r"/(?:BEG|END)$",
    )
    return replace(pack, chunking=policy)


def units(tmp_path, pack, records, template):
    with Archive(tmp_path / "t.archive") as a:
        for kind in dict.fromkeys(type(r) for r in records):
            a.insert_records([r for r in records if type(r) is kind])
        return [c for c in chunk.build(a, pack) if c.template == template]


def test_voice_header_names_the_line_not_the_engine_key(tmp_path, tuned):
    got = units(tmp_path, tuned, [
        Voice(page="Ann/voice", subject="Ann", idx=0, text="Hi.", title="", trigger="HELLO"),
        Voice(page="Ann/voice", subject="Ann", idx=1, text="Bye.", title="Parting",
              trigger="BYE", seq=1),
        Voice(page="Ann/voice", subject="Ann", idx=1, text="Bye!", title="Parting",
              trigger="BYE", variant="outfit", seq=2),
    ], "voice")
    assert [c.header for c in got] == [
        "[voice] Ann · greeting", "[voice] Ann · Parting", "[voice] Ann · Parting · outfit"]
    assert all("HELLO" not in c.header and "BYE" not in c.header for c in got)


def test_story_header_drops_the_page_part_but_the_unit_keeps_its_page(tmp_path, tuned):
    [unit] = units(tmp_path, tuned, [
        Scene(page="Ch 1/BEG", group="Book"),
        Line(scene="Ch 1/BEG", seq=0, text="Hello.", speaker="Ann"),
        Choice(scene="Ch 1/BEG", seq=1, options=("Yes", "No", "Maybe")),
    ], "dialogue")
    assert unit.header == "[scene] Book · Ch 1 · Ann"
    assert (unit.page, unit.title, unit.span_of) == ("Ch 1/BEG", "Ch 1/BEG", "Ch 1/BEG")
    # Every option is kept, under the pack's label for what the player could reply.
    assert unit.text.splitlines()[-1] == "[possible replies] Yes / No / Maybe"


def test_engine_metadata_in_a_header_is_detected(tuned):
    test = chunk.engine_meta_test(tuned.chunking, ["HELLO", "BATTLE_START"])
    assert test("voice", "P", "[voice] Ann · HELLO")
    assert test("voice", "P", "[voice] Ann · éBATTLE_START")
    assert not test("voice", "P", "[voice] Ann · HELLOS greeting")
    assert test("dialogue", "Ch 1/BEG", "[scene] Book · Ch 1/BEG · Ann")
    assert not test("dialogue", "Ch 1/BEG", "[scene] Book · Ch 1 · Ann")
    assert not test("dialogue", "Ch 1", "[scene] Book · Ch 1 · Ann")


def test_a_trigger_the_pack_does_not_name_is_reported(built, tuned, tmp_path):
    with Archive(built.archive) as a:
        a.insert_records([Voice(page="Alice", subject="Alice", idx=0, text="Hm.",
                                title="Idle", trigger="IDLE")])
        a.commit()
        rep = corpus.run(a, tuned, tmp_path / "f.folio", segmenter="bigram")
    assert rep.unnamed_triggers == ["IDLE"] and rep.engine_meta_headers == 0
    assert any("IDLE" in f.detail for f in rep.ledger.findings)
