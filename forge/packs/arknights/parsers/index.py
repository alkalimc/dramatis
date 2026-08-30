"""The hand-listed index pages, one reader each.

These are nine pages that between them hold some of the densest material in the
corpus: in-world correspondence, a glossary, a register of non-roster characters,
per-mission synopses, world-building tips, and a code-name-to-real-name table.

There is no generic reader for them and there should not be. They are nine
different hand-built layouts, each expressing a different thing; a "table reader"
general enough to cover all nine would be configured per page anyway, at which
point the configuration is the parser with extra steps. Nine named functions are
honest about that, and each one can carry the note explaining what its page does.
"""

from __future__ import annotations

import re
from collections.abc import Iterator

from dramatis_forge.normalize.records import Alias, CharRef, Letter, Lore, Record, Term
from dramatis_forge.normalize.wikitext import (
    RE_SECTION,
    find_header,
    iter_tables,
    table_rows,
    top_sections,
)
from dramatis_forge.pack import PageContext

import mwparserfromhell as mw

from dramatis_forge.normalize.wikitext import template_name


# --------------------------------------------------------------------------- #
# in-world correspondence
# --------------------------------------------------------------------------- #

T_LETTER = "邮件"


def parse_letters(ctx: PageContext) -> Iterator[Record]:
    """Letters written to the player character.

    First-person prose by a named character with no narrator in between — the purest
    voice sample in the corpus, and the reason this page is worth a parser of its own
    for a hundred-odd records.
    """
    warnings: list[str] = []
    count = 0
    for tm in mw.parse(ctx.wikitext).filter_templates(recursive=True):
        if template_name(tm) != T_LETTER:
            continue
        got = {str(p.name).strip(): str(p.value) for p in tm.params}
        body = ctx.clean.text(got.get("内容", ""), warnings)
        if not body:
            continue
        count += 1
        yield Letter(
            page=ctx.title,
            body=body,
            sender=ctx.clean.text(got.get("来自", "")),
            date=got.get("日期", "").strip(),
            title=ctx.clean.text(got.get("标题", "")),
        )
    for w in warnings:
        ctx.warn(w)
    if not count:
        ctx.warn("mail page produced no letters", high=True)


# --------------------------------------------------------------------------- #
# world-building tips
# --------------------------------------------------------------------------- #

RE_ROWSPAN_CATEGORY = re.compile(r'^!\s*rowspan="?(\d+)"?[^|]*\|\s*(.+)$', re.M)
KEEP_CATEGORY = "背景"
MIN_TIP_CHARS = 15
MIN_WORLDVIEW_CHARS = 20


def parse_tips(ctx: PageContext) -> Iterator[Record]:
    """Loading-screen tips: keep the world-building, drop the game manual.

    The whole `世界观` section is world-building. Inside `通常` only the `背景`
    category is; combat, recruitment and base-management tips are instructions to a
    player. The category is not a column — it is written as `! rowspan=N | category`,
    so the reader has to count rows down from the marker.
    """
    for name, body in top_sections(ctx.wikitext).items():
        if name not in ("通常", "世界观"):
            continue
        for table in iter_tables(body):
            if name == "世界观":
                for cells in table_rows(table):
                    if len(cells) < 2:
                        continue
                    subject = ctx.clean.text(cells[0])
                    desc = ctx.clean.text(cells[1])
                    if desc and len(desc) >= MIN_WORLDVIEW_CHARS:
                        yield Lore(
                            page=ctx.title, path=("世界观", subject),
                            text=f"{subject}：{desc}", revid=ctx.revid)
                continue

            category: str | None = None
            remaining = 0
            for line in table.splitlines():
                stripped = line.strip()
                m = RE_ROWSPAN_CATEGORY.match(stripped)
                if m:
                    category, remaining = m.group(2).strip(), int(m.group(1))
                    continue
                if (
                    category == KEEP_CATEGORY
                    and remaining > 0
                    and stripped.startswith("|")
                    and not stripped.startswith("|-")
                ):
                    text = ctx.clean.text(stripped[1:])
                    if len(text) >= MIN_TIP_CHARS:
                        yield Lore(
                            page=ctx.title, path=("贴士", KEEP_CATEGORY),
                            text=text, revid=ctx.revid)
                        remaining -= 1


# --------------------------------------------------------------------------- #
# glossary
# --------------------------------------------------------------------------- #

HEADER_WORDS = frozenset({"中文", "鹰角文本", "英文", "日文", "其他"})
#: A cell still carrying any of these did not parse cleanly: `?` is an unexpanded query
#: placeholder, `=` a surviving parameter assignment, `{`/`}`/`|` raw markup. A glossary
#: entry that is partly markup is worse than a missing one, because it will be matched
#: against a user's query and returned as if it meant something.
_RESIDUE = re.compile(r"[?={}|<>]")


