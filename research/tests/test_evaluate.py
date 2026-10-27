import json

import numpy as np
import pytest

from dramatis_research import ablate, data, evaluate, report
from dramatis_research.cli import main
from dramatis_research.encoders import HashingEmbedder, OverlapReranker
from dramatis_research.evaluate import Backends, SystemSpec
from dramatis_research.train import TrainConfig


def backends(runs_dir):
    return Backends(lambda p, i: HashingEmbedder(), lambda p: OverlapReranker(), runs_dir)


def fake_run(runs_dir, run_id, gpu=0.5, calls=7):
    d = runs_dir / run_id
    d.mkdir(parents=True)
    (d / "run.json").write_text(
        json.dumps(
            {
                "model_dir": "model",
                "config": {"query_instruction": ""},
                "train.gpu_hours": gpu,
                "data": {"llm_calls_for_data": calls},
            }
        )
    )


def test_sets_are_scored_separately(root):
    res = evaluate.evaluate(root, backends(root / "runs"), list(evaluate.DEFAULT_SYSTEMS))
    assert set(res["sets"]) == {"structural", "synthetic"}  # verify absent: not invented
    for block in res["sets"].values():
        assert set(block["systems"]) == {"bm25", "base-embed", "base-embed+rerank"}
    assert "macro" not in res and "overall" not in json.dumps(res)
    assert res["sets"]["structural"]["queries"] == 3
    assert res["sets"]["synthetic"]["queries"] == 6


def test_structural_set_prefers_the_engine_format_file(root):
    sets = evaluate.load_sets(root, data.load_units(root))
    assert sets["structural"][0].qid.startswith("redirect:")
    (root / "eval.structural.test.jsonl").unlink()
    sets = evaluate.load_sets(root, data.load_units(root))
    assert len(sets["structural"]) == 9  # falls back to the held-out pairs


def test_bm25_solves_the_lexical_fixture(root):
    res = evaluate.evaluate(root, backends(root / "runs"), [])
    fam = res["sets"]["structural"]["systems"]["bm25"]["by_family"]["redirect"]
    assert fam["queries"] == 3 and fam["recall@10"] > 0


def test_rerank_only_reorders_the_head():
    class Reverse:
        def score_pairs(self, pairs):
            return list(range(len(pairs)))  # last candidate scores highest

    from dramatis_research.metrics import Query

    out = evaluate.rerank_rankings(
        Reverse(),
        [["a", "b", "c", "d"]],
        [Query("q", "f", "t", {"a": 2})],
        dict.fromkeys("abcd", "x"),
        2,
    )
    assert out == [["b", "a", "c", "d"]]


def test_top_k_orders_by_similarity():
    sims = np.array([[0.1, 0.9, 0.5, 0.7]])
    assert evaluate.top_k(sims, 3).tolist() == [[1, 3, 2]]


def test_subsample_keeps_every_family():
    from dramatis_research.metrics import Query

    qs = [Query(str(i), "big", "t", {"a": 1}) for i in range(100)] + [Query("s", "small", "t", {})]
    out = evaluate.subsample(qs, 10)
    assert {q.family for q in out} == {"big", "small"} and len(out) <= 11


def test_cost_columns_come_from_run_json(root):
    runs = root / "runs"
    fake_run(runs, "r1", gpu=0.25, calls=9)
    fake_run(runs, "r2", gpu=0.5, calls=0)
    cost = evaluate.system_cost(runs, SystemSpec("x", "r1", "r2"))
    assert cost == {"llm_calls_for_data": 9, "gpu_hours": 0.75}
    assert evaluate.system_cost(runs, SystemSpec("b", "base", None)) == {
        "llm_calls_for_data": 0,
        "gpu_hours": 0.0,
    }
    fake_run(runs, "r3", calls=None)
    assert evaluate.system_cost(runs, SystemSpec("u", "r3"))["llm_calls_for_data"] is None


def test_system_spec_parsing():
    assert SystemSpec.parse("a=base,none") == SystemSpec("a", "base", None)
    assert SystemSpec.parse("a=run1,run2") == SystemSpec("a", "run1", "run2")
    with pytest.raises(ValueError):
        SystemSpec.parse("oops")


def test_engine_json_is_imported_for_the_structural_set_only(root, tmp_path):
    agg = {
        "queries": 3,
        "ndcg": 0.5,
        "precision": 0,
        "recall": 0.6,
        "mrr": 0.4,
        "negative_win_rate": 0.1,
    }
    path = tmp_path / "engine.json"
    path.write_text(json.dumps({"by_family": {"redirect": agg}, "macro_average": agg, "scored": 3}))
    res = evaluate.evaluate(root, backends(root / "runs"), [], engine_json=path)
    assert res["sets"]["structural"]["systems"]["bm25-engine"]["macro"]["ndcg@10"] == 0.5
    assert "bm25-engine" not in res["sets"]["synthetic"]["systems"]


