"""`forge research synth` and the verification set, with a fake model endpoint."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import typer
from persona_support import ENDPOINTS, FakeEndpoint, key_lookup, offline  # noqa: F401
from research_support import build
from rich.console import Console
from typer.testing import CliRunner

from dramatis_forge.config import Paths
from dramatis_forge.persona.client import Client
from dramatis_forge.persona.endpoints import Endpoints, persona_target
from dramatis_forge.research import cli as research_cli
from dramatis_forge.research import export, split, synth, verify

SHARE = 0.4


def chat(queries: list[str]) -> dict:
    return {"choices": [{"message": {"content": json.dumps({"queries": queries})},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}}


def passage_of(body: dict) -> str:
    return body["messages"][-1]["content"]


def lore_reply(body: dict) -> dict:
    """Queries for a lore unit: one good paraphrase, one copy, one unfindable, one repeat."""
    text = passage_of(body)
    k = text.split("Lore")[1].split(" ")[0]
    j = text.split("Sec ")[1].split()[0]
    return chat([f"passage about section {j} lore {k}",
                 f"text of section {j} on lore {k}",
                 "zzzqqq nothing like it",
                 f"passage about section {j} lore {k}"])


@pytest.fixture
def research(tmp_path: Path):
    folio = build(tmp_path / "toy.folio")
    summary = export.run(folio, tmp_path / "research", share=SHARE)
    return folio, summary


def client(ep: FakeEndpoint) -> Client:
    target = persona_target(Endpoints.from_toml(ENDPOINTS.format(wire="chat")), key_lookup())
    return Client(target, retries=1, transport=ep.transport)


def run(research, side, ep, **kw):
    folio, summary = research
    params = synth.Params(templates=("lore",), **kw.pop("params", {}))
    return synth.run(folio, summary.outdir, side, params=params, client=client(ep),
                     model="model-a", **kw)


def test_filters_keep_only_retrievable_non_copies(research):
    ep = FakeEndpoint(lore_reply)
    rep = run(research, "test", ep, jobs=2)
    n = rep.units
    assert n and rep.calls == n and rep.failed == 0
    assert rep.generated == 4 * n
    assert rep.dropped["copy"] == n
    assert rep.dropped["not_retrievable"] == n
    assert rep.dropped["duplicate"] == n
    assert rep.kept == n
    rows = export.read_lines(rep.path)
    assert len(rows) == n
    units = {u["id"]: u for u in export.read_lines(research[1].outdir / export.UNITS)}
    sp = research[1].split
    for r in rows:
        assert r["family"] == "synthetic" and r["split"] == "test"
        assert r["page"] in sp.test_pages
        assert r["query"].startswith("passage about")
        assert r["positive"] not in r["negatives"]
        assert all(x in units for x in [r["positive"], *r["negatives"]])
    assert rep.usage["output"] == 5 * n


def test_train_side_obeys_the_split_and_the_cache_avoids_calls(research):
    ep = FakeEndpoint(lore_reply)
    rep = run(research, "train", ep)
    sp = research[1].split
    rows = export.read_lines(rep.path)
    assert rows
    units = {u["id"]: u for u in export.read_lines(research[1].outdir / export.UNITS)}
    split.check(sp, rows, units)
    first = len(ep.requests)
    again = run(research, "train", ep)
    assert len(ep.requests) == first and again.cached == again.units and again.kept == len(rows)


def test_dry_run_prints_prompts_and_calls_nothing(research):
    folio, summary = research
    rep = synth.run(folio, summary.outdir, "test", params=synth.Params(templates=("lore",)),
                    dry_run=True, show=2)
    assert rep.units and rep.calls == rep.units and len(rep.samples) == 2
    assert "Lore" in rep.samples[0][1] and rep.samples[0][0].startswith("You write search queries")
    assert not (summary.outdir / "pairs.synth.test.jsonl").exists()


def test_failed_call_is_counted_not_fatal(research):
    ep = FakeEndpoint((400, {"error": {"message": "bad"}}))
    rep = run(research, "test", ep)
    assert rep.failed == rep.units and rep.kept == 0 and rep.errors


def test_copy_share():
    assert synth.copy_share("alpha beta", "x alpha beta y") == 1.0
    assert synth.copy_share("alpha gamma", "x alpha beta y") < 0.6
    assert synth.copy_share("", "abc") == 0.0


def test_needs_the_split_file(tmp_path):
    folio = build(tmp_path / "toy.folio")
    with pytest.raises(FileNotFoundError):
        synth.run(folio, tmp_path / "none", "test", dry_run=True)


def test_verify_sample_and_read(research, tmp_path):
    outdir = research[1].outdir
    path, counts = verify.sample(outdir, per_family=2)
    rows = export.read_lines(path)
    assert set(counts) <= {"alias", "disambiguation", "section", "speaker", "identity"}
    assert all(n <= 2 for n in counts.values())
    assert all(r["verdict"] is None for r in rows)
    assert verify.sample(outdir, per_family=2)[1] == counts
    rows[0]["verdict"] = "ok"
    rows[0]["also_relevant"] = [rows[1]["positive"]] if rows[1]["positive"] != rows[0]["positive"] else []
    rows[1]["verdict"] = "fix"
    rows[1]["corrected_query"] = "a better query"
    rows[2]["verdict"] = "bad"
    export.write_lines(path, rows)
    out, seen = verify.read(outdir)
    assert dict(seen) == {"ok": 1, "fix": 1, "bad": 1, "unverified": len(rows) - 3}
    queries = export.read_lines(out)
    assert len(queries) == 2
    assert {q["text"] for q in queries} >= {"a better query"}
    for q in queries:
        assert q["gold"] and 2 in q["gold"].values()

    rows[3]["verdict"] = "maybe"
    export.write_lines(path, rows)
    with pytest.raises(verify.VerifyError):
        verify.read(outdir)
    rows[3]["verdict"] = "fix"
    export.write_lines(path, rows)
    with pytest.raises(verify.VerifyError, match="corrected_query"):
        verify.read(outdir)


def test_cli_dry_run_and_verify(research, tmp_path):
    folio, summary = research
    paths = Paths.for_pack("toy", tmp_path / "home")
    paths.folio.parent.mkdir(parents=True)
    paths.folio.write_bytes(folio.read_bytes())
    app = typer.Typer()

    def die(message: str, hint: str = "") -> None:
        raise typer.Exit(1)

    research_cli.register(app, resolve=lambda *_: (None, paths), die=die, console=Console())
    out = ["--out", str(summary.outdir)]
    res = CliRunner().invoke(app, ["research", "synth", "--split", "test", "--dry-run", *out])
    assert res.exit_code == 0, res.output
    assert "--- system" in res.output
    res = CliRunner().invoke(app, ["research", "verify-sample", "--per-family", "1", *out])
    assert res.exit_code == 0, res.output
    res = CliRunner().invoke(app, ["research", "verify-read", *out])
    assert res.exit_code == 0 and "unverified" in res.output
