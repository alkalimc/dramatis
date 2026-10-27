"""Fixed-window units: the control for "does the shape of a unit matter".

Construction (deterministic): units are grouped by page in file order, their texts are joined
with a newline into one page text, and that text is cut into consecutive windows of `size`
characters (the last one may be shorter). Headers and templates are discarded on purpose: a
window knows nothing about the typed structure it was cut from. A window carries the page and
the union of the persons of the units it overlaps, so the leak rules apply unchanged.

A typed unit id maps to the windows that overlap its character span, ordered by overlap size.
That mapping lets the same pairs and the same held-out queries be replayed on windows: a
window is relevant when it overlaps a relevant unit.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass, field

from .data import Unit, units_from_rows
from .metrics import Query


@dataclass
class Windowing:
    size: int
    windows: dict[str, Unit]
    # typed unit id -> overlapping window ids, largest overlap first
    spans: dict[str, list[str]] = field(default_factory=dict)

    def overlapping(self, unit_id: str) -> list[str]:
        return self.spans.get(unit_id, [])

    def best(self, unit_id: str) -> str:
        return self.spans[unit_id][0]

    def map_negatives(self, unit_ids: Iterable[str], avoid: set[str]) -> tuple[str, ...]:
        """Best window of each negative unit, dropping duplicates and anything in `avoid`."""
        out: list[str] = []
        for uid in unit_ids:
            for wid in self.spans.get(uid, [])[:1]:
                if wid not in avoid and wid not in out:
                    out.append(wid)
        return tuple(out)

    def remap_query(self, q: Query) -> Query | None:
        """The same query with gold and negatives expressed as windows (None if no gold
        unit survives the mapping)."""
        gold: dict[str, int] = {}
        for uid, grade in q.gold.items():
            for wid in self.overlapping(uid):
                gold[wid] = max(gold.get(wid, 0), grade)
        if not gold:
            return None
        negs: list[str] = []
        for uid in q.hard_negatives:
            for wid in self.overlapping(uid):
                if wid not in gold and wid not in negs:
                    negs.append(wid)
        return Query(q.qid, q.family, q.text, gold, negs, q.stratum)


def build_windows(units: dict[str, Unit], size: int) -> Windowing:
    if size <= 0:
        raise ValueError("window size must be positive")
    by_page: dict[str, list[Unit]] = {}
    for u in units.values():
        if u.text:
            by_page.setdefault(u.page, []).append(u)

    windows: dict[str, Unit] = {}
    spans: dict[str, list[str]] = {}
    for page_no, (page, members) in enumerate(by_page.items()):
        pieces = []
        offset = 0
        extents = []  # (unit, start, end) in page-text coordinates
        for u in members:
            extents.append((u, offset, offset + len(u.text)))
            pieces.append(u.text)
            offset += len(u.text) + 1  # the joining newline
        text = "\n".join(pieces)

        count = max(1, -(-len(text) // size))
        overlap: dict[str, list[tuple[int, str]]] = {u.id: [] for u in members}
        for i in range(count):
            lo, hi = i * size, min((i + 1) * size, len(text))
            wid = f"win{size}:{page_no}:{i}"
            persons: list[str] = []
            for u, start, end in extents:
                shared = min(hi, end) - max(lo, start)
                if shared > 0:
                    overlap[u.id].append((shared, wid))
                    persons += [p for p in u.persons if p not in persons]
            windows[wid] = Unit(wid, "", text[lo:hi], "window", page, tuple(persons))
        for uid, found in overlap.items():
            # Largest overlap first; ties resolve to the earlier window.
            found.sort(key=lambda t: (-t[0], t[1]))
            spans[uid] = [w for _, w in found]
    return Windowing(size, windows, spans)


def write_windows(windowing: Windowing, path) -> int:
    """Write the window file in the same row format as `units.jsonl`."""
    import json

    with open(path, "w", encoding="utf-8") as fh:
        for u in windowing.windows.values():
            row = {
                "id": u.id,
                "header": u.header,
                "text": u.text,
                "template": u.template,
                "page": u.page,
                "persons": list(u.persons),
            }
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")
    return len(windowing.windows)


def from_rows(rows: Iterable[dict], size: int) -> Windowing:
    return build_windows(units_from_rows(rows), size)