def test_windowed_pool_evaluates_with_remapped_gold(root):
    res = evaluate.evaluate(root, backends(root / "runs"), [], window=60)
    assert res["meta"]["pool"] == "windows"
    assert res["sets"]["structural"]["queries"] == 3
    assert res["sets"]["structural"]["systems"]["bm25"]["macro"]["recall@10"] > 0


def test_report_has_a_section_per_set_and_marks_the_floor(root):
    runs = root / "runs"
    fake_run(runs, "r1")
    res = evaluate.evaluate(root, backends(runs), [SystemSpec("ft", "r1")], lexical_floor=0.0)
    md = report.render(res)
    assert "## Held-out structural set" in md and "## Held-out synthetic set" in md
    assert "| ft |" in md and "LLM calls (data)" in md and "GPU hours" in md
    assert "redirect *" in md
    assert "combined" not in md.replace("no combined score", "")
    md_dir = root / "out"
    mdp, jsp = report.write(res, md_dir)
    assert mdp.name == "RESULTS-research.md" and json.loads(jsp.read_text())["sets"]


def test_cli_evaluate_dry_run_writes_results(root, capsys):
    main(["evaluate", "--root", str(root), "--dry"])
    assert (root / "runs" / "RESULTS-research.md").exists()
    assert (root / "runs" / "RESULTS-research.json").exists()


def test_cli_windows_writes_a_unit_file(root, capsys):
    main(["windows", "--root", str(root), "--size", "50"])
    assert (root / "units.windows50.jsonl").exists()


def base_cfg(root):
    return TrainConfig(root=root, kind="embed")


def test_ablation_plan_covers_the_four_questions(root):
    entries = ablate.plan(base_cfg(root), sizes=(5, 10), window=50)
    by = {e.ablation for e in entries}
    assert by == {"main", "drop-source", "negatives", "size", "windows"}
    drops = {e.cfg.drop_families[0] for e in entries if e.ablation == "drop-source"}
    assert drops == {"redirect", "section", "speaker"}
    assert {e.cfg.limit for e in entries if e.ablation == "size"} == {5, 10}
    assert {e.cfg.negatives for e in entries if e.ablation == "negatives"} == {"random"}
    (win,) = [e for e in entries if e.ablation == "windows"]
    assert win.cfg.window == 50 == win.pool_window


def test_ablation_flags_change_the_selected_data(root):
    from dramatis_research.train import prepare

    plan = {e.system: e for e in ablate.plan(base_cfg(root), sizes=(5,), window=50)}
    ref, _ = prepare(plan["ft-structural"].cfg)
    drop, _ = prepare(plan["ft-structural-drop-speaker"].cfg)
    rand, _ = prepare(plan["ft-structural-random-neg"].cfg)
    small, _ = prepare(plan["ft-structural-n5"].cfg)
    win, w = prepare(plan["ft-structural-win50"].cfg)
    both, _ = prepare(plan["ft-both"].cfg)
    assert len(drop.examples) < len(ref.examples)
    assert [e.negatives for e in rand.examples] != [e.negatives for e in ref.examples]
    assert len(small.examples) == 5
    assert all(p.positive in w.windows for p in win.pairs)
    assert len(both.examples) == len(ref.examples) + 10
    assert len({s.digest for s in (ref, drop, rand, small, win, both)}) == 6


def test_ablation_without_synthetic_data_skips_synthetic_runs(root):
    (root / "pairs.synth.train.jsonl").unlink()
    systems = {e.system for e in ablate.plan(base_cfg(root), sizes=(5,))}
    assert "ft-synthetic" not in systems and "ft-both" not in systems


def test_ablation_run_trains_missing_runs_and_writes_both_reports(root):
    trained = []

    def fake_train(cfg):
        from dramatis_research.train import make_run_id

        run_id = make_run_id(cfg)
        fake_run(root / "runs", run_id)
        trained.append(run_id)

    cfg = base_cfg(root)
    entries = ablate.plan(cfg, sizes=(5,), window=50)
    written = ablate.run(cfg, backends, entries=entries, train=fake_train, rerank_depth=5)
    assert len(trained) == len(entries)
    assert written["typed"].exists() and written["windows"].exists()
    first = len(trained)
    ablate.run(cfg, backends, entries=entries, train=fake_train, rerank_depth=5)
    assert len(trained) == first  # existing runs are reused
