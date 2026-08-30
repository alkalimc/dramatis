"""Six seed sets, each defined by something the site itself asserts.

Not one of these is a guess about what a page contains. Each is a membership query
whose answer the wiki maintains: rows of a structured table, transclusions of a
template, a namespace, a category, or a list a human wrote down once.

The one hand-listed set is the index pages, and that is correct rather than lazy:
there are nine of them, each needs its own reader anyway, and a wildcard over titles
would be a more fragile way to name nine things.
"""

from __future__ import annotations

from collections.abc import Iterable

from dramatis_forge.pack import HarvestContext, SeedSet
from dramatis_forge.wiki import Wiki

VOICE_SUFFIX = "/语音记录"
VOICE_TEMPLATE = "模板:VoiceTable"
CODEX_NAMESPACE = 3000
DISAMBIG_CATEGORY = "分类:消歧义页"

#: Hand-listed index pages. Each has its own reader in `parsers/index.py`.
#:
#: Two subpages that a title-prefix rule would sweep in are deliberately absent, both
#: because a guard reported them producing nothing:
#:
#: * `角色真名/编辑指南` is an editing standard for the page above it — wiki
#:   meta-space, the same category as the codex's own style guides.
#: * `邮件记录/国际服` is the global server's mail in Japanese and English, marked
#:   work-in-progress. Partly a translation of the Chinese page and partly
#:   server-exclusive text; either way it falls under the same single-language rule
#:   that drops eleven language columns from the voice tables.
INDEX_PAGES: tuple[str, ...] = (
    "剧情角色一览",
    "角色真名",
    "泰拉词库",
    "邮件记录",
    "情报处理室",
    "贴士一览",
    "游戏内容前瞻",
)

#: Appearance matrices. They feed the roster's "who was in what" view and produce no
#: corpus records, so they are registered as non-corpus to keep the coverage report
#: from claiming credit for text it did not extract.
APPEARANCE_PAGES: tuple[str, ...] = (
    "干员剧情一览/六星干员", "干员剧情一览/五星干员", "干员剧情一览/四星干员",
    "干员剧情一览/三星干员", "干员剧情一览/一二星干员",
)


def _story(wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
    """Canon story scripts.

    Membership in the site's story table *is* the definition of canon here. A second,
    independent signal — an in-page beta marker — is checked during normalisation, and
    the two agreed on a full run, which is what makes the boundary verifiable rather
    than asserted.
    """
    return {r["page"] for r in ctx.rows("story") if r.get("page")}


def _operators(wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
    """Operator pages, from the structured roster table rather than by parsing."""
    return {r["page"] for r in ctx.rows("chara") if r.get("page")}


def _voices(wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
    """Voice-record subpages.

    Transclusion alone is not enough: every operator's main page transcludes the
    voice table too, so `embeddedin` returns both the subpage and the parent. The
    title suffix separates them, and the parser then confirms the table is written
    locally rather than pulled in.
    """
    return (t for t in wiki.embeddedin(VOICE_TEMPLATE, ns=0) if t.endswith(VOICE_SUFFIX))


def _codex(wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
    """The in-world encyclopaedia namespace, entire. Selection happens inside pages.

    Redirects excluded: `allpages` includes them by default, and a redirect has no
    body to parse, so each one lands in guard G3 as an unexplained empty page. Worse,
    one of them was being *mis*-attributed — a redirect whose title happens to match
    the meta-page prefix was reported as an editing guide. Their alias value is
    already captured by the redirect seed set.
    """
    return wiki.allpages(CODEX_NAMESPACE, redirects="nonredirects")


def _redirects(wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
    """Every redirect in the main namespace.

    Free human-curated synonymy: thousands of "this name means that entity"
    judgements, already made. Turned one way it is query normalisation; turned the
    other it is a retrieval benchmark with no annotation budget.
    """
    return wiki.allpages(0, redirects="redirects")


def _disambiguations(wiki: Wiki, ctx: HarvestContext) -> Iterable[str]:
    """Disambiguation pages: one word, several entities.

    Also the natural source of *hard* negatives — the entities a disambiguation page
    lists are exactly the ones a retriever will confuse, chosen by a human who
    noticed the confusion.
    """
    return wiki.categorymembers(DISAMBIG_CATEGORY)


SEEDS: tuple[SeedSet, ...] = (
    SeedSet("S1", "story scripts", "structured table `story`",
            enumerate=_story, baseline=2086),
    SeedSet("S2", "operator pages", "structured table `chara`",
            enumerate=_operators, baseline=456, roster=True),
    SeedSet("S3", "voice records", f"transclusion + `{VOICE_SUFFIX}` suffix",
            enumerate=_voices, baseline=432),
    SeedSet("S4", "world codex", f"namespace {CODEX_NAMESPACE}, non-redirects",
            enumerate=_codex, baseline=126),
    SeedSet("S5", "index pages", f"{len(INDEX_PAGES)} hand-listed pages",
            fixed=INDEX_PAGES, baseline=len(INDEX_PAGES)),
    # Enumerated but not fetched. The matrices are a genuine roster feature — who
    # appears in which story — but nothing consumes them yet, and fetching pages no
    # reader handles would leave five permanent unattributed entries in guard G3.
    # Flip `fetch` on in the same change that adds the reader.
    SeedSet("S5R", "appearance matrices", f"{len(APPEARANCE_PAGES)} hand-listed pages",
            fixed=APPEARANCE_PAGES, baseline=len(APPEARANCE_PAGES),
            corpus=False, fetch=False),
    SeedSet("S6", "redirects", "main-namespace redirects",
            enumerate=_redirects, baseline=7412,
            fetch=False, corpus=False, alias_kind="redirect"),
    SeedSet("S6D", "disambiguations", DISAMBIG_CATEGORY,
            enumerate=_disambiguations, baseline=146,
            fetch=False, corpus=False, alias_kind="disambig"),
    # Not enumerable in advance: story pages whose body is transcluded from a subpage
    # the site registers nowhere. Populated by the fetch follow-up hook, and preserved
    # across re-scopes because nothing else knows these titles exist.
    SeedSet("S1D", "transcluded story bodies", "discovered during fetch",
            baseline=2, discovered=True),
)
