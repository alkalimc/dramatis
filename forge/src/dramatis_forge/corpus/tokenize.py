"""Segmentation for the lexical path.

Chinese has no word delimiters, so an FTS5 index over raw text either treats whole
sentences as tokens or falls back to per-character matching. The fix chosen here is
to **segment at build time and store the segmented text in an ordinary
`unicode61` column**. BM25 comes out the same as with a custom tokeniser, and:

  * no native dependency, no per-platform shared library to ship
  * no requirement that SQLite was compiled with extension loading enabled — some
    platform Python builds have it off, which turns a load-time failure into a
    support burden for a feature nobody asked about
  * the reader side needs only the same segmenter, not the same SQLite build

The cost is that query-time segmentation must match build-time segmentation. That is
a real constraint, so which segmenter was used is recorded in the folio manifest and
the engine can refuse a mismatch rather than silently return worse results.
"""

from __future__ import annotations

from dataclasses import dataclass

CJK_RANGES = (
    (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF),
    (0x20000, 0x2A6DF), (0x2A700, 0x2EBEF),
)


def _is_cjk(ch: str) -> bool:
    cp = ord(ch)
    return any(lo <= cp <= hi for lo, hi in CJK_RANGES)


@dataclass(frozen=True)
class Segmenter:
    name: str
    version: str

    def __call__(self, text: str) -> str:  # pragma: no cover - replaced below
        raise NotImplementedError


class _Jieba(Segmenter):
    def __call__(self, text: str) -> str:
        import jieba

        return " ".join(t for t in jieba.cut_for_search(text) if t.strip())


class _Bigram(Segmenter):
    """Fallback: character bigrams for CJK runs, whitespace elsewhere.

    Weaker than a real segmenter — it over-generates tokens and so inflates the index
    — but it is *correct*, deterministic, and dependency-free, which makes it a usable
    default rather than a broken one. Overlapping bigrams matter: without the overlap
    a query whose word straddles a pair boundary matches nothing.
    """

    def __call__(self, text: str) -> str:
        out: list[str] = []
        run: list[str] = []
        latin: list[str] = []

        def flush_cjk() -> None:
            if not run:
                return
            if len(run) == 1:
                out.append(run[0])
            else:
                out.extend(run[i] + run[i + 1] for i in range(len(run) - 1))
            run.clear()

        def flush_latin() -> None:
            if latin:
                out.append("".join(latin))
                latin.clear()

        for ch in text:
            if _is_cjk(ch):
                flush_latin()
                run.append(ch)
            elif ch.isalnum():
                flush_cjk()
                latin.append(ch)
            else:
                flush_cjk()
                flush_latin()
        flush_cjk()
        flush_latin()
        return " ".join(out)


def load(prefer: str = "auto") -> Segmenter:
    """Pick a segmenter, reporting which one so the choice is never invisible."""
    if prefer in ("auto", "jieba"):
        try:
            import jieba

            return _Jieba(name="jieba", version=getattr(jieba, "__version__", "unknown"))
        except ImportError:
            if prefer == "jieba":
                raise
    return _Bigram(name="char-bigram", version="1")
