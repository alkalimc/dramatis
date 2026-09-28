"""A toy pack for the forge's tests: a fake English wiki with two seed sets.

`story` pages are dialogue scripts, one line per utterance:

    {{Scene|category=Main|group=Chapter One}}
    Alice: Words spoken by Alice.
    > Narration.
    [caption] On-screen text.
    [choice] First option | Second option

`people` pages are prose, parsed into sections; they form the roster. A page carrying
`{{AltForm|<name>}}` is another form of that person. An `{{Info|key=value|…}}` line is the
person's dossier: `born` (MM-DD, or anything else when unknown), `home`, `role`.
Everything here is invented.
"""

from __future__ import annotations

import re
from collections.abc import Iterator

from dramatis_forge.pack import (
    ChunkPolicy,
    ChunkTemplate,
    ContentSpec,
    DocAudit,
    FigureSpec,
    FindingNote,
    IdentityRules,
    InlineRules,
    Pack,
    PageContext,
    PersonSource,
    Route,
    RosterRules,
    SeedSet,
    WikiConfig,
)
from dramatis_forge.records import Choice, Dossier, Line, Lore, Record, Scene

STORY_CATEGORY = "Category:Stories"
RE_SCENE = re.compile(r"\{\{Scene\|category=([^|}]*)\|group=([^|}]*)\}\}")
RE_SPEECH = re.compile(r"^([A-Z][A-Za-z]*):\s*(.+)$")
RE_ALT = re.compile(r"\{\{AltForm\|([^}]+)\}\}")
RE_INFO = re.compile(r"\{\{Info\|([^}]*)\}\}")
RE_BORN = re.compile(r"^\d\d-\d\d$")


def parse_story(ctx: PageContext) -> Iterator[Record]:
    if "{{NoScript}}" in ctx.wikitext:
        ctx.empty("marked as having no script")
        return
    head = RE_SCENE.search(ctx.wikitext)
    records: list[Record] = []
    warnings: list[str] = []
    seq = 0
    for raw in ctx.wikitext.splitlines():
        raw = raw.strip()
        if not raw or RE_SCENE.fullmatch(raw):
            continue
        if raw.startswith("[choice]"):
            options = tuple(o.strip() for o in raw[len("[choice]"):].split("|"))
            records.append(Choice(scene=ctx.title, seq=seq, options=options))
        elif raw.startswith("[caption]"):
            text = ctx.clean.text(raw[len("[caption]"):], warnings)
            records.append(Line(scene=ctx.title, seq=seq, text=text, kind="subtitle"))
        elif m := RE_SPEECH.match(raw):
            records.append(Line(scene=ctx.title, seq=seq, speaker=m.group(1),
                                text=ctx.clean.text(m.group(2), warnings)))
        else:
            text = ctx.clean.text(raw.lstrip("> "), warnings)
            records.append(Line(scene=ctx.title, seq=seq, text=text, kind="narration"))
        seq += 1
    for detail in warnings:
        ctx.warn(detail)
    if not records:
        return  # silently empty: guard G3 must flag it
    yield Scene(page=ctx.title, category=head.group(1) if head else "",
                group=head.group(2) if head else "", revid=ctx.revid)
    yield from records


def parse_prose(ctx: PageContext) -> Iterator[Record]:
    warnings: list[str] = []
    info = RE_INFO.search(ctx.wikitext)
    if info:
        fields = dict(part.split("=", 1) for part in info.group(1).split("|") if "=" in part)
        yield Dossier(page=ctx.title, fields=fields, revid=ctx.revid)
    for section in ctx.clean.split_sections(RE_INFO.sub("", ctx.wikitext), warnings):
        yield Lore(page=ctx.title, path=section["path"], text=section["text"],
                   revid=ctx.revid)
    for detail in warnings:
        ctx.warn(detail)


def birthday(person: PersonSource) -> str | None:
    born = person.facets.get("born", "")
    return born if RE_BORN.match(born) else None


