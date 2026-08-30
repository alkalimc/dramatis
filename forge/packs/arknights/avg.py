"""AVG script parsing: a whitelist, never a blacklist.

The scripts are a stage-direction language embedded in wikitext. Across the full
corpus of roughly 1.1 million script lines there are **77 distinct directive
spellings**, including six hand-typed misspellings of "delay" (`dalay`, `daley`,
`dealy`, `delau`, `delya`) and others like `palysound`, `stopmucis`, `charslsot`. A
blacklist of directives to skip cannot be completed, and cannot stay complete,
because new typos ship with new story.

So only five forms produce records and everything else is discarded:

    [name="X"] line          spoken line
    [multiline(name="X")] …  continuation of the previous speaker
    bare text                narration
    [Decision(options=…)]    a branch offered to the player
    [Subtitle|Sticker|animtext|spellsticker]   on-screen text

The whitelist has variants of its own — single quotes, trailing parameters, an
omitted `name=` — and every one of them was found by a guard on a full run rather
than imagined at design time. That is the argument for guarding instead of
enumerating: the rules are wrong in ways only the data knows.
"""

from __future__ import annotations

import re
from collections.abc import Iterable, Iterator

from dramatis_forge.normalize.records import Choice, Line, Record, Scene
from dramatis_forge.pack import PageContext

# Quotes appear in both forms (`[name='寒檀']`, twenty lines on one page), and the
# bracket can carry extra parameters (`[name="可露希尔",delay=0.1]`). Accepting only
# double-quoted, parameter-free forms silently discards real dialogue.
RE_NAME = re.compile(r"""^\[name\s*=\s*(["'])(.*?)\1[^\]]*\]\s*(.*)$""", re.I)
RE_MULTILINE = re.compile(
    r"""^\[multiline\s*(?:\(\s*name\s*=\s*(["'])(.*?)\1[^)]*\))?[^\]]*\]\s*(.*)$""", re.I
)
RE_DECISION = re.compile(r"^\[decision\s*\((.*)\)\]\s*$", re.I)
# `spellsticker` carries incanted spell text — it is dialogue, not decoration.
RE_TEXTPARAM = re.compile(r"^\[(subtitle|sticker|animtext|spellsticker)\b(.*)$", re.I)
#: On-screen text arrives inside a quoted script attribute, so its line breaks are
#: written as the two characters `\` and `n`. Measured on a full run: 1,833 caption
#: lines carried the escape through verbatim, which reads as a typo in the middle of a
#: sentence. The other escapes below occur in the same position and for the same reason.
_ESCAPES = (("\\n", "\n"), ("\\t", " "), ('\\"', '"'), ("\\'", "'"))


def _unescape(text: str) -> str:
    for encoded, decoded in _ESCAPES:
        text = text.replace(encoded, decoded)
    return text
RE_DIRECTIVE = re.compile(r"^\[([A-Za-z_]+)")
RE_DATA_TRANSCLUDE = re.compile(r"\{\{:\s*(?:\{\{PAGENAME\}\}|[^}]*?)/data\s*\}\}")
RE_OPTIONS = re.compile(r'options\s*=\s*"([^"]*)"')
RE_TEXT_ATTR = re.compile(r'text\s*=\s*"([^"]*)"')
RE_PARAGRAPH = re.compile(r"<p=\d+>(.*?)</>")
# Beta scripts and PRTS reconstructions are outside the canon we archive.
RE_BETA = re.compile(r"\|\s*内测\s*=\s*1")

BODY_MARKER = "文本数据="
DATA_SUFFIX = "/data"

#: Fixed noise strings that appear as bare lines. A closed set, not a pattern.
BARE_NOISE = frozenset({"}}", "{{剧情导航}}", "{{内测剧情导航}}", "{{活动剧情导航}}"})

SPEECH, NARRATION, CAPTION = "对白", "旁白", "字幕"


def discover_data_subpages(title: str, wikitext: str) -> Iterable[str]:
    """Followup hook: story pages whose body is transcluded from `X/data`.

    These subpages are in no seed set — the site does not register them anywhere
    enumerable — so this is the one genuine hole in enumeration, plugged by looking
    at what we already fetched. Measured: two pages.
    """
    i = wikitext.find(BODY_MARKER)
    if i < 0:
        return ()
    if RE_DATA_TRANSCLUDE.search(wikitext[i:]):
        return (f"{title}{DATA_SUFFIX}",)
    return ()


