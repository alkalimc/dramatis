"""`forge research`, registered onto the forge CLI by `register`."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Annotated

import typer
from rich.console import Console

from ..config import Paths
from ..pack import Pack
from . import export, pairs, split, synth, verify

PackOpt = Annotated[str, typer.Option("--pack", "-p", help="Pack to use (or set DRAMATIS_PACK).")]
HomeOpt = Annotated[Path | None, typer.Option(
    "--home", help="Artifact root (default: <workspace>/artifacts).")]
OutOpt = Annotated[Path | None, typer.Option(
    "--out", help="Research directory (default: <home>/<pack>/research).")]


def register(app: typer.Typer, *, resolve: Callable[[str, Path | None], tuple[Pack, Paths]],
             die: Callable[[str, str], None], console: Console) -> None:
    group = typer.Typer(no_args_is_help=True)
    app.add_typer(group, name="research",
                  help="Research phase: structural training pairs, synthetic queries, "
                       "a verification set. Never changes product defaults.")

    def folio_and_out(pack: str, home: Path | None, out: Path | None) -> tuple[Paths, Path]:
        _, paths = resolve(pack, home)
        if not paths.folio.exists():
            die(f"no folio at {paths.folio}", "run `forge build` first")
        return paths, out or paths.pack_dir / "research"

    @group.command("pairs")
    def pairs_cmd(
        pack: PackOpt = "", home: HomeOpt = None, out: OutOpt = None,
        seed: Annotated[int, typer.Option(help="Split seed.")] = split.DEFAULT_SEED,
        test_share: Annotated[float, typer.Option(
            help="Share of pages held out as test; their persons are held out too.")]
            = split.DEFAULT_TEST_SHARE,
        split_mode: Annotated[str, typer.Option(
            help="`page`: hash the pages, test persons follow. `person`: hash the persons, "
                 "a page is test when all its persons are.")] = split.DEFAULT_MODE,
        negatives: Annotated[int, typer.Option(min=0, help="Hard negatives per pair.")] = 8,
        group_facet: Annotated[str | None, typer.Option(
            help="Person attribute that defines the same group for identity negatives "
                 "(default: most shared attributes).")] = None,
        max_per_family: Annotated[int, typer.Option(
            min=0, help="Deterministic cap per family before the split; 0 keeps all.")] = 0,
    ) -> None:
        """Offline: write structural pairs, the held-out structural set and the manifest."""
        paths, outdir = folio_and_out(pack, home, out)
        try:
            summary = export.run(paths.folio, outdir, seed=seed, share=test_share, mode=split_mode,
                                 params=pairs.Params(negatives, group_facet, max_per_family))
        except (split.LeakError, ValueError) as exc:
            die(str(exc), "")
            return
        for fam, n in summary.counts().items():
            console.print(f"{fam}: train {n.get('train', 0)} · test {n.get('test', 0)}")
        for path in summary.paths:
            console.print(f"[cyan]{path}[/cyan]")

    @group.command("synth")
    def synth_cmd(
        side: Annotated[str, typer.Option("--split", help="train or test.")],
        pack: PackOpt = "", home: HomeOpt = None, out: OutOpt = None,
        per_unit: Annotated[int, typer.Option(min=1, help="Queries asked per unit.")] = 3,
        limit: Annotated[int, typer.Option(min=0, help="At most this many units; 0 = all.")] = 0,
        template: Annotated[list[str] | None, typer.Option(
            help="Only units of this template; repeatable.")] = None,
        top_k: Annotated[int, typer.Option(min=1, help="Gold must be in BM25's top k.")] = 10,
        copy_ratio: Annotated[float, typer.Option(
            help="Drop a query whose longest verbatim run covers this share of it.")] = 0.6,
        jobs: Annotated[int | None, typer.Option(
            "--jobs", "-j", min=1, help="Model calls in flight at once. Default: 3 when the "
                                        "persona model is billed per request, else 1.")] = None,
        endpoints: Annotated[Path | None, typer.Option(
            "--endpoints", help="Endpoints file (default: $DRAMATIS_HOME/endpoints.toml).")]
            = None,
        dry_run: Annotated[bool, typer.Option(
            "--dry-run", help="No calls: print sample prompts and the counts.")] = False,
        show: Annotated[int, typer.Option(help="Prompts printed by --dry-run.")] = 2,
    ) -> None:
        """Network: write synthetic queries for the units of one split with the `persona`
        endpoint role, filter them, and write pairs.synth.<split>.jsonl.

        Run `pairs` first (the split decides which units are eligible). Replies are cached
        in synth.cache.jsonl, so a rerun makes no repeated request.
        """
        paths, outdir = folio_and_out(pack, home, out)
        if side not in ("train", "test"):
            die("--split must be train or test", "")
            return
        params = synth.Params(per_unit=per_unit, top_k=top_k, copy_ratio=copy_ratio,
                              limit=limit, templates=tuple(template or ()))
        client, model = None, ""
        if not dry_run:
            from ..persona.client import Client
            from ..persona.command import REQUEST_JOBS
            from ..persona.endpoints import Endpoints, EndpointsError, persona_target
            from ..persona.params import Persona
            try:
                eps = Endpoints.load(endpoints)
                target = persona_target(eps)
            except EndpointsError as exc:
                die(str(exc), exc.hint)
                return
            per_request = eps.per_request(target.role)
            knobs = Persona.load(paths.home / "params.toml").billed(per_request)
            jobs = jobs or (REQUEST_JOBS if per_request else 1)
            model = target.role.model
            client = Client(target, retries=knobs.retries,
                            rate_limit_wait_s=knobs.rate_limit_wait_s,
                            on_limit=lambda m: console.print(f"  [dim]{m}[/dim]"))
        try:
            rep = synth.run(paths.folio, outdir, side, params=params, client=client,
                            model=model, jobs=jobs or 1, dry_run=dry_run, show=show,
                            progress=lambda m: console.print(f"  [dim]synth[/dim] {m}"))
        except (FileNotFoundError, split.LeakError, ValueError) as exc:
            die(str(exc), "")
            return
        console.print(f"units {rep.units} · requests {rep.calls} · cached {rep.cached}")
        if dry_run:
            for system, user in rep.samples:
                console.print("--- system", style="dim")
                console.print(system, markup=False, highlight=False)
                console.print("--- user", style="dim")
                console.print(user, markup=False, highlight=False)
            return
        console.print(f"generated {rep.generated} · kept {rep.kept} · failed calls "
                      f"{rep.failed} · dropped {dict(rep.dropped)}")
        console.print(f"tokens {dict(rep.usage)}")
        for line in rep.errors[:5]:
            console.print(f"[red]{line}[/red]", highlight=False)
        console.print(f"[cyan]{rep.path}[/cyan]")
        if rep.failed:
            raise typer.Exit(1)

    @group.command("verify-sample")
    def verify_sample(
        pack: PackOpt = "", home: HomeOpt = None, out: OutOpt = None,
        per_family: Annotated[int, typer.Option(min=1, help="Queries drawn per family.")] = 10,
        seed: Annotated[int, typer.Option(help="Sampling seed.")] = 1,
    ) -> None:
        """Offline: draw a small stratified sample of test queries for hand verification."""
        _, outdir = folio_and_out(pack, home, out)
        try:
            path, counts = verify.sample(outdir, per_family=per_family, seed=seed)
        except (verify.VerifyError, FileNotFoundError) as exc:
            die(str(exc), "")
            return
        console.print(" · ".join(f"{k} {v}" for k, v in sorted(counts.items())))
        console.print(f"[cyan]{path}[/cyan]")

    @group.command("verify-read")
    def verify_read(
        pack: PackOpt = "", home: HomeOpt = None, out: OutOpt = None,
        file: Annotated[Path | None, typer.Option(
            help="The edited todo file (default: <research>/verify.todo.jsonl).")] = None,
    ) -> None:
        """Offline: turn the verified rows into eval.verified.test.jsonl."""
        _, outdir = folio_and_out(pack, home, out)
        try:
            path, seen = verify.read(outdir, file)
        except (verify.VerifyError, FileNotFoundError) as exc:
            die(str(exc), "")
            return
        console.print(" · ".join(f"{k} {v}" for k, v in sorted(seen.items())))
        console.print(f"[cyan]{path}[/cyan]")
