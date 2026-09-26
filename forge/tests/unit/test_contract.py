"""The pack contract's fallbacks: text, punctuation, kinds, shapes.

Every piece of human-facing wording and every piece of punctuation a builder writes has a
neutral English default here and a pack override. These tests pin the mechanism with a
toy pack; no real pack is loaded.
"""

from __future__ import annotations

from dataclasses import replace

import pytest

from dramatis_forge.pack import ChunkPolicy, ChunkTemplate
from dramatis_forge.text import TEXT


def test_say_falls_back_to_the_framework_default(toy_pack):
    assert toy_pack.say("coverage.sources") == TEXT["coverage.sources"]


def test_say_prefers_pack_text_and_formats_fields(toy_pack):
    pack = replace(toy_pack, text={"coverage.pages": "held={n}"})
    assert pack.say("coverage.pages", n=7) == "held=7"
    assert pack.say("coverage.title", pack="x") == "# Coverage · x"


def test_an_unknown_text_key_is_an_error_not_a_blank(toy_pack):
    with pytest.raises(KeyError):
        toy_pack.say("no.such.key")


def test_framework_defaults_are_ascii():
    """The open framework picks no corpus language; non-ASCII here would be a choice."""
    offenders = {k: v for k, v in TEXT.items() if not v.isascii()}
    # the only non-ASCII allowed is typographic: middle dot, arrows, emoji check marks
    assert all(set(v) - set(map(chr, range(128))) <= {"·", "›", "→"} for v in offenders.values())


def test_punctuation_defaults_and_overrides():
    policy = ChunkPolicy(templates=())
    assert policy.sep("label") == ": " and policy.mark("x") == "[x] "
    custom = ChunkPolicy(templates=(), separators={"mark": "<{}>"})
    assert custom.mark("x") == "<x>" and custom.sep("list") == ", "


def test_kinds_include_speech_by_default():
    assert ChunkPolicy(templates=()).kind("speech") == "speech"


def test_template_shape_defaults_to_its_name():
    assert ChunkTemplate("lore", sources=()).builder == "lore"
    assert ChunkTemplate("talk", sources=(), shape="dialogue").builder == "dialogue"


def test_policy_finds_templates_by_shape(toy_pack):
    assert toy_pack.chunking.shaped("dialogue") == ("talk",)
