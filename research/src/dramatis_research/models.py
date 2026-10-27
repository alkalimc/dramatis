"""Default models and the prompt formats they expect.

Both defaults are Apache-2.0 licensed Qwen3 models. The embedding model uses last-token pooling
and expects an instruction in front of queries (documents carry none). The reranker is a causal
language model, not a classic cross-encoder: a pair is rendered into a fixed chat-style prompt
and scored by the model's next-token preference for "yes" over "no".
"""

from __future__ import annotations

EMBED_MODEL = "Qwen/Qwen3-Embedding-0.6B"  # Apache-2.0
RERANK_MODEL = "Qwen/Qwen3-Reranker-0.6B"  # Apache-2.0

QUERY_INSTRUCTION = "Given a web search query, retrieve relevant passages that answer the query"
RERANK_INSTRUCTION = QUERY_INSTRUCTION

RERANK_PREFIX = (
    "<|im_start|>system\nJudge whether the Document meets the requirements based on the Query "
    'and the Instruct provided. Note that the answer can only be "yes" or "no".<|im_end|>\n'
    "<|im_start|>user\n"
)
RERANK_SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"


def format_query(instruction: str, query: str) -> str:
    """Query text as the embedding model expects it; an empty instruction means no prefix."""
    return f"Instruct: {instruction}\nQuery:{query}" if instruction else query


def format_rerank_body(instruction: str, query: str, doc: str) -> str:
    return f"<Instruct>: {instruction}\n<Query>: {query}\n<Document>: {doc}"
