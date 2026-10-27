import json

from dramatis_research import data, models
from dramatis_research.encoders import STEmbedder, YesNoReranker
from dramatis_research.train import TrainConfig, load_run, train_embed, train_rerank


def cfg(root, base, **kw):
    kw.setdefault("query_instruction", "")
    return TrainConfig(
        root=root, base=base, batch_size=4, max_seq_length=64, learning_rate=1e-3, **kw
    )


def test_train_embed_writes_model_and_run_json(root, tiny_embed):
    run_dir = train_embed(cfg(root, tiny_embed, pairs="both", limit=10, kind="embed"))
    info = load_run(run_dir.parent, run_dir.name)
    assert (run_dir / info["model_dir"]).is_dir()
    assert info["data"]["examples"] == 10
    assert set(info["data"]["inputs"]) >= {"units.jsonl", "pairs.train.jsonl"}
    assert info["data"]["inputs"]["units.jsonl"] == data.sha256_file(root / "units.jsonl")
    assert info["data"]["selection_digest"].startswith("sha256:")
    assert info["time"]["train_seconds"] > 0
    assert info["train.gpu_hours"] == 0.0 and info["train.cpu_hours"] > 0  # forced to CPU
    assert info["device"] == "cpu"
    # the fine-tuned checkpoint loads back and embeds
    emb = STEmbedder(str(run_dir / info["model_dir"]), device="cpu", query_instruction="")
    assert emb.encode_docs(["alpha chapter 0"]).shape[0] == 1


def test_run_json_records_llm_calls_for_synthetic_data(root, tiny_embed):
    (root / "cost.json").write_text(json.dumps({"synthetic_llm_calls": 42}))
    run_dir = train_embed(cfg(root, tiny_embed, pairs="synthetic", limit=4, kind="embed"))
    assert load_run(run_dir.parent, run_dir.name)["data"]["llm_calls_for_data"] == 42
    run_dir = train_embed(cfg(root, tiny_embed, pairs="structural", limit=4, kind="embed"))
    assert load_run(run_dir.parent, run_dir.name)["data"]["llm_calls_for_data"] == 0


def test_train_embed_is_seeded(root, tiny_embed):
    a = train_embed(cfg(root, tiny_embed, limit=6, seed=1, kind="embed", run_id="a"))
    b = train_embed(cfg(root, tiny_embed, limit=6, seed=1, kind="embed", run_id="b"))
    ea = STEmbedder(str(a / "model"), "cpu", query_instruction="").encode_docs(["alpha chapter 0"])
    eb = STEmbedder(str(b / "model"), "cpu", query_instruction="").encode_docs(["alpha chapter 0"])
    assert abs(ea - eb).max() < 1e-5


def test_train_refuses_a_leaky_export(root, tiny_embed):
    rows = data.read_jsonl(root / "pairs.train.jsonl")
    rows[0]["page"] = "foxtrot"
    (root / "pairs.train.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    import pytest

    with pytest.raises(data.LeakError):
        train_embed(cfg(root, tiny_embed, kind="embed"))
    with pytest.raises(data.LeakError):
        train_rerank(cfg(root, tiny_embed, kind="rerank"))


def test_embed_query_instruction_reaches_the_training_anchor(root):
    from dramatis_research.train import embed_datasets, prepare

    c = cfg(root, "unused", query_instruction="Find it", kind="embed")
    sel, _ = prepare(c)
    anchors = [a for ds in embed_datasets(c, sel).values() for a in ds["anchor"]]
    assert all(a.startswith("Instruct: Find it\nQuery:") for a in anchors)


def test_yes_no_reranker_scores_pairs(root, tiny_rerank):
    rr = YesNoReranker(tiny_rerank, device="cpu", max_length=64)
    scores = rr.score_pairs([("find keywordalpha0", "alpha chapter 0"), ("x", "y")])
    assert len(scores) == 2 and all(0.0 <= s <= 1.0 for s in scores)
    assert rr.yes != rr.no
    # batching must not change scores (left padding is handled)
    solo = rr.score_pairs([("x", "y")])[0]
    assert abs(solo - scores[1]) < 1e-4


def test_prompt_format_matches_the_model_card():
    body = models.format_rerank_body("inst", "q", "d")
    assert body == "<Instruct>: inst\n<Query>: q\n<Document>: d"
    assert models.format_query("inst", "q") == "Instruct: inst\nQuery:q"
    assert models.format_query("", "q") == "q"


def test_train_rerank_lowers_loss_and_writes_a_reloadable_model(root, tiny_rerank):
    c = cfg(root, tiny_rerank, kind="rerank", epochs=8, limit=6, hard_negatives=2)
    run_dir = train_rerank(c)
    info = load_run(run_dir.parent, run_dir.name)
    assert info["losses"]["last"] < info["losses"]["first"]
    rr = YesNoReranker(str(run_dir / info["model_dir"]), device="cpu", max_length=64)
    assert len(rr.score_pairs([("a", "b")])) == 1


def test_rerank_needs_negatives(root, tiny_rerank):
    import pytest

    with pytest.raises(ValueError, match="negative"):
        train_rerank(cfg(root, tiny_rerank, kind="rerank", pairs="synthetic"))


def test_lora_variants_train_and_reload(root, tiny_embed, tiny_rerank):
    import pytest

    pytest.importorskip("peft")
    rr_dir = train_rerank(
        cfg(root, tiny_rerank, kind="rerank", lora_rank=4, limit=6, hard_negatives=2)
    )
    YesNoReranker(str(rr_dir / "model"), device="cpu", max_length=64).score_pairs([("a", "b")])
    emb_dir = train_embed(cfg(root, tiny_embed, kind="embed", lora_rank=4, limit=6))
    STEmbedder(str(emb_dir / "model"), "cpu", query_instruction="").encode_docs(["x"])
