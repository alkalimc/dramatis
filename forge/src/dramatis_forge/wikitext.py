"""Wikitext mechanism: flatten markup to text, split sections, read tables.

Nothing here knows a template name. Everything a site actually calls its
templates arrives as an `InlineRules` from the pack. What the engine contributes
is the walk order, the escape handling, and — the part that matters — the
guarantee that a construct outside the rules **produces a warning instead of a
guess**.

That guarantee is why the rules are allowed to be incomplete. The alternative,
an exhaustive rule table, is not achievable against a live wiki: editors add
templates, and typos in hand-written markup are permanent.
"""

from __future__ import annotations

import html
import re
from collections.abc import Iterator
from dataclasses import dataclass

import mwparserfromhell as mw

from .pack import ContentSpec, InlineRules

RE_COMMENT = re.compile(r"<!--.*?-->", re.S)
RE_OPEN_COMMENT = re.compile(r"<!--.*$", re.S)
RE_SECTION = re.compile(r"^(=+)\s*([^=\n]+?)\s*=+\s*$", re.M)
RE_TOP_SECTION = re.compile(r"^==\s*([^=\n]+?)\s*==\s*$", re.M)


def strip_comments(s: str) -> str:
    """Remove comments, including one that is never closed.

    An unterminated `<!--` running to end of section is real and appears in
    hand-edited pages; treating it as "no comment here" leaks editor notes into
    the corpus.
    """
    return RE_OPEN_COMMENT.sub("", RE_COMMENT.sub("", s))


def template_name(node) -> str:
    """Normalise a template's name: strip embedded comments, then whitespace.

    Skipping this loses whole infoboxes: generated pages commonly write the name as
    `Name\\n<!-- auto-generated -->\\n`, and an exact-match comparison against `Name`
    then matches almost nothing.
    """
    return strip_comments(str(node.name)).strip()


def strip_tables(s: str) -> str:
    """Delete wikitables, tracking nesting depth.

    Tables in prose are layout: infoboxes repeat text that is already in the body.
    Pages whose *content* is tabular go through `table_rows` instead, so nothing
    is lost by removing them here.
    """
    out: list[str] = []
    depth = 0
    i = 0
    while i < len(s):
        if s.startswith("{|", i):
            depth += 1
            i += 2
            continue
        if s.startswith("|}", i) and depth:
            depth -= 1
            i += 2
            continue
        if not depth:
            out.append(s[i])
        i += 1
    return "".join(out)


def iter_tables(wikitext: str) -> Iterator[str]:
    """Yield each top-level wikitable's source, tracking nesting."""
    depth = 0
    start = 0
    i = 0
    n = len(wikitext)
    while i < n - 1:
        if wikitext[i] == "{" and wikitext[i + 1] == "|":
            if depth == 0:
                start = i
            depth += 1
            i += 2
            continue
        if wikitext[i] == "|" and wikitext[i + 1] == "}":
            depth -= 1
            if depth == 0:
                yield wikitext[start: i + 2]
            elif depth < 0:
                depth = 0
            i += 2
            continue
        i += 1


def _cell(part: str) -> str:
    """Drop a cell's style prefix (`style="…" | content`) without eating content."""
    if "|" in part and re.match(r'^[^|]*(?:=|style|width|rowspan|colspan|scope)', part, re.I):
        head, _, tail = part.partition("|")
        if "=" in head:
            return tail.strip()
    return part.strip()


