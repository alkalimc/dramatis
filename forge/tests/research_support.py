"""A synthetic folio for the research tests: invented people, pages and aliases.

Written directly (not built from a toy wiki) so each test can state exactly which units
and aliases exist. N people each have a card, a voice page and (even ones) a form page;
stories hold scenes of two speakers; lore pages hold sections.
"""

from __future__ import annotations

import json
from pathlib import Path

from dramatis_forge import segment
from dramatis_forge.folio import Folio

N = 24
SHAPES = {"profile": "profile", "voice": "voice", "dialogue": "dialogue", "lore": "lore"}


def pid(i: int) -> str:
    return f"P{i:02d}"


def build(path: Path, *, extra_page: bool = False) -> Path:
    rows: list[tuple] = []
    links: list[tuple[str, str]] = []
    ordn = 0

    def unit(uid, template, page, title, header, text, span=None):
        nonlocal ordn
        rows.append((uid, ordn, template, page, 1, title, header, text, len(text),
                     span or page, None, None))
        ordn += 1

    persons = []
    for i in range(N):
        p = pid(i)
        unit(f"card:{p}", "profile", p, f"{p} card", f"[card] {p}", f"Name: {p}\nGroup: g{i % 3}",
             f"{p}#card")
        links.append((f"card:{p}", p))
        for j, t in enumerate(("greeting", "farewell", "idle")):
            unit(f"voice:{p}:{j}", "voice", f"{p}/voice", t, f"[voice] {p} {t}",
                 f"{p} says line {j} aloud", f"{p}#voice")
            links.append((f"voice:{p}:{j}", p))
        forms = [{"page": p, "kind": "canonical"}]
        if i % 2 == 0:
            forms.append({"page": f"{p} alt", "kind": "alter"})
        persons.append((p, p, f"Name{p}", json.dumps(forms),
                        json.dumps({"group": f"g{i % 3}", "kind": "k"}), 100, 0.5, None))
    for s in range(12):
        for k in range(2):
            uid = f"dlg:{s}:{k}"
            unit(uid, "dialogue", f"Story {s}", f"Story {s} scene {k}", f"[story] {s} {k}",
                 f"talk {s} {k}", f"Story {s}#{k}")
            for who in (pid((s * 2 + k) % N), pid((s * 2 + k + 1) % N)):
                links.append((uid, who))
    for k in range(8):
        for j in range(4):
            unit(f"lore:{k}:{j}", "lore", f"Lore{k}", f"Sec {j}", f"[lore] Lore{k} Sec {j}",
                 f"text of section {j} on lore {k}", f"Lore{k}#{j}")
    if extra_page:
        unit("lore:x:0", "lore", "Extra", "Only", "[lore] Extra", "extra", "Extra#0")

    aliases = [(f"nick{i:02d}", pid(i), "redirect") for i in range(N)]
    aliases += [("Twin", pid(1), "realname"), ("Twin", pid(2), "realname"),
                ("Mark", pid(3), "disambig"), ("Mark", pid(4), "disambig"),
                ("Mark", pid(5), "disambig"), ("Solo", pid(7), "disambig"),
                ("P06 style", pid(6), "form")]
    with Folio.create(path) as f:
        f.add_chunks(rows)
        f.add_unit_persons(links)
        f.write_persons(persons)
        f.write_aliases(aliases)
        seg = segment.load("bigram")
        f.add_fts([(r[1], seg(f"{r[6]}\n{r[7]}")) for r in rows])
        f.set_meta("segmenter", "char-bigram/1")
        f.set_meta("stopwords", [])
        f.set_meta("template_shapes", SHAPES)
        f.set_meta("build_fingerprint", "sha256:toy")
        f.set_meta("chunk_count", len(rows))
    return path
