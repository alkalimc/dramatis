"""Which operator pages are the same person.

The site has two mechanisms and both are *declared*, which is the only kind of
signal allowed here:

    ① alter    page contains {{异格干员|原型=X}}, X ≠ this page, X in roster
    ② variant  page contains {{异格干员/升变}} AND is titled `X(…)` with X in roster
    ③ else     the page is its own person

Only the alter side needs reading. The prototype page carries a self-referential
`{{异格干员|原型={{BASEPAGENAME}}|非异格=1}}`, but "is this page an alter" is
answerable and "does this page have an alter" adds nothing.

Two measured facts that constrain the implementation:

**No alter page links to its prototype in body text — zero out of thirty-eight.**
The relation exists solely as a template parameter. An implementation that scans
wikilinks finds nothing and reports success.

**Name heuristics must be excluded outright.** Real counterexamples: 推进之王 /
维娜·维多利亚 share no substring; 傀影 / 酒神 likewise; 凯尔希 / 凯尔希·思衡托 share a
prefix by coincidence; W / 维什戴尔 the same. Prefix, suffix or edit-distance
pairing would both miss and misfire, and a misfire is undiscoverable.

**Rule ② needs the template as a gate.** 阿米娅, 阿米娅(医疗) and 阿米娅(近卫) are
multi-class forms, not alters: none carries `异格干员`, all three carry
`异格干员/升变`. That template takes no parameters — it declares membership in a
variant group without naming the group, so the group comes from the title. Title
shape alone would be fragile (parentheses are a general disambiguation device);
requiring both the template and a resolvable stripped title makes it exact. The
intersection is precisely 阿米娅's two extra pages, and 阿米娅 itself has no
parenthetical, so it falls through to ③ and becomes the person.
"""

from __future__ import annotations

import re
from collections.abc import Mapping

import mwparserfromhell as mw

from dramatis_forge.identity import CANONICAL
from dramatis_forge.normalize.wikitext import template_name
from dramatis_forge.pack import IdentityRules

ALTER = "alter"
VARIANT = "variant"

T_ALTER = "异格干员"
T_VARIANT = "异格干员/升变"
RE_PARENTHETICAL = re.compile(r"^(.+?)[(（][^()（）]*[)）]$")

#: Measured on a full run of all 456 operator pages.
BASELINES = {ALTER: 38, VARIANT: 2, "persons": 416}


def _declared_prototype(wikitext: str) -> str | None:
    for tm in mw.parse(wikitext).filter_templates(recursive=True):
        if template_name(tm) != T_ALTER:
            continue
        if not tm.has("原型"):
            continue
        proto = str(tm.get("原型").value).strip()
        # `{{BASEPAGENAME}}` is the self-referential marker on prototype pages; it
        # is not a title and must not be treated as one.
        if not proto or "{{" in proto:
            continue
        return proto
    return None


def _has_variant_marker(wikitext: str) -> bool:
    return any(
        template_name(tm) == T_VARIANT
        for tm in mw.parse(wikitext).filter_templates(recursive=True)
    )


def resolve(
    pages: Mapping[str, str], roster: frozenset[str]
) -> Mapping[str, tuple[str, str]]:
    out: dict[str, tuple[str, str]] = {}
    for title in sorted(roster):
        text = pages.get(title) or ""
        if not text:
            continue

        proto = _declared_prototype(text)
        if proto and proto != title and proto in roster:
            out[title] = (proto, ALTER)
            continue

        if _has_variant_marker(text):
            m = RE_PARENTHETICAL.match(title)
            if m and m.group(1) in roster:
                out[title] = (m.group(1), VARIANT)
                continue

        out[title] = (title, CANONICAL)
    return out


RULES = IdentityRules(
    resolve=resolve,
    #: Canonical first, then alters, then variants: the roster entry expands in the
    #: order a reader expects, and the persona generator sees "here is who she is,
    #: then here is who she became".
    form_order={CANONICAL: 0, ALTER: 10, VARIANT: 20},
    baselines=BASELINES,
)


def variant_marker_pages(pages: Mapping[str, str]) -> list[str]:
    """Every page carrying the variant marker, for the novelty tripwire.

    Measured baseline is three (阿米娅 plus her two class pages). A fourth means a
    new grouping shipped, and a human should decide what it means before it
    reshapes the roster on its own.
    """
    return sorted(t for t, text in pages.items() if text and _has_variant_marker(text))