def table_rows(table: str) -> list[list[str]]:
    """Flatten a wikitable into a matrix of raw cell sources.

    Header (`!`) cells and `rowspan` category cells come back with the rest: the
    caller decides what they mean. That is not laziness — a category written as
    `! rowspan=27 | combat` *is* the row's classification, and a reader that
    discards header cells cannot see it.
    """
    rows: list[list[str]] = []
    for chunk in re.split(r"^\|-.*$", table, flags=re.M)[1:]:
        cells: list[str] = []
        for line in chunk.splitlines():
            s = line.strip()
            if not s or s.startswith("|}") or s.startswith("{|"):
                continue
            if s[0] not in "|!":
                if cells:  # a cell's text continued onto the next line
                    cells[-1] += "\n" + s
                continue
            body = s[1:].lstrip("|!") if s[:2] in ("||", "!!") else s[1:]
            sep = "||" if s[0] == "|" else "!!"
            cells.extend(_cell(part) for part in body.split(sep))
        if cells:
            rows.append(cells)
    return rows


def find_header(table: str, *needles: str) -> tuple[list[str], int] | None:
    """Locate the header row by content, and say which `|-` segment it is.

    Assuming the header is the first segment is wrong on real tables: a merged
    title row above the header is common. Searching for the segment that contains
    the expected column names finds the header *and* tells the caller where the
    data starts, which is the same question asked twice.
    """
    segs = re.split(r"^\|-.*$", table, flags=re.M)
    for i, seg in enumerate(segs):
        cells: list[str] = []
        for line in seg.splitlines():
            s = line.strip()
            if not s.startswith("!"):
                continue
            cells.extend(_cell(part) for part in s[1:].split("!!"))
        cells = [RE_COMMENT.sub("", c).strip() for c in cells]
        if cells and all(any(n in c for c in cells) for n in needles):
            return cells, i
    return None


RE_SPAN = re.compile(r'\b(row|col)span\s*=\s*"?(\d+)"?', re.I)


@dataclass(frozen=True)
class Cell:
    text: str          # raw cell source, attributes removed
    header: bool       # written with `!`


def _row_cells(segment: str) -> list[tuple[str, bool, int, int]]:
    """(source, is_header, rowspan, colspan) for each cell in one `|-` segment."""
    out: list[tuple[str, bool, int, int]] = []
    for line in segment.splitlines():
        s = line.strip()
        if not s or s.startswith(("|}", "{|", "|+")):
            continue
        if s[0] not in "|!":
            if out:  # a cell's text continued onto the next line
                src, hdr, rs, cs = out[-1]
                out[-1] = (src + "\n" + s, hdr, rs, cs)
            continue
        header = s[0] == "!"
        sep = "!!" if header else "||"
        for part in s[1:].split(sep):
            # `attrs | content`, as MediaWiki reads a cell: text before the first bare
            # `|` is attributes, even when empty (`|||-` is an empty-attribute cell
            # holding `-`). A `|` inside a link or template is not that separator.
            attrs, bar, body = part.partition("|")
            if bar and (not attrs.strip() or "=" in attrs or RE_SPAN.search(attrs)) \
                    and "[[" not in attrs and "{{" not in attrs:
                spans = dict((k.lower(), int(v)) for k, v in RE_SPAN.findall(attrs))
                out.append((body.strip(), header, spans.get("row", 1), spans.get("col", 1)))
            else:
                out.append((part.strip(), header, 1, 1))
    return out


def table_grid(table: str) -> list[list[Cell]]:
    """A wikitable as the rectangular grid a browser shows.

    `rowspan` and `colspan` are expanded: a spanned cell appears in every slot it
    covers, so column *i* of every row means the same thing. Reading cells by position
    without this attaches every value after a spanned cell to the wrong column — the
    header says one language and the data is another.
    """
    grid: list[list[Cell]] = []
    carry: dict[int, tuple[Cell, int]] = {}  # column -> (cell, rows still to fill)
    for segment in re.split(r"^\|-.*$", table, flags=re.M):
        cells = _row_cells(segment)
        if not cells:
            continue
        row: list[Cell] = []
        col = 0
        for src, header, rs, cs in cells:
            col = _fill_carried(row, col, carry)
            cell = Cell(src, header)
            for _ in range(cs):
                row.append(cell)
                if rs > 1:
                    carry[col] = (cell, rs - 1)
                col += 1
        _fill_carried(row, col, carry)
        grid.append(row)
    return grid


