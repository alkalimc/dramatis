"""Operator pages → one dossier record each.

Two sources, deliberately: attribute fields come from the site's structured tables
rather than from parsing the infobox, and narrative text comes from parsing the
page. Structured tables are already normalised and already correct; re-deriving the
same numbers from markup adds a way to be wrong for no gain.

The selection rule throughout is that gameplay is not narrative. Unlock conditions,
material costs, skill values and promotion requirements are all real information
about a game and none of it is something a character knows about herself.
"""

from __future__ import annotations

import re
from collections.abc import Iterator

import mwparserfromhell as mw

from dramatis_forge.normalize.records import Dossier, Record
from dramatis_forge.normalize.wikitext import template_name
from dramatis_forge.pack import PageContext

from ..fields import FIELD_NAMES

# `档案N文本` is body. `档案N条件` is the unlock requirement — gameplay, dropped.
RE_SECTION_FIELD = re.compile(r"档案(\d+)(文本|条件)?$")

#: Fields on the related-items template that carry an authorial voice. `用途`
#: describes what an item does mechanically and is dropped.
ITEM_FIELDS = (
    "干员简介", "干员简介补充", "信物描述", "信件描述", "文件夹描述", "信赖物品描述",
)
#: The only narrative fields in the infobox: outfit and promotion blurbs are short
#: pieces of characterful prose.
RE_INFOBOX_TEXT = re.compile(r"^(精英\d介绍|时装\d介绍)$")

T_SECTIONS = "人员档案"
T_ITEMS = "相关道具"
T_INFOBOX = "CharinfoV2"


def parse_dossier(ctx: PageContext) -> Iterator[Record]:
    warnings: list[str] = []
    sections: list[dict[str, str]] = []
    items: dict[str, object] = {}

    for tm in mw.parse(ctx.wikitext).filter_templates(recursive=True):
        name = template_name(tm)
        if name == T_SECTIONS:
            grouped: dict[str, dict[str, str]] = {}
            for p in tm.params:
                m = RE_SECTION_FIELD.match(str(p.name).strip())
                if m:
                    grouped.setdefault(m.group(1), {})[m.group(2) or "标题"] = str(p.value)
            for idx in sorted(grouped, key=int):
                got = grouped[idx]
                text = ctx.clean.text(got.get("文本", ""), warnings)
                if text:
                    sections.append(
                        {"title": ctx.clean.text(got.get("标题", "")), "text": text})
        elif name == T_ITEMS:
            for p in tm.params:
                key = str(p.name).strip()
                if key in ITEM_FIELDS:
                    text = ctx.clean.text(str(p.value), warnings)
                    if text:
                        items[key] = text
        elif name == T_INFOBOX:
            for p in tm.params:
                key = str(p.name).strip()
                if RE_INFOBOX_TEXT.match(key):
                    text = ctx.clean.text(str(p.value), warnings)
                    if text:
                        items[key] = text

    for w in warnings:
        ctx.warn(w)

    raw = {**ctx.row("chara"), **ctx.row("chara_extra_info")}
    raw.pop("page", None)
    fields = {FIELD_NAMES.get(k, k): v for k, v in raw.items() if v}

    # Record-set titles and their prologues: extra tone material, and the only
    # place a character's own record collection is introduced in prose.
    records = [
        {"set": m.get("storySetName", ""), "intro": m.get("storyIntro", "")}
        for m in sorted(ctx.rows("char_memory"), key=lambda x: str(x.get("storyIndex", "")))
        if m.get("storyIntro")
    ]
    if records:
        items["密录"] = records

    if not sections and not items and not fields:
        ctx.empty("no dossier and no attributes — support unit or reserve operator")
        return

    if not sections and not items:
        # Attributes without any prose. Real for roughly two dozen reserve operators
        # and guard-protocol units: they exist on the roster and have nothing to say
        # about themselves, which is a fact about the source, not a parse failure.
        ctx.warn("attributes only, no narrative sections (reserve / support unit)")

    yield Dossier(
        page=ctx.title,
        fields=fields,
        sections=tuple(sections),
        items=items,
        revid=ctx.revid,
    )
