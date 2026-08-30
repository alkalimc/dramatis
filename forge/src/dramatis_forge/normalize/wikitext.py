"""Wikitext mechanism: flatten markup to text, split sections, read tables.

Nothing here knows a template name. Everything a site actually calls its
templates arrives as an `InlineRules` from the pack. What the engine contributes
is the walk order, the escape handling, and — the part that matters — the
guarantee that a construct outside the rules **produces a warning instead of a
guess**.

That guarantee is why the rules are allowed to be incomplete. The alternative,
an exhaustive rule table, is not achievable against a live wiki: editors add
templates, and typos in hand-written markup are permanent. The measured example
is a directive vocabulary of 77 spellings in one script corpus, including six
distinct misspellings of "delay".
"""

from __future__ import annotations

import re
from collections.abc import Iterator

import mwparserfromhell as mw

from ..pack import ContentSpec, InlineRules

RE_COMMENT = re.compile(r"<!--.*?-->", re.S)
RE_OPEN_COMMENT = re.compile(r"<!--.*$", re.S)
RE_SECTION = re.compile(r"^(=+)\s*([^=\n]+?)\s*=+\s*$", re.M)
RE_TOP_SECTION = re.compile(r"^==\s*([^=\n]+?)\s*==\s*$", re.M)
RE_MACRO = re.compile(r"\{@(\w+)\}")
RE_TAB = re.compile(r'<div id="([^"]+)" class="section_tabcontent">(.*?)</div>', re.S)


def strip_comments(s: str) -> str:
    """Remove comments, including one that is never closed.

    An unterminated `<!--` running to end of section is real and appears in
    hand-edited pages; treating it as "no comment here" leaks editor notes into
    the corpus.
    """
    return RE_OPEN_COMMENT.sub("", RE_COMMENT.sub("", s))


def template_name(node) -> str:
    """Normalise a template's name: strip embedded comments, then whitespace.

    Skipping this loses whole infoboxes. Measured: 455 of 456 operator pages
    write the name as `CharinfoV2\\n<!-- auto-generated -->\\n`, so an exact-match
    comparison against `CharinfoV2` matches exactly one page.
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


def tabs(wikitext: str) -> dict[str, str]:
    """Named tab panes on a page, if it uses them."""
    return {m.group(1): m.group(2) for m in RE_TAB.finditer(wikitext)}


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

    # ---- templates ----

    def _content_template(self, node, spec: ContentSpec) -> str:
        """Assemble `qualifier: body` from a body-bearing template.

        The qualifier goes *into* the text rather than into metadata because it is
        what makes the unit findable: a synopsis paragraph with its stage name and
        timepoint attached answers "what happened in GT-1"; the same paragraph
        without them answers nothing in particular.
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
        return f"{prefix}：{body}" if prefix else body

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
        for node in code.filter_templates(recursive=True):
            raw = template_name(node)
            name = raw.lower()
            try:
                # Parser functions and variables (`{{#if:}}`, `{{#var:}}`) are
                # program, not prose.
                if name.startswith("#"):
                    code.remove(node)
                elif name in self.rules.literal:
                    code.replace(node, self.rules.literal[name])
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
        s = re.sub(r"\[\[\s*(?:分类|Category|文件|File|Image)\s*:[^\]]*\]\]", "", s, flags=re.I)
        s = re.sub(r"\[\[(?:[^\]|]*\|)?([^\]|]*)\]\]", r"\1", s)
        s = re.sub(r"\[(?:https?|//)\S+\s+([^\]]*)\]", r"\1", s)   # [url label] → label
        s = re.sub(r"\[(?:https?|//)\S+\]", "", s)                  # bare external link
        s = re.sub(r"'''+|''", "", s)
        s = re.sub(
            r"</?(?:small|big|code|nowiki|poem|blockquote|i|b|u|s|span|div|p|center|font)[^>]*>",
            "", s, flags=re.I,
        )
        s = re.sub(r"<color[^>]*>|</color>", "", s, flags=re.I)
        # Engine-level macros. Not templates, so the pass above cannot see them; without
        # this a character addresses the player by reciting a variable name.
        for pattern, replacement in self.rules.macros:
            s = pattern.sub(replacement, s)
        # Anything still matching the macro shape is a spelling the rules do not know.
        # Report rather than ship it: an unresolved macro reads as a typo to a user and
        # is unmatchable to a retriever.
        if warn is not None:
            for leftover in RE_MACRO.findall(s):
                warn.append(f"unknown engine macro: {{@{leftover}}}")
        # Last resort: shells of templates whose syntax was too broken for the
        # parser to recognise. Their content has already been read by whichever
        # parameter-name reader wanted it; this only clears the wreckage.
        if "{{" in s:
            s = re.sub(r"\{\{[^{}]*$", "", s, flags=re.S)
            s = re.sub(r"\{\{\s*[^|}\n]*\|?", "", s)
            s = s.replace("}}", "")
        s = re.sub(r"^\s*\|\s*\w+\s*=\s*", "", s, flags=re.M)
        s = re.sub(r"[ \t]+", " ", s)
        s = re.sub(r"\n{3,}", "\n\n", s)
        return s.strip()

    # ---- sections ----

    def split_sections(
        self, body: str, warn: list[str] | None = None, *, min_chars: int = 30
    ) -> list[dict]:
        """Split prose into sections keyed by their heading path.

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
                out.append({"path": (), "text": text})
            return out

        lead = self.text(body[: marks[0].start()], warn)
        if len(lead) >= min_chars:
            out.append({"path": ("引言",), "text": lead})

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
                out.append({"path": tuple(stack), "text": text})
        return out

    def is_meta_page(self, title: str) -> bool:
        return bool(self.rules.meta_pages) and title.startswith(self.rules.meta_pages)
