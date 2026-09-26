"""Shared fixtures for the framework's own tests.

**Nothing here loads a pack.** This repository holds mechanism; the rules that drive it
live with whoever supplies a pack, and so do the tests that assert those rules. A fixture
here that reached for a concrete pack would put domain knowledge back on this side of the
seam, which is the arrangement this layout exists to prevent.

Tests that need a pack — parser behaviour, identity rules, end-to-end stages — belong
beside that pack.
"""

from __future__ import annotations

from collections.abc import Iterable
from pathlib import Path

import pytest

from dramatis_forge.normalize.records import Line, Lore, Record, Scene
from dramatis_forge.pack import (
    ChunkPolicy, ChunkTemplate, IdentityRules, InlineRules, Pack, PageContext, Route, SeedSet,
    WikiConfig,
)
from dramatis_forge.store.archive import Archive


def _parse_scene(ctx: PageContext):
    """`speaker: text` per line; a line without a colon is narration."""
    yield Scene(page=ctx.title, category="cat", group="grp", revid=ctx.revid)
    for i, raw in enumerate(l for l in ctx.wikitext.splitlines() if l.strip()):
        speaker, sep, text = raw.partition(": ")
        if sep:
            yield Line(scene=ctx.title, seq=i + 1, text=text, speaker=speaker)
        else:
            yield Line(scene=ctx.title, seq=i + 1, text=raw, kind="narration")


def _parse_prose(ctx: PageContext):
    if not ctx.wikitext.strip():
        ctx.empty("blank page")
        return
    for sec in ctx.clean.split_sections(ctx.wikitext, min_chars=1):
        yield Lore(page=ctx.title, path=sec["path"], text=sec["text"], revid=ctx.revid)


def _no_identity(pages, roster):
    return {}


@pytest.fixture
def toy_pack() -> Pack:
    """A minimal pack built here, so no real pack is ever loaded by these tests."""
    return Pack(
        name="toy", version=1,
        wiki=WikiConfig(api="https://wiki.example/api.php", contact="ops@example.org",
                        source_url="https://wiki.example/w/{page}",
                        history_url="https://wiki.example/h/{page}",
                        revision_url="https://wiki.example/r/{revid}"),
        seeds=(
            SeedSet("A", "scenes", "fixed", fixed=("s1",)),
            SeedSet("B", "prose", "fixed", fixed=("p1",)),
            SeedSet("R", "roster", "fixed", fixed=(), roster=True, corpus=False),
        ),
        routes=(Route("A", _parse_scene, label="scene"), Route("B", _parse_prose, label="prose")),
        followups=(),
        inline=InlineRules(),
        identity=IdentityRules(resolve=_no_identity),
        chunking=ChunkPolicy(templates=(
            ChunkTemplate("talk", sources=("line",), shape="dialogue", target=2, max_span=3),
            ChunkTemplate("lore", sources=("lore",)),
        ), headers={"talk": "{group} {scene}{speakers}", "lore": "{page}{path}"}),
        stopwords=("the", "a"),
    )


@pytest.fixture
def archive(tmp_path: Path) -> Archive:
    with Archive(tmp_path / "t.archive") as store:
        yield store


@pytest.fixture
def of_kind():
    """Filter records by kind. A fixture rather than an importable helper so test
    modules need no package plumbing to reach it."""
    def pick(records: Iterable[Record], kind: str) -> list[Record]:
        return [r for r in records if r.KIND == kind]
    return pick


@pytest.fixture
def kinds():
    def names(records: Iterable[Record]) -> list[str]:
        return [r.KIND for r in records]
    return names