def _term_variants(text: str) -> list[str]:
    """One cell can stack a code name over its translation on separate lines.

    The wiki writes `Ace` above `王牌` in a single cell to show that both name the same
    thing. Measured: seven such entries. Keeping the newline produces one unsearchable
    two-line token; splitting yields two aliases for one concept, which is what the
    editor meant and what query normalisation needs.
    """
    return [part.strip() for part in text.splitlines() if part.strip()]


def parse_glossary(ctx: PageContext) -> Iterator[Record]:
    """Term table: the developer's own Chinese / English / other columns.

    The localisation columns are dropped: two renderings of one term are not two
    terms, and query normalisation only needs the canonical set.
    """
    for section, body in top_sections(ctx.wikitext).items():
        if section == "注释":
            continue
        for table in iter_tables(body):
            for cells in table_rows(table):
                if len(cells) < 2:
                    continue
                zh = ctx.clean.text(cells[0])
                en = ctx.clean.text(cells[1]) if len(cells) > 1 else ""
                other = ctx.clean.text(cells[2]) if len(cells) > 2 else ""
                if not zh or zh in HEADER_WORDS:
                    continue
                if _RESIDUE.search(zh) or _RESIDUE.search(other):
                    ctx.warn(f"glossary row with unparsed markup, dropped: {zh[:40]!r}")
                    continue
                for variant in _term_variants(zh):
                    if variant in HEADER_WORDS:
                        continue
                    yield Term(
                        page=ctx.title, zh=variant,
                        en="" if en == "-" else en,
                        other="" if other == "-" else other,
                        category=section,
                    )


# --------------------------------------------------------------------------- #
# code name ↔ real name
# --------------------------------------------------------------------------- #

RE_POPUP_PARTS = re.compile(r"(?:图标|内容)\s*=\s*(.*?)(?=\||\}\}|$)", re.S)
RE_MDI_ARROW = re.compile(r"\{\{\s*mdi\s*\|\s*arrow-\w+\s*\}\}", re.I)
NAME_STRIP = " ？?“”\"'()（）"
MAX_NAME_CHARS = 40


def parse_real_names(ctx: PageContext) -> Iterator[Record]:
    """Real names, into the alias dictionary rather than the corpus.

    Column position cannot be assumed. The table is `portrait | code name | real
    name | source`, and taking column 0 as the code name yields four rows out of two
    hundred. Locating columns by header text is the only stable approach, and the
    header itself is not the first row — a merged title row precedes it.
    """
    pairs = 0
    for table in iter_tables(ctx.wikitext):
        rows = table_rows(table)
        if not rows:
            continue
        found = find_header(table, "代号", "真名")
        if found is None:
            continue
        header, header_seg = found
        code_col = next(i for i, h in enumerate(header) if "代号" in h)
        name_col = next(i for i, h in enumerate(header) if "真名" in h)
        # `table_rows` starts after the first `|-`, so header segment i is rows[i-1]
        # and data begins at rows[header_seg].
        for cells in rows[max(header_seg, 0):]:
            if max(code_col, name_col) >= len(cells):
                continue
            code = ctx.clean.text(cells[code_col])
            if not code:
                continue
            for name in _name_variants(ctx, cells[name_col]):
                if name != code and 1 < len(name) <= MAX_NAME_CHARS:
                    pairs += 1
                    yield Alias(alias=name, target=code, kind="realname")
    if not pairs:
        ctx.warn("real-name table produced no pairs", high=True)


def _name_variants(ctx: PageContext, cell: str) -> list[str]:
    """One cell can hold a Chinese and a foreign rendering, and a rename sequence.

    Two escapes must be undone before anything else or the output is garbage:

    * `{{=}}` is a literal `=`. Left alone, `<span class{{=}}"langs">` parses as a
      parameter separator and the markup leaks into the name.
    * `{{mdi|arrow-right}}` renders as an arrow and *means* "renamed to". It has to
      become a separator, not text, or you get one name reading
      `佐原田金兵卫arrow-right三船光平` instead of two.
    """
    cell = cell.replace("{{=}}", "=")
    cell = RE_MDI_ARROW.sub("→", cell)
    raw = RE_POPUP_PARTS.findall(cell) or [cell]
    out: list[str] = []
    for part in raw:
        text = ctx.clean.text(part)
        for name in re.split(r"[→、/;；\n]", text):
            name = name.strip(NAME_STRIP)
            # Residual markup means the cell did not parse cleanly. An alias that is
            # partly markup is worse than a missing alias, so drop it.
            if name and not re.search(r"[<>{}|=]", name):
                out.append(name)
    return out


