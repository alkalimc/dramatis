"""Shared fixtures: the toy pack, a fake wiki, and a built toy corpus in a temp directory.

Nothing here touches the network or any real artifact store: every path is under
pytest's `tmp_path`, and the wiki is an in-memory object with the client's interface.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
for entry in (HERE.parent / "src", HERE / "toypack"):
    if str(entry) not in sys.path:
        sys.path.insert(0, str(entry))

from dramatis_forge import corpus, harvest, normalize  # noqa: E402
from dramatis_forge.archive import Archive  # noqa: E402
from dramatis_forge.config import Paths  # noqa: E402
from dramatis_forge.pack import load_pack  # noqa: E402

#: A term that occurs in exactly one line of the toy corpus; the contract tests search for it.
KNOWN_TERM = "zephyrine"

STORIES = {
    "Chapter 1": (
        "{{Scene|category=Main|group=Chapter One}}\n"
        "> The harbour is quiet.\n"
        "Alice: Good morning, ${player}.\n"
        "Bob: The boats are late again.\n"
        "Alice: Then we wait.\n"
        "[caption] Later that day\n"
        "[choice] Wait with them | Walk away\n"
        "Bob: You chose well.\n"
        "Carol: Nobody asked me.\n"
    ),
    "Chapter 2": (
        "{{Scene|category=Main|group=Chapter Two}}\n"
        + "".join(f"Alice: Line {i} about the lighthouse.\nBob: Reply {i}.\n" for i in range(4))
        + f"Alice: The lamp burns {KNOWN_TERM} oil.\n"
        + "".join(f"Bob: Closing words {i}.\nAlice: Answer {i}.\n" for i in range(3))
    ),
    "Interlude": "{{NoScript}}\n",
    "Silent": "{{Scene|category=Side|group=Nowhere}}\n",
}

PEOPLE = {
    "Alice": (
        "{{Info|born=03-14|home=North Cape|role=Keeper of the northern light}}\n"
        "Alice keeps the lighthouse on the northern cape and writes letters.\n"
        "== Early life ==\n"
        "She grew up in the harbour town, the youngest of five siblings.\n"
        "== Work ==\n"
        "Every night she trims the wick and records the passing ships.\n"
    ),
    "Alice (Winter)": (
        "{{AltForm|Alice}}\n"
        "In winter Alice wears a heavy coat and speaks less than usual.\n"
    ),
    "Bob": "{{Info|born=unknown|home=Harbour|role=Fisherman with two boats}}\n"
           "Bob is a fisherman who owns two boats and a very old dog named Pepper.\n",
}


class FakeWiki:
    """The `Wiki` surface harvest uses, backed by dicts. Counts calls so tests can check
    what a sync asked for."""

    def __init__(self, stories: dict[str, str] | None = None,
                 people: dict[str, str] | None = None) -> None:
        self.pages: dict[str, tuple[str, int]] = {}
        self.category: list[str] = []
        self.changes: list[dict] = []
        self.rcid = 100
        self.revid = 1000
        self.requested: list[str] = []
        for title, text in (stories if stories is not None else STORIES).items():
            self.edit(title, text, story=True, log=False)
        for title, text in (people if people is not None else PEOPLE).items():
            self.edit(title, text, log=False)

    # ---- test-side mutation ----

    def edit(self, title: str, text: str, *, story: bool = False, log: bool = True) -> None:
        self.revid += 1
        self.pages[title] = (text, self.revid)
        if story and title not in self.category:
            self.category.append(title)
        if log:
            self.rcid += 1
            self.changes.insert(0, {"rcid": self.rcid, "title": title})

    def delete(self, title: str) -> None:
        self.pages.pop(title, None)
        if title in self.category:
            self.category.remove(title)

    # ---- the client interface ----

    def categorymembers(self, category: str):
        return list(self.category)

    def cargo(self, table, fields):
        return []

    def redirect_targets(self, titles):
        return {}

    def links(self, titles, ns=0):
        return {}

    def content(self, titles):
        for t in titles:
            self.requested.append(t)
            if t in self.pages:
                text, revid = self.pages[t]
                yield t, text, revid
            else:
                yield t, None, None

    def latest_change_id(self, namespaces):
        return self.rcid

    def recentchanges(self, namespaces):
        # Newest first, ending with an entry at or below any watermark a test sets.
        yield from self.changes
        yield {"rcid": 0, "title": ""}

    def get(self, **params):
        raise AssertionError("tests must not make raw API requests")


@pytest.fixture
def pack():
    return load_pack("toy")


@pytest.fixture
def paths(tmp_path):
    return Paths.for_pack("toy", home=tmp_path).ensure()


@pytest.fixture
def wiki():
    return FakeWiki()


def sync(wiki: FakeWiki, pack, paths: Paths, **kw):
    """Plan and apply one sync; returns (plan, result)."""
    with Archive(paths.archive) as archive:
        p = harvest.plan(wiki, archive, pack, **kw)
        return p, harvest.apply(wiki, archive, pack, p)


def build(pack, paths: Paths, folio: Path | None = None):
    """Normalize and pack the folio; returns (normalize report, corpus report)."""
    with Archive(paths.archive) as archive:
        rep = normalize.run(archive, pack)
    with Archive(paths.archive) as archive:
        crep = corpus.run(archive, pack, folio or paths.folio, segmenter="bigram")
    return rep, crep


@pytest.fixture
def built(wiki, pack, paths):
    """A synced and built toy corpus."""
    sync(wiki, pack, paths)
    build(pack, paths)
    return paths
