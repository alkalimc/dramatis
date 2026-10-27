"""Shared fixtures: a tiny export in the exact on-disk format, and tiny offline models."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

os.environ["DRAMATIS_RESEARCH_DEVICE"] = "cpu"

TRAIN_PAGES = ["alpha", "bravo", "charlie", "delta", "echo"]
TEST_PAGES = ["foxtrot", "golf", "hotel"]
PERSON = {
    "alpha": "ann",
    "bravo": "ann",
    "charlie": "bo",
    "delta": "bo",
    "echo": "cy",
    "foxtrot": "dee",
    "golf": "dee",
    "hotel": "eli",
}
TEMPLATES = ["lore", "line", "line"]


def unit_id(page: str, j: int) -> str:
    return f"{TEMPLATES[j]}:{page}{j}"


def make_units() -> list[dict]:
    rows = []
    for page, person in PERSON.items():
        for j in range(3):
            rows.append(
                {
                    "id": unit_id(page, j),
                    "header": f"{page} part {j}" if j == 0 else "",
                    "text": f"{page} chapter {j} keyword{page}{j} tells of {person} and the harbor",
                    "template": TEMPLATES[j],
                    "page": page,
                    "persons": [person],
                }
            )
    return rows


def make_pairs(pages: list[str], split: str) -> list[dict]:
    rows = []
    for page in pages:
        for j in range(3):
            rows.append(
                {
                    "query": f"find keyword{page}{j}",
                    "positive": unit_id(page, j),
                    "negatives": [unit_id(page, k) for k in range(3) if k != j],
                    "family": ["redirect", "section", "speaker"][j],
                    "split": split,
                    "page": page,
                    "persons": [PERSON[page]],
                }
            )
    return rows


def make_synth(pages: list[str], split: str) -> list[dict]:
    return [
        {
            "query": f"what does {page} say about chapter {j}",
            "positive": unit_id(page, j),
            "negatives": [],
            "family": "synthetic",
            "split": split,
            "page": page,
            "persons": [PERSON[page]],
        }
        for page in pages
        for j in range(2)
    ]


def make_eval(pages: list[str]) -> list[dict]:
    return [
        {
            "qid": f"redirect:{page}",
            "family": "redirect",
            "text": f"find keyword{page}0",
            "stratum": "paraphrase",
            "gold": {unit_id(page, 0): 2, unit_id(page, 1): 1},
            "hard_negatives": [unit_id(page, 2)],
        }
        for page in pages
    ]


def write_jsonl(path: Path, rows: list[dict]) -> None:
    path.write_text("".join(json.dumps(r) + "\n" for r in rows))


@pytest.fixture
def root(tmp_path: Path) -> Path:
    write_jsonl(tmp_path / "units.jsonl", make_units())
    write_jsonl(tmp_path / "pairs.train.jsonl", make_pairs(TRAIN_PAGES, "train"))
    write_jsonl(tmp_path / "pairs.test.jsonl", make_pairs(TEST_PAGES, "test"))
    write_jsonl(tmp_path / "pairs.synth.train.jsonl", make_synth(TRAIN_PAGES, "train"))
    write_jsonl(tmp_path / "pairs.synth.test.jsonl", make_synth(TEST_PAGES, "test"))
    write_jsonl(tmp_path / "eval.structural.test.jsonl", make_eval(TEST_PAGES))
    return tmp_path


def _word_tokenizer(words: list[str], extra: list[str]):
    from tokenizers import Tokenizer, models, normalizers, pre_tokenizers
    from transformers import PreTrainedTokenizerFast

    specials = ["[PAD]", "[UNK]", "[CLS]", "[SEP]"]
    vocab = {w: i for i, w in enumerate(dict.fromkeys(specials + extra + words))}
    tok = Tokenizer(models.WordLevel(vocab, unk_token="[UNK]"))
    tok.normalizer = normalizers.Lowercase()
    tok.pre_tokenizer = pre_tokenizers.WhitespaceSplit()
    return PreTrainedTokenizerFast(
        tokenizer_object=tok,
        unk_token="[UNK]",
        pad_token="[PAD]",
        cls_token="[CLS]",
        sep_token="[SEP]",
    ), len(vocab)


def _vocab_words(root: Path) -> list[str]:
    words: set[str] = set()
    for name in ("units.jsonl", "pairs.train.jsonl", "pairs.synth.train.jsonl"):
        for line in (root / name).read_text().splitlines():
            row = json.loads(line)
            words.update((row.get("text") or row.get("query") or "").lower().split())
    return sorted(words)


@pytest.fixture
def tiny_embed(root: Path, tmp_path_factory) -> str:
    """A randomly initialised 1-layer encoder with a word-level tokenizer."""
    from sentence_transformers import SentenceTransformer
    from sentence_transformers.sentence_transformer import modules
    from transformers import BertConfig, BertModel

    path = tmp_path_factory.mktemp("tiny-embed")
    tok, size = _word_tokenizer(_vocab_words(root), [])
    cfg = BertConfig(
        vocab_size=size,
        hidden_size=16,
        num_hidden_layers=1,
        num_attention_heads=2,
        intermediate_size=32,
        max_position_embeddings=128,
    )
    BertModel(cfg).save_pretrained(path / "hf")
    tok.save_pretrained(path / "hf")
    model = SentenceTransformer(
        modules=[modules.Transformer(str(path / "hf")), modules.Pooling(16, "mean")]
    )
    model.save(str(path / "st"))
    return str(path / "st")


@pytest.fixture
def tiny_rerank(root: Path, tmp_path_factory) -> str:
    """A randomly initialised causal LM with single-token `yes` and `no`."""
    from transformers import Qwen3Config, Qwen3ForCausalLM

    path = tmp_path_factory.mktemp("tiny-rerank")
    tok, size = _word_tokenizer(_vocab_words(root), ["yes", "no"])
    cfg = Qwen3Config(
        vocab_size=size,
        hidden_size=16,
        intermediate_size=32,
        num_hidden_layers=1,
        num_attention_heads=2,
        num_key_value_heads=1,
        head_dim=8,
        max_position_embeddings=512,
    )
    Qwen3ForCausalLM(cfg).save_pretrained(path)
    tok.save_pretrained(path)
    return str(path)