# --------------------------------------------------------------------------- #
# non-roster characters
# --------------------------------------------------------------------------- #

HEADER_CELLS = frozenset({"名称/代号", "简介"})
MIN_DESC_CHARS = 6


def parse_char_refs(ctx: PageContext) -> Iterator[Record]:
    """Hand-written descriptions of characters with no page of their own.

    A hundred-odd tables grouped by story arc. This is the only coverage for
    everyone outside the playable roster, and because a human wrote each line it is
    also the natural few-shot source and hold-out reference for persona synthesis.

    The length floor exists to skip blank rows and headers, not short entries: "海伦：
    the Patriot's wife, deceased." is eleven characters and complete. Sixteen such
    rows were measured; a higher floor silently deletes them.
    """
    marks = list(RE_SECTION.finditer(ctx.wikitext))
    count = 0
    for i, m in enumerate(marks):
        end = marks[i + 1].start() if i + 1 < len(marks) else len(ctx.wikitext)
        group = m.group(2).strip()
        for table in iter_tables(ctx.wikitext[m.end(): end]):
            for cells in table_rows(table):
                if len(cells) < 2:
                    continue
                name = ctx.clean.text(cells[0])
                desc = ctx.clean.text(cells[1])
                if not name or name in HEADER_CELLS or len(desc) < MIN_DESC_CHARS:
                    continue
                count += 1
                yield CharRef(
                    page=ctx.title, name=name, description=desc, story_group=group,
                    source=ctx.clean.text(cells[2]) if len(cells) > 2 else "",
                )
    if not count:
        ctx.warn("character register produced no entries", high=True)


# --------------------------------------------------------------------------- #
# mission synopses
# --------------------------------------------------------------------------- #

RE_STYLE_WIDGET = re.compile(r"\{\{#Widget:style\|.*?\}\}", re.S | re.I)
RE_TEMPLATESTYLES = re.compile(r"<templatestyles[^>]*/>")
RE_BIG_TITLE = re.compile(r"<big><big>(.*?)</big></big>", re.S)
RE_SYNOPSIS = re.compile(r"\{\{剧情简介\|([^|]*)\|([^|]*)\|([^|]*)\|(.*?)\}\}", re.S)
MIN_SYNOPSIS_CHARS = 15


def _strip_styles(wikitext: str) -> str:
    """Remove the leading CSS and widget block. One page carries about 8 KB of it."""
    return RE_TEMPLATESTYLES.sub("", RE_STYLE_WIDGET.sub("", wikitext))


def parse_synopses(ctx: PageContext) -> Iterator[Record]:
    """Per-mission synopses: a thousand short, self-contained summaries.

    The generic prose path cannot read this page — the whole thing is one table, so
    section splitting yields three sections and loses every synopsis. Each synopsis
    becomes its own retrieval unit because each one already *is* a complete summary,
    which is exactly the shape a retrieval unit wants.
    """
    body = _strip_styles(ctx.wikitext)
    # Activity names appear as oversized table captions; their positions partition
    # the page, so a synopsis belongs to the last caption above it.
    anchors = [(m.start(), ctx.clean.text(m.group(1))) for m in RE_BIG_TITLE.finditer(body)]
    warnings: list[str] = []
    count = 0

    for m in RE_SYNOPSIS.finditer(body):
        stage, name, phase = (ctx.clean.text(x) for x in m.groups()[:3])
        text = ctx.clean.text(m.group(4), warnings)
        if len(text) < MIN_SYNOPSIS_CHARS:
            continue
        activity = ""
        for pos, title in anchors:
            if pos >= m.start():
                break
            activity = title
        label = " ".join(x for x in (stage, name, phase) if x)
        count += 1
        # The page title is not part of the path: the header already carries it, and
        # including it produced `情报处理室 › 情报处理室 › …` on all 992 units.
        yield Lore(
            page=ctx.title,
            path=tuple(p for p in (activity, label) if p),
            text=f"{label}：{text}" if label else text,
            revid=ctx.revid,
        )
    for w in warnings:
        ctx.warn(w)
    if not count:
        ctx.warn("synopsis page produced no synopses", high=True)


# --------------------------------------------------------------------------- #
# generic prose index page
# --------------------------------------------------------------------------- #


def parse_prose(ctx: PageContext) -> Iterator[Record]:
    """Section-split prose, for index pages that really are just prose."""
    warnings: list[str] = []
    sections = ctx.clean.split_sections(_strip_styles(ctx.wikitext), warnings)
    for w in warnings:
        ctx.warn(w)
    if not sections:
        ctx.empty("no prose sections above the length floor")
        return
    for s in sections:
        yield Lore(page=ctx.title, path=tuple(s["path"]), text=s["text"], revid=ctx.revid)
