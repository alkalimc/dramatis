"""Embedding and reranking backends behind two small interfaces.

The evaluator and the tests only see `encode_queries` / `encode_docs` / `score_pairs`, so a
real model, a fine-tuned checkpoint and the offline stand-ins are interchangeable.
"""

from __future__ import annotations

import hashlib
import os
import re
from collections.abc import Sequence
from typing import Protocol

import numpy as np

from . import models


def pick_device() -> str:
    """cuda, then Apple MPS, then CPU. `DRAMATIS_RESEARCH_DEVICE` overrides."""
    forced = os.environ.get("DRAMATIS_RESEARCH_DEVICE")
    if forced:
        return forced
    import torch

    if torch.cuda.is_available():
        return "cuda"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def is_accelerator(device: str) -> bool:
    return device != "cpu"


class Embedder(Protocol):
    def encode_queries(self, texts: Sequence[str]) -> np.ndarray: ...

    def encode_docs(self, texts: Sequence[str]) -> np.ndarray: ...


class Reranker(Protocol):
    def score_pairs(self, pairs: Sequence[tuple[str, str]]) -> list[float]: ...


def _normalise(x: np.ndarray) -> np.ndarray:
    norm = np.linalg.norm(x, axis=1, keepdims=True)
    return x / np.maximum(norm, 1e-12)


class STEmbedder:
    """A sentence-transformers model (base or fine-tuned) with a query instruction."""

    def __init__(
        self,
        path: str,
        device: str | None = None,
        max_seq_length: int = 512,
        query_instruction: str = models.QUERY_INSTRUCTION,
        batch_size: int = 16,
    ):
        from sentence_transformers import SentenceTransformer

        self.model = SentenceTransformer(path, device=device or pick_device())
        self.model.max_seq_length = max_seq_length
        self.instruction = query_instruction
        self.batch_size = batch_size

    def _encode(self, texts: Sequence[str]) -> np.ndarray:
        out = self.model.encode(
            list(texts),
            batch_size=self.batch_size,
            convert_to_numpy=True,
            show_progress_bar=False,
        )
        return _normalise(np.asarray(out, dtype=np.float32))

    def encode_queries(self, texts: Sequence[str]) -> np.ndarray:
        return self._encode([models.format_query(self.instruction, t) for t in texts])

    def encode_docs(self, texts: Sequence[str]) -> np.ndarray:
        return self._encode(texts)


_WORD = re.compile(r"\w+", re.UNICODE)


class HashingEmbedder:
    """Offline stand-in: hashed character trigrams. Deterministic, no weights, CPU in
    milliseconds. Used by the tests and by `--embedder hashing` dry runs of the pipeline."""

    def __init__(self, dim: int = 512):
        self.dim = dim

    def _vec(self, text: str) -> np.ndarray:
        v = np.zeros(self.dim, dtype=np.float32)
        for word in _WORD.findall(text.lower()):
            padded = f"^{word}$"
            for i in range(max(1, len(padded) - 2)):
                h = hashlib.md5(padded[i : i + 3].encode()).digest()
                v[int.from_bytes(h[:4], "little") % self.dim] += 1.0
        return v

    def _encode(self, texts: Sequence[str]) -> np.ndarray:
        if not texts:
            return np.zeros((0, self.dim), dtype=np.float32)
        return _normalise(np.stack([self._vec(t) for t in texts]))

    def encode_queries(self, texts: Sequence[str]) -> np.ndarray:
        return self._encode(texts)

    def encode_docs(self, texts: Sequence[str]) -> np.ndarray:
        return self._encode(texts)


class OverlapReranker:
    """Offline stand-in: fraction of query words present in the document."""

    def score_pairs(self, pairs: Sequence[tuple[str, str]]) -> list[float]:
        out = []
        for q, d in pairs:
            qw = set(_WORD.findall(q.lower()))
            dw = set(_WORD.findall(d.lower()))
            out.append(len(qw & dw) / len(qw) if qw else 0.0)
        return out


class YesNoReranker:
    """A causal-LM reranker scored by the yes/no next-token logits.

    The pair is rendered into the fixed prompt in `models`, left-padded, and the score is
    softmax([no, yes])[yes] at the last position. Training uses `margin_logits` on the same
    code path so train and eval cannot drift apart.
    """

    def __init__(
        self,
        path: str,
        device: str | None = None,
        max_length: int = 512,
        instruction: str = models.RERANK_INSTRUCTION,
        batch_size: int = 8,
        dtype=None,
        model=None,
        tokenizer=None,
    ):
        from transformers import AutoModelForCausalLM, AutoTokenizer

        self.device = device or pick_device()
        self.tok = tokenizer or AutoTokenizer.from_pretrained(path, padding_side="left")
        self.tok.padding_side = "left"
        self.model = model or AutoModelForCausalLM.from_pretrained(path, dtype=dtype)
        self.model.to(self.device)
        self.max_length = max_length
        self.instruction = instruction
        self.batch_size = batch_size
        self.yes = self.tok.convert_tokens_to_ids("yes")
        self.no = self.tok.convert_tokens_to_ids("no")
        self.prefix = self.tok.encode(models.RERANK_PREFIX, add_special_tokens=False)
        self.suffix = self.tok.encode(models.RERANK_SUFFIX, add_special_tokens=False)

    def _inputs(self, pairs: Sequence[tuple[str, str]]):
        import torch

        room = max(8, self.max_length - len(self.prefix) - len(self.suffix))
        rows = []
        for q, d in pairs:
            body = models.format_rerank_body(self.instruction, q, d)
            ids = self.tok.encode(body, add_special_tokens=False)[:room]
            rows.append(self.prefix + ids + self.suffix)
        width = max(len(r) for r in rows)
        pad = self.tok.pad_token_id if self.tok.pad_token_id is not None else 0
        ids = torch.tensor([[pad] * (width - len(r)) + r for r in rows])
        mask = torch.tensor([[0] * (width - len(r)) + [1] * len(r) for r in rows])
        return ids.to(self.device), mask.to(self.device)

    def two_way_logits(self, pairs: Sequence[tuple[str, str]]):
        """(batch, 2) logits for [no, yes] at the answer position (differentiable)."""
        ids, mask = self._inputs(pairs)
        out = self.model(input_ids=ids, attention_mask=mask, logits_to_keep=1).logits[:, -1, :]
        return out[:, [self.no, self.yes]]

    def margin_logits(self, pairs: Sequence[tuple[str, str]]):
        two = self.two_way_logits(pairs)
        return two[:, 1] - two[:, 0]

    def score_pairs(self, pairs: Sequence[tuple[str, str]]) -> list[float]:
        import torch

        self.model.eval()
        scores: list[float] = []
        with torch.no_grad():
            for i in range(0, len(pairs), self.batch_size):
                two = self.two_way_logits(pairs[i : i + self.batch_size]).float()
                scores += torch.softmax(two, dim=1)[:, 1].cpu().tolist()
        return scores