def _body(ctx: PageContext) -> str | None:
    """The script body, or None with a reason recorded on the context."""
    if ctx.title.endswith(DATA_SUFFIX):
        # A `/data` subpage *is* the body; it carries no wrapper template. Parsing
        # it here rather than splicing it into the parent also means the record
        # provenance points at the revision that actually holds the text.
        return ctx.wikitext

    i = ctx.wikitext.find(BODY_MARKER)
    if i < 0:
        ctx.warn(f"story page with no {BODY_MARKER} parameter", high=True)
        return None
    body = ctx.wikitext[i + len(BODY_MARKER):]
    if RE_DATA_TRANSCLUDE.search(body):
        ctx.empty(f"body is transcluded from {ctx.title}{DATA_SUFFIX} and is parsed there")
        return None
    return body


def _scene_id(title: str) -> str:
    return title[: -len(DATA_SUFFIX)] if title.endswith(DATA_SUFFIX) else title


def parse_story(ctx: PageContext) -> Iterator[Record]:
    if RE_BETA.search(ctx.wikitext):
        # S1 comes from the site's own story table, so a beta script appearing in
        # it means the upstream registration changed. Report rather than absorb.
        ctx.warn("beta script (|内测=1) found inside the canon story set", high=True)
        return

    body = _body(ctx)
    if body is None:
        return

    scene = _scene_id(ctx.title)
    lines, choices = _read(body, ctx)
    if not lines and not choices:
        if re.search(r"\[video\b", body, re.I):
            ctx.empty("cut-scene video only, no text — a fact, not a failure")
        return

    meta = ctx.row("story", scene)
    yield Scene(
        page=scene,
        story_type=meta.get("storyType", ""),
        story_group=meta.get("storyGroup", ""),
        text_path=meta.get("textPath", ""),
        revid=ctx.revid,
    )
    yield from lines
    yield from choices


def _read(body: str, ctx: PageContext) -> tuple[list[Line], list[Choice]]:
    lines: list[Line] = []
    choices: list[Choice] = []
    seq = 0

    for raw in body.splitlines():
        s = raw.strip()
        if not s:
            continue

        if not s.startswith("["):
            if s in BARE_NOISE or s.startswith("//") or s.startswith("{{") or s.startswith("|"):
                continue
            text = ctx.clean.text(s)
            if text:
                seq += 1
                lines.append(Line(scene=_scene_id(ctx.title), seq=seq, text=text, kind=NARRATION))
            continue

        is_multi = s.lower().startswith("[multiline")
        m = RE_MULTILINE.match(s) if is_multi else RE_NAME.match(s)
        if m:
            speaker = (m.group(2) or "").strip()
            text = ctx.clean.text(m.group(3))
            if not text:
                continue
            # A multiline block continues the previous utterance. Emitting it as a
            # separate line would fragment one speech into several retrieval units
            # and break the speaker-attribution statistics.
            if is_multi and lines and (not speaker or lines[-1].speaker == speaker):
                prev = lines[-1]
                lines[-1] = Line(
                    scene=prev.scene, seq=prev.seq, text=f"{prev.text}\n{text}",
                    kind=prev.kind, speaker=prev.speaker,
                )
                continue
            seq += 1
            lines.append(Line(
                scene=_scene_id(ctx.title), seq=seq, text=text,
                kind=SPEECH if speaker else NARRATION, speaker=speaker or None,
            ))
            continue

        m = RE_DECISION.match(s)
        if m:
            opts = RE_OPTIONS.search(m.group(1))
            if opts:
                options = tuple(
                    ctx.clean.text(o) for o in opts.group(1).split(";") if o.strip()
                )
                if options:
                    seq += 1
                    choices.append(Choice(scene=_scene_id(ctx.title), seq=seq, options=options))
            continue

        m = RE_TEXTPARAM.match(s)
        if m:
            attr = RE_TEXT_ATTR.search(m.group(2))
            parts = RE_PARAGRAPH.findall(m.group(2))
            for t in ([attr.group(1)] if attr else []) + parts:
                t = ctx.clean.text(_unescape(t))
                if t:
                    seq += 1
                    lines.append(Line(
                        scene=_scene_id(ctx.title), seq=seq, text=t, kind=CAPTION))
            continue

        _report_trailing_text(s, ctx)

    return lines, choices


def _report_trailing_text(s: str, ctx: PageContext) -> None:
    """Outside the whitelist, only *trailing prose* is worth a warning.

    Two tiers, because otherwise real problems drown:

    * A stray bracket or a single leftover character after a directive
      (`[Character]]`, a lone `。`) is an editing slip. Twenty-one occurrences
      measured. Discard, count as low.
    * A full clause after a directive is the signal that content may be escaping
      the whitelist. High.

    `[HEADER(...)]` is exempt: its trailing text is an editor's chapter note, not a
    line of dialogue, and it fires on every story page.
    """
    directive = RE_DIRECTIVE.match(s)
    rest = s[s.rfind("]") + 1:].strip() if "]" in s else ""
    if not (rest and directive) or directive.group(1).lower() == "header":
        return
    ctx.warn(f"text trailing a non-whitelisted directive: {s[:80]}", high=len(rest) > 3)