def _fill_carried(row: list[Cell], col: int, carry: dict[int, tuple[Cell, int]]) -> int:
    """Append cells still spanning down from rows above, starting at `col`; return the
    next free column."""
    while col in carry:
        cell, left = carry[col]
        row.append(cell)
        if left <= 1:
            del carry[col]
        else:
            carry[col] = (cell, left - 1)
        col += 1
    return col


def header_rows(grid: list[list[Cell]]) -> int:
    """How many leading rows are all-header: a table's column labels, however stacked."""
    n = 0
    for row in grid:
        if row and all(c.header for c in row):
            n += 1
        else:
            break
    return n


def tabs(wikitext: str, css_class: str) -> dict[str, str]:
    """Named tab panes `<div id="name" class="css_class">…</div>` on a page."""
    if not css_class:
        return {}
    pattern = re.compile(
        rf'<div id="([^"]+)" class="{re.escape(css_class)}">(.*?)</div>', re.S)
    return {m.group(1): m.group(2) for m in pattern.finditer(wikitext)}


def top_sections(wikitext: str) -> dict[str, str]:
    """`== Heading ==` → body, top level only."""
    out: dict[str, str] = {}
    marks = list(RE_TOP_SECTION.finditer(wikitext))
    for i, m in enumerate(marks):
        end = marks[i + 1].start() if i + 1 < len(marks) else len(wikitext)
        out[m.group(1).strip()] = wikitext[m.end(): end]
    return out


