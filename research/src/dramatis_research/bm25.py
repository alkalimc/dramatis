"""A small BM25 over the same unit pool the dense systems search.

The engine's own lexical numbers (`dramatis-cli evaluate --json`) are imported for the
structural set and reported next to this row. This implementation exists because the engine
searches its full index, not the exported pool, and cannot score the synthetic or verify
sets: a lexical row on the same pool and queries is what makes the systems comparable.

Tokens are lowercase word runs; text in scripts written without spaces is split into single
characters plus adjacent character pairs, which is what an unsegmented index can do.
"""

from __future__ import annotations

import math
import re
from collections import Counter, defaultdict
from collections.abc import Sequence

# Han, Hiragana/Katakana and Hangul ranges, written as escapes so this file stays ASCII.
_UNSPACED = "\u3040-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uac00-\ud7af"
_TOKEN = re.compile(rf"[{_UNSPACED}]+|[^\W{_UNSPACED}]+|_", re.UNICODE)
_RUN = re.compile(rf"^[{_UNSPACED}]+$")


def tokenize(text: str) -> list[str]:
    out: list[str] = []
    for tok in _TOKEN.findall(text.lower()):
        if tok == "_":
            continue
        if _RUN.match(tok):
            out += list(tok)
            out += [tok[i : i + 2] for i in range(len(tok) - 1)]
        else:
            out.append(tok)
    return out


class BM25:
    def __init__(self, ids: Sequence[str], docs: Sequence[str], k1: float = 1.2, b: float = 0.75):
        self.ids = list(ids)
        self.k1, self.b = k1, b
        self.postings: dict[str, list[tuple[int, int]]] = defaultdict(list)
        self.lengths: list[int] = []
        for i, doc in enumerate(docs):
            counts = Counter(tokenize(doc))
            self.lengths.append(sum(counts.values()))
            for term, tf in counts.items():
                self.postings[term].append((i, tf))
        n = len(self.ids)
        self.avg = (sum(self.lengths) / n) if n else 0.0
        self.idf = {
            t: math.log(1 + (n - len(p) + 0.5) / (len(p) + 0.5)) for t, p in self.postings.items()
        }

    def rank(self, query: str, depth: int) -> list[str]:
        scores: dict[int, float] = defaultdict(float)
        for term in set(tokenize(query)):
            idf = self.idf.get(term)
            if idf is None:
                continue
            for i, tf in self.postings[term]:
                norm = 1 - self.b + self.b * self.lengths[i] / (self.avg or 1.0)
                scores[i] += idf * tf * (self.k1 + 1) / (tf + self.k1 * norm)
        top = sorted(scores.items(), key=lambda kv: (-kv[1], kv[0]))[:depth]
        return [self.ids[i] for i, _ in top]
