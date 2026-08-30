"""Voice-record pages → one record per line, with its trigger.

Only the Chinese text is taken, out of eleven or more language columns. The others
are translations of the same performance: they multiply the corpus without adding
information, and mixing languages into one vector space costs retrieval quality on
both sides.

The trigger type is kept and matters more than it looks. A line tagged as said on
being assigned to office duty, on being touched, on a mission failing, tells the
persona generator *under what circumstance* the character speaks that way. Without
it the same lines are a bag of quotations.
"""

from __future__ import annotations

import re
from collections.abc import Iterator

import mwparserfromhell as mw

from dramatis_forge.normalize.records import Record, Voice
from dramatis_forge.normalize.wikitext import template_name
from dramatis_forge.pack import PageContext

T_TABLE = "VoiceTable"
RE_FIELD = re.compile(r"(标题|台词|触发类型|条件)(\d+)$")
# `{{VoiceData/word|中文|…}}` runs until the next language block or the field end.
RE_CHINESE = re.compile(
    r"\{\{VoiceData/word\|中文\|(.*?)\}\}(?=\s*\{\{VoiceData/word|\s*$)", re.S
)
SUFFIX = "/语音记录"


def subject_of(title: str) -> str:
    """`凯尔希/语音记录` → `凯尔希`."""
    return title.split("/", 1)[0]


def parse_voices(ctx: PageContext) -> Iterator[Record]:
    if T_TABLE not in ctx.wikitext:
        # The seed set is defined by title suffix *and* a local table. A page with
        # the suffix but no table is a redirect or a development stub — the table it
        # appeared to have came from transclusion, not from its own text.
        ctx.empty("no local voice table (redirect or development stub)")
        return

    warnings: list[str] = []
    subject = subject_of(ctx.title)
    found = 0

    for tm in mw.parse(ctx.wikitext).filter_templates(recursive=False):
        if template_name(tm) != T_TABLE:
            continue
        grouped: dict[str, dict[str, str]] = {}
        for p in tm.params:
            m = RE_FIELD.match(str(p.name).strip())
            if m:
                grouped.setdefault(m.group(2), {})[m.group(1)] = str(p.value)
        for idx in sorted(grouped, key=int):
            got = grouped[idx]
            m = RE_CHINESE.search(got.get("台词", ""))
            if not m:
                continue
            text = ctx.clean.text(m.group(1), warnings)
            if not text:
                continue
            found += 1
            yield Voice(
                page=ctx.title,
                subject=subject,
                idx=int(idx),
                text=text,
                title=ctx.clean.text(got.get("标题", "")),
                trigger=got.get("触发类型", "").strip(),
                unlock=ctx.clean.text(got.get("条件", "")),
            )

    for w in warnings:
        ctx.warn(w)
    if not found:
        ctx.warn("voice table present but no Chinese lines extracted", high=True)