ROSTER = RosterRules(
    birthday=birthday,
    topic_terms=lambda p: [p.facets[k] for k in ("home", "role") if p.facets.get(k)],
    faction=lambda p: p.facets.get("home", ""),
    reason=lambda p: p.facets.get("role", ""),
)


def resolve_identity(pages, roster):
    out = {}
    for title, text in pages.items():
        m = RE_ALT.search(text)
        if m:
            out[title] = (m.group(1).strip(), "alt")
    return out


PACK = Pack(
    name="toy",
    version=1,
    wiki=WikiConfig(
        api="https://toy.invalid/api.php",
        contact="maintainer@example.invalid",
        source_url="https://toy.invalid/wiki/{page}",
        history_url="https://toy.invalid/history/{page}",
        revision_url="https://toy.invalid/rev/{revid}",
    ),
    seeds=(
        SeedSet("story", "Stories", "category members",
                enumerate=lambda wiki, ctx: wiki.categorymembers(STORY_CATEGORY)),
        SeedSet("people", "People", "hand-listed", roster=True,
                fixed=("Alice", "Alice (Winter)", "Bob")),
    ),
    routes=(
        Route("story", parse_story, label="dialogue"),
        Route("people", parse_prose, label="prose", min_yield=0.5),
    ),
    followups=(),
    inline=InlineRules(
        drop=frozenset({"cite", "altform", "noscript"}),
        text_param={"color": -1},
        content={"Quote": ContentSpec(positional=(1,), prefix=(0,))},
        literal={"reader": "the reader"},
        # The reader's name stays a placeholder in the corpus; the runtime fills it in.
        macros=((re.compile(r"\$\{player\}"), "{user}"),),
        letters=r"[A-Za-z]",
        macro_shape=r"\$\{\w+\}",
    ),
    identity=IdentityRules(resolve=resolve_identity, form_order={"alt": 1}),
    chunking=ChunkPolicy(
        templates=(
            ChunkTemplate("lore", sources=("lore",), max_chars=600),
            ChunkTemplate("dialogue", sources=("line", "choice"), max_chars=600,
                          target=3, max_span=4),
            ChunkTemplate("profile", sources=("dossier",), max_chars=600),
        ),
        headers={"lore": "[lore] {page}{path}",
                 "dialogue": "[scene] {group} · {scene}{speakers}",
                 "profile": "[file] {person}{section}"},
    ),
    figures=(
        FigureSpec("units", "folio:chunk_count", target="> 0"),
        FigureSpec("persons", "derived:persons", target=">= 2"),
        FigureSpec("forms.alt", "derived:forms_by_kind", member="alt", target="== 1"),
        FigureSpec("guards.high", "derived:guards_high", target="== 0"),
        FigureSpec("guards.by_guard", "derived:guards_by_guard"),
        FigureSpec("guards.unattributed", "derived:guards_unattributed"),
        FigureSpec("units.attributed", "derived:units_attributed"),
        FigureSpec("cooccur.pairs", "derived:cooccur_pairs", target="== 1"),
        FigureSpec("cooccur.scenes_per_person", "derived:cooccur_scenes_per_person"),
        FigureSpec("stopword.top_df", "derived:stopword_top_df"),
        FigureSpec("pipeline.hours", "derived:pipeline_hours"),
        FigureSpec("birthdays", "derived:birthdays", target="== 1"),
    ),
    finding_notes=(
        FindingNote("G3", "marked as having no script", "interludes carry no script"),
    ),
    roster=ROSTER,
    sample_pages={"Bob": "the only person without a birthday"},
    wording={"host.name": "Keeper", "user.title": "Captain {name}"},
    year_offset=100,
    audit=DocAudit(id_prefixes=("D",), planned_keys=("units.later",)),
    text={"samples.title": "# Toy samples", "record.lore": "prose sections"},
    stopwords=("the", "a"),
)
