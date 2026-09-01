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

from dramatis_forge.normalize.records import Record
from dramatis_forge.store.archive import Archive


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
