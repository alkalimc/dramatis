"""Shared fixtures.

The helpers here exist so a test can state a *rule* rather than assemble machinery:
`ctx("wikitext")` gives a page context wired to the real pack, so a parser test reads
as "given this markup, expect these records" and nothing else.
"""

from __future__ import annotations

from collections.abc import Iterable
from pathlib import Path

import pytest

from dramatis_forge.normalize.records import Record
from dramatis_forge.normalize.wikitext import Cleaner
from dramatis_forge.pack import Pack, PageContext, load_pack
from dramatis_forge.store.archive import Archive


@pytest.fixture(scope="session")
def pack() -> Pack:
    return load_pack("arknights")


@pytest.fixture(scope="session")
def cleaner(pack: Pack) -> Cleaner:
    return Cleaner(pack.inline)


@pytest.fixture
def archive(tmp_path: Path) -> Archive:
    with Archive(tmp_path / "t.archive") as store:
        yield store


@pytest.fixture
def ctx(cleaner: Cleaner):
    """Build a PageContext for one page of markup.

    `tables` accepts the flat form `{"chara": {"page": {...}}}` so a test can supply the
    structured side-channel without constructing the nested storage shape by hand.
    """
    def build(
        wikitext: str,
        *,
        title: str = "测试页",
        seed: str = "S1",
        revid: int | None = 1,
        tables: dict | None = None,
    ) -> PageContext:
        shaped: dict[str, dict[str, list[dict]]] = {}
        for table, by_page in (tables or {}).items():
            for page, rows in by_page.items():
                shaped.setdefault(table, {})[page] = rows if isinstance(rows, list) else [rows]
        return PageContext(
            title=title, wikitext=wikitext, revid=revid, seed=seed,
            clean=cleaner, tables=shaped,
        )
    return build


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
