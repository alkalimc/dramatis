"""The in-world encyclopaedia → sections of prose.

Every page in the namespace is taken; the *page* is never the unit of exclusion.
Selection happens inside the page, because a single page mixes world content with
real-world commentary and the two are separated by structure the site already
provides.

The earlier rule was "if the page contains the commentary box, drop the page". On a
full run that discarded the three highest-value pages in the namespace: an 18 KB
treatment of the world's races, a 59 KB register of named characters, and the
concept encyclopaedia. Dropping a container because of one of its parts is the
mistake this parser exists to not repeat.

Two paths, because the site uses two layouts:

  ① tabbed pages carry `<div id="情报">` and `<div id="考据">` side by side. Take
     the first, discard the second entirely — the second is about the real world.
  ② untabbed pages keep their content inside `{{X考据}}` templates, where in-world
     and real-world material are *parameters of the same template*, so the split is
     by parameter name.

The criterion is never "is this text good". It is **"is this true inside the
world"**. A well-sourced note on the real-world origin of a design is excellent
writing that a character must not have read.
"""

from __future__ import annotations

from collections.abc import Iterator

import mwparserfromhell as mw

from dramatis_forge.normalize.records import Lore, Record
from dramatis_forge.normalize.wikitext import tabs, template_name
from dramatis_forge.pack import PageContext

#: Parameter names that hold in-world content. Enumerated from a full pass over the
#: namespace, not guessed: the same templates also carry 考据 / 出处 / 英文名 /
#: 日文名 / Logo / 图片 and layout keys, all of which are dropped.
WORLD_PARAMS = (
    "情报", "简介", "角色经历", "基本信息", "描述", "背景", "成员", "主要种族", "亚种",
    "角色", "干员",
)
#: Parameters whose text stands alone; the rest get their key as a label, because
#: `亚种：…` reads as a fact and a bare fragment does not.
UNLABELLED = frozenset({"情报", "简介", "角色经历", "描述"})

LORE_TEMPLATES = frozenset({
    "种族考据", "角色考据", "组织考据", "地理考据", "百科词条",
    "道具考据", "事件考据", "收藏品考据", "家具考据", "角色资料",
})
TITLE_PARAMS = ("种族", "姓名", "词条名")

WORLD_TAB = "情报"
MIN_TEMPLATE_CHARS = 20
MIN_PARAM_CHARS = 4


def parse_lore(ctx: PageContext) -> Iterator[Record]:
    if ctx.clean.is_meta_page(ctx.title):
        ctx.empty("wiki meta-space: editing guidance, not world content")
        return

    warnings: list[str] = []
    found: list[dict] = []
    panes = tabs(ctx.wikitext)

    if panes:
        body = panes.get(WORLD_TAB)
        if body is None:
            ctx.empty("tabbed page with no in-world pane (real-world commentary only)")
            return
        found += ctx.clean.split_sections(body, warnings)
        found += _from_templates(ctx, body, warnings)
    else:
        found += _from_templates(ctx, ctx.wikitext, warnings)

    for w in warnings:
        ctx.warn(w)

    seen: set[tuple] = set()
    emitted = 0
    for row in found:
        key = (tuple(row["path"]), row["text"][:80])
        if key in seen:
            continue
        seen.add(key)
        emitted += 1
        yield Lore(
            page=ctx.title, path=tuple(row["path"]), text=row["text"], revid=ctx.revid)

    if not emitted:
        ctx.empty("no in-world content (commentary page, navigation page or stub)")


def _from_templates(ctx: PageContext, body: str, warnings: list[str]) -> list[dict]:
    """Read in-world parameters out of the commentary templates.

    A tabbed page's in-world pane also embeds these, so both paths run it. The
    heading stack supplies the path, which is what gives a leaf entry its context:
    a race entry filed under "human races" means something the same words filed
    elsewhere would not.
    """
    out: list[dict] = []
    if not body:
        return out

    for tm in mw.parse(body).filter_templates(recursive=True):
        name = template_name(tm)
        if name not in LORE_TEMPLATES:
            continue

        title = ""
        for key in TITLE_PARAMS:
            if tm.has(key):
                title = ctx.clean.text(str(tm.get(key).value))
                break
        if not title:
            positional = [str(p.value).strip() for p in tm.params if not p.showkey]
            title = ctx.clean.text(positional[0]) if positional else ""

        parts: list[str] = []
        for key in WORLD_PARAMS:
            if not tm.has(key):
                continue
            text = ctx.clean.text(str(tm.get(key).value), warnings)
            if text and len(text) >= MIN_PARAM_CHARS:
                parts.append(text if key in UNLABELLED else f"{key}：{text}")
        if not parts:
            continue

        text = "\n".join(parts)
        if len(text) < MIN_TEMPLATE_CHARS:
            continue
        # Prepend the title only when the body does not already open with it, so we
        # do not produce `阿戈尔：阿戈尔涉足的地区……`.
        if title and not text.startswith(title):
            text = f"{title}：{text}"
        out.append({"path": (name, title) if title else (name,), "text": text})
    return out
