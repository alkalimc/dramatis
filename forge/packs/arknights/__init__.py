"""The Arknights pack: every domain-specific rule in the forge, assembled.

Read this file to see the shape of a pack; read the modules it imports to see the
reasoning. Nothing in `dramatis_forge/` knows any of it.

Provenance and terms, stated here because it is the first place anyone touching the
data will look: the source is prts.wiki, a volunteer-run wiki whose editorial
contributions are CC BY-NC-SA. The underlying fiction — the dialogue, the character
files, the world text — belongs to its rights holder, and that licence does not cover
it. Anything built from this pack therefore carries per-page attribution with a
revision id, is non-commercial, and names a takedown contact. See
`docs/attribution.md`.
"""

from __future__ import annotations

from dramatis_forge.pack import Followup, Pack, Route, WikiConfig

from . import avg, chunks, coverage, fields, identity, inline, parsers, seeds
from .wording import WORDING

WIKI = WikiConfig(
    api="https://prts.wiki/api.php",
    # A volunteer-run site is about to receive a few thousand requests from us. A
    # reachable maintainer is the minimum owed in return.
    contact="https://github.com/alkalimc/dramatis",
    # Deliberately below the 8/s the client defaults to. The full sync is ~3,100 page
    # bodies plus index queries; finishing twenty minutes sooner is worth nothing next
    # to being a good guest on someone else's server.
    rate=5.0,
    watch_namespaces=(0, 3000),
)

ROUTES: tuple[Route, ...] = (
    Route("S1", avg.parse_story, label="story script"),
    Route("S1D", avg.parse_story, label="transcluded story body"),
    Route("S2", parsers.parse_dossier, label="operator dossier"),
    Route("S3", parsers.parse_voices, label="voice records"),
    Route("S4", parsers.parse_lore, label="world codex"),
    # The index pages: each one is a layout of its own, so each gets an exact-title
    # route. Exact titles outrank the bare-seed fallback, so order here is not
    # load-bearing.
    Route("S5", parsers.parse_char_refs, title="剧情角色一览", label="character register"),
    Route("S5", parsers.parse_real_names, title="角色真名", label="real names"),
    Route("S5", parsers.parse_glossary, title="泰拉词库", label="glossary"),
    Route("S5", parsers.parse_letters, title="邮件记录", label="letters"),
    Route("S5", parsers.parse_tips, title="贴士一览", label="tips"),
    Route("S5", parsers.parse_synopses, title="情报处理室", label="mission synopses"),
    Route("S5", parsers.parse_prose, label="index prose"),
)

FOLLOWUPS: tuple[Followup, ...] = (
    Followup(
        of_seed="S1",
        seed="S1D",
        discover=avg.discover_data_subpages,
        label="story bodies transcluded from /data",
    ),
)

PACK = Pack(
    name="arknights",
    #: Bump when the pack's rules change in a way that alters output. Separate from
    #: the parser version, which tracks the engine's normalisation mechanism.
    version=1,
    wiki=WIKI,
    seeds=seeds.SEEDS,
    routes=ROUTES,
    followups=FOLLOWUPS,
    inline=inline.RULES,
    identity=identity.RULES,
    chunking=chunks.POLICY,
    tables=fields.TABLES,
    multi_row_tables=fields.MULTI_ROW_TABLES,
    field_names=fields.FIELD_NAMES,
    wording=WORDING,
    coverage=coverage.COVERAGE,
    # Pages whose entire output is dictionary entries rather than corpus records.
    # Without this, `report inspect` says a working parser "produced nothing".
    alias_pages={"角色真名": "realname"},
)
