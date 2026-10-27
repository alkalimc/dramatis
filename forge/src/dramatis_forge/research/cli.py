"""`forge research`, registered onto the forge CLI by `register`."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Annotated

import typer
from rich.console import Console

from ..config import Paths
from ..pack import Pack
from . import export, pairs, split

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