class Cleaner:
    """Flattens wikitext to plain text under one pack's inline rules."""

    def __init__(self, rules: InlineRules) -> None:
        self.rules = rules
        names = "|".join(re.escape(n) for n in rules.link_namespaces)
        self._ns_link = (
            re.compile(rf"\[\[\s*(?:{names})\s*:[^\]]*\]\]", re.I) if names else None)
        self._extra_tags = (
            re.compile(rf"</?(?:{'|'.join(map(re.escape, rules.strip_tags))})\b[^>]*>", re.I)
            if rules.strip_tags else None)
        self._macro_shape = re.compile(rules.macro_shape) if rules.macro_shape else None
        self._letters = re.compile(rules.letters) if rules.letters else None

    def tabs(self, wikitext: str) -> dict[str, str]:
        """Named tab panes, if the site uses them (`InlineRules.tab_class`)."""
        return tabs(wikitext, self.rules.tab_class)

    # ---- templates ----

    def _content_template(self, node, spec: ContentSpec) -> str:
        """Assemble `qualifier: body` from a body-bearing template.

        The qualifier goes *into* the text rather than into metadata because it is
        what makes the unit findable: a synopsis paragraph with its stage name and
        qualifier attached answers "what happened there"; the same paragraph without
        it answers nothing in particular.
        """
        pos = [str(p.value).strip() for p in node.params if not p.showkey]
        body_parts = [pos[i] for i in spec.positional if i < len(pos)]
        body_parts += [
            str(node.get(k).value).strip() for k in spec.named if node.has(k)
        ]
        body = "\n".join(x for x in body_parts if x)
        if not body:
            return ""
        prefix = " ".join(pos[i] for i in spec.prefix if i < len(pos) and pos[i]).strip()
        return f"{prefix}{self.rules.qualifier_sep}{body}" if prefix else body

    def text(self, s: str, warn: list[str] | None = None) -> str:
        """Flatten one span of wikitext to plain text.

        Order matters: comments before tables (a comment can contain `{|`), tables
        before the template walk (a table cell can contain a template we would
        otherwise resurrect), galleries and refs before parsing (their contents are
        never wanted), and the leftover-brace sweep strictly last.
        """
        s = strip_comments(s)
        s = strip_tables(s)
        # Image galleries: no media is archived at all, so not even filenames.
        s = re.sub(r"<gallery[^>]*>.*?</gallery>", "", s, flags=re.S | re.I)
        s = re.sub(r"<gallery[^>]*>.*$", "", s, flags=re.S | re.I)
        s = re.sub(r"<ref[^>]*/>", "", s)
        s = re.sub(r"<ref[^>]*>.*?</ref>", "", s, flags=re.S)
        s = re.sub(r"<br\s*/?>", "\n", s, flags=re.I)

        code = mw.parse(s)
        # Innermost first: an outer template that keeps a parameter then keeps its
        # children's rendered text. Outermost first replaced the outer node with its raw
        # parameter, detaching the children unprocessed, and the brace sweep below then
        # deleted them — a name wrapped in a marker template vanished.
        for node in reversed(code.filter_templates(recursive=True)):
            raw = template_name(node)
            name = raw.lower()
            try:
                # Parser functions and variables (`{{#if:}}`, `{{#var:}}`) are
                # program, not prose.
                if name.startswith("#"):
                    code.remove(node)
                elif name in self.rules.literal:
                    value = self.rules.literal[name]
                    if callable(value):
                        value = value({str(p.name).strip(): str(p.value).strip()
                                       for p in node.params if p.showkey})
                    code.replace(node, value)
                elif name in self.rules.drop:
                    code.remove(node)
                elif raw in self.rules.content:
                    code.replace(node, self._content_template(node, self.rules.content[raw]))
                elif name in self.rules.text_param:
                    vals = [str(p.value) for p in node.params if not p.showkey]
                    idx = self.rules.text_param[name]
                    code.replace(node, vals[idx] if vals and -len(vals) <= idx < len(vals) else "")
                else:
                    # Unknown template: keep the last positional parameter, which
                    # is where decoration templates put their text — and warn,
                    # because that default is exactly how body-bearing templates
                    # get quietly hollowed out.
                    if warn is not None:
                        warn.append(f"unknown inline template: {raw}")
                    vals = [str(p.value) for p in node.params if not p.showkey]
                    code.replace(node, vals[-1] if vals else "")
            except ValueError:
                pass  # already removed as part of an enclosing replacement

        s = str(code)
        # Categories and file links are wiki organisation, not body text.
        if self._ns_link is not None:
            s = self._ns_link.sub("", s)
        s = re.sub(r"\[\[(?:[^\]|]*\|)?([^\]|]*)\]\]", r"\1", s)
        s = re.sub(r"\[(?:https?|//)\S+\s+([^\]]*)\]", r"\1", s)   # [url label] → label
        s = re.sub(r"\[(?:https?|//)\S+\]", "", s)                  # bare external link
        s = re.sub(r"'''+|''", "", s)
        s = re.sub(
            r"</?(?:small|big|code|nowiki|poem|blockquote|i|b|u|s|span|div|p|center|font)[^>]*>",
            "", s, flags=re.I,
        )
        if self._extra_tags is not None:
            s = self._extra_tags.sub("", s)
        # Engine-level macros. Not templates, so the pass above cannot see them; without
        # this a character addresses the player by reciting a variable name.
        for pattern, replacement in self.rules.macros:
            s = pattern.sub(replacement, s)
        # Anything still matching the macro shape is a spelling the rules do not know.
        # Report rather than ship it: an unresolved macro reads as a typo to a user and
        # is unmatchable to a retriever.
        if warn is not None and self._macro_shape is not None:
            for leftover in self._macro_shape.finditer(s):
                warn.append(f"unknown engine macro: {leftover.group(0)}")
        # Last resort: shells of templates whose syntax was too broken for the
        # parser to recognise. Their content has already been read by whichever
        # parameter-name reader wanted it; this only clears the wreckage.
        if "{{" in s:
            s = re.sub(r"\{\{[^{}]*$", "", s, flags=re.S)
            s = re.sub(r"\{\{\s*[^|}\n]*\|?", "", s)
        # A closer with no opener: the section split cut a template in half, so the opener
        # went with the previous section and only its `}}` landed here.
        s = s.replace("}}", "")
        s = re.sub(r"^\s*\|\s*\w+\s*=\s*", "", s, flags=re.M)
        # Character references are markup too; after the tag pass so a decoded `&lt;` can
        # never be mistaken for a tag. A non-breaking space is just a space in prose.
        s = html.unescape(s).replace("\xa0", " ")
        s = re.sub(r"[ \t]+", " ", s)
        s = re.sub(r"\n{3,}", "\n\n", s)
        return s.strip()

    # ---- sections ----

    def split_sections(
        self, body: str, warn: list[str] | None = None, *, min_chars: int = 30
    ) -> list[dict]:
        """Split prose into sections keyed by their heading path.

        Each section also carries `at`, its offset in `body`, so a caller merging these
        with records read another way can put everything back in source order.

        The heading stack is kept so a leaf entry carries its ancestry: an entry
        under "human races" means something different from the same words under
        "reconstructed history", and a retrieval unit that has lost that is a unit
        that will be retrieved for the wrong query.
        """
        out: list[dict] = []
        marks = list(RE_SECTION.finditer(body))
        if not marks:
            text = self.text(body, warn)
            if len(text) >= min_chars:
                out.append({"path": (), "text": text, "at": 0})
            return out

        lead = self.text(body[: marks[0].start()], warn)
        if len(lead) >= min_chars:
            out.append({"path": (self.rules.lead_section,), "text": lead, "at": 0})

        stack: list[str] = []
        for i, m in enumerate(marks):
            level = len(m.group(1))
            title = m.group(2).strip()
            end = marks[i + 1].start() if i + 1 < len(marks) else len(body)
            stack = stack[: max(level - 2, 0)] + [title]
            if any(s in self.rules.drop_sections for s in stack):
                continue
            text = self.text(body[m.end(): end], warn)
            if len(text) >= min_chars:
                out.append({"path": tuple(stack), "text": text, "at": m.start()})
        return out

    @staticmethod
    def headings_at(body: str, offset: int) -> tuple[str, ...]:
        """The heading path in force at `offset` of `body`, outermost first."""
        stack: list[str] = []
        for m in RE_SECTION.finditer(body, 0, offset):
            stack = stack[: max(len(m.group(1)) - 2, 0)] + [m.group(2).strip()]
        return tuple(stack)

    def visible(self, wikitext: str) -> int:
        """Letters a reader of the page sees: markup, comments and links resolved, digits,
        punctuation and whitespace not counted. The yardstick for guard G3's yield check.

        Deliberately generous: templates in the drop list are kept, tables are kept as
        cell text. It measures what the page shows, so a parser that leaves something
        out on purpose does so against a known total.
        """
        s = strip_comments(wikitext)
        s = re.sub(r"<ref[^>]*>.*?</ref>", "", s, flags=re.S)
        s = re.sub(r"\{\{\s*#[^{}]*\}\}", "", s)
        s = re.sub(r"\[\[(?:[^\]|]*\|)?([^\]|]*)\]\]", r"\1", s)
        s = re.sub(r"<[^<>]+>", "", s)
        # Template names and parameter names are markup; parameter values are text.
        s = re.sub(r"\{\{\s*[^|{}]*", "", s)
        s = re.sub(r"\|\s*[^|=\n{}\[\]]{1,24}=", "", s)
        return self.letters(s)

    def letters(self, text: str) -> int:
        """Letters of the corpus language in plain text (`InlineRules.letters`)."""
        if self._letters is not None:
            return len(self._letters.findall(text))
        return sum(1 for ch in text if ch.isalpha())

    def is_meta_page(self, title: str) -> bool:
        return bool(self.rules.meta_pages) and title.startswith(self.rules.meta_pages)
