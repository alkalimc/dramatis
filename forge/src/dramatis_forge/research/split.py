"""The train/test split, enforced by code.

Two rules, both applied here and checked again on the finished pairs:

* by page: a page goes to test when a stable hash of (seed, page) falls under the test
  share. The hash, not an RNG, so the split does not move when pages are added or the
  order of enumeration changes, and the same seed reproduces it anywhere.
* by person: every person involved in any test page is a test person. A pair may enter
  the training set only if neither it nor anything it names (positive, negatives)
  involves a test page or a test person.

Mode `page` is the rule as stated. On a story-heavy wiki nearly every recurring person
speaks on some test page, so almost everyone becomes a test person and few training pairs
survive. Mode `person` picks the test persons by hash instead, and a page is a test page
only when everyone on it is a test person (a page with nobody on it falls back to the
page hash). The two rules still hold; the test persons are just chosen first.

`check` is the independent verification: it takes the finished training pairs and the
units they reference, and fails on the first leak, so a construction bug cannot
silently pass.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterable, Mapping
from dataclasses import dataclass

DEFAULT_SEED = 1
DEFAULT_TEST_SHARE = 0.2
MODES = ("page", "person")
DEFAULT_MODE = "person"


class LeakError(Exception):
    """A training pair touches a test page or a test person."""


def page_fraction(seed: int, page: str) -> float:
    digest = hashlib.sha256(f"{seed}\0{page}".encode()).digest()
    return int.from_bytes(digest[:8], "big") / 2**64


@dataclass(frozen=True)
class Split:
    mode: str
    seed: int
    share: float
    train_pages: frozenset[str]
    test_pages: frozenset[str]
    test_persons: frozenset[str]
    train_persons: frozenset[str]

    def split_of_page(self, page: str) -> str:
        return "test" if page in self.test_pages else "train"

    def as_dict(self) -> dict:
        return {
            "mode": self.mode, "seed": self.seed, "test_share": self.share,
            "rule": RULES[self.mode],
            "counts": {"train_pages": len(self.train_pages), "test_pages": len(self.test_pages),
                       "train_persons": len(self.train_persons),
                       "test_persons": len(self.test_persons)},
            "pages": {"train": sorted(self.train_pages), "test": sorted(self.test_pages)},
            "persons": {"train": sorted(self.train_persons), "test": sorted(self.test_persons)},
        }


RULES = {
    "page": "page goes to test when sha256(seed, page) / 2^64 < test_share; every person on "
            "a test page is a test person and appears in no training pair",
    "person": "person is a test person when sha256(seed, person) / 2^64 < test_share; a page "
              "is a test page when everyone on it is a test person (a page with nobody on "
              "it: when sha256(seed, page) / 2^64 < test_share); a test person appears in "
              "no training pair",
}


def assign(page_persons: Mapping[str, set[str]], *, seed: int = DEFAULT_SEED,
           share: float = DEFAULT_TEST_SHARE, mode: str = DEFAULT_MODE) -> Split:
    if not 0.0 <= share < 1.0:
        raise ValueError("test share must be in [0, 1)")
    if mode not in MODES:
        raise ValueError(f"split mode must be one of {', '.join(MODES)}")
    if mode == "page":
        test = {p for p in page_persons if page_fraction(seed, p) < share}
    else:
        chosen = {x for people in page_persons.values() for x in people
                  if page_fraction(seed, f"person:{x}") < share}
        test = {p for p, people in page_persons.items()
                if (people and people <= chosen)
                or (not people and page_fraction(seed, p) < share)}
    train = set(page_persons) - test
    test_persons = {x for p in test for x in page_persons[p]}
    if mode == "person":
        test_persons |= chosen
    train_persons = {x for p in train for x in page_persons[p]} - test_persons
    return Split(mode, seed, share, frozenset(train), frozenset(test), frozenset(test_persons),
                 frozenset(train_persons))


def clean_for_train(split: Split, page: str, persons: Iterable[str]) -> bool:
    """Whether a unit (its page and persons) may appear in a training pair."""
    return page not in split.test_pages and split.test_persons.isdisjoint(persons)


def check(split: Split, pairs: Iterable[dict], units: Mapping[str, dict]) -> None:
    """Fail if a training pair, or any unit it references, involves a test page or person."""
    for pair in pairs:
        where = f"pair {pair['query']!r} ({pair['family']})"
        if pair["page"] in split.test_pages:
            raise LeakError(f"{where}: page {pair['page']!r} is a test page")
        leaked = split.test_persons.intersection(pair["persons"])
        if leaked:
            raise LeakError(f"{where}: test person(s) {sorted(leaked)} in a training pair")
        for uid in [pair["positive"], *pair["negatives"]]:
            unit = units.get(uid)
            if unit is None:
                raise LeakError(f"{where}: unit {uid} missing from the units file")
            if unit["page"] in split.test_pages:
                raise LeakError(f"{where}: unit {uid} is on test page {unit['page']!r}")
            leaked = split.test_persons.intersection(unit["persons"])
            if leaked:
                raise LeakError(f"{where}: unit {uid} involves test person(s) {sorted(leaked)}")
