"""`forge persona`, registered onto the forge CLI by `register`."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Annotated

import typer
from rich.console import Console

from ..config import Paths
from ..pack import Pack
from .command import UsageError, execute
from .run import ERROR, FAILED


def register(app: typer.Typer, *, resolve: Callable[[str, Path | None], tuple[Pack, Paths]],
             die: Callable[[str, str], None], console: Console) -> None:
    @app.command()
    def persona(
        pack: Annotated[str, typer.Option("--pack", "-p",
                                          help="Pack to use (or set DRAMATIS_PACK).")] = "",
        home: Annotated[Path | None, typer.Option(
            "--home", help="Artifact root (default: <workspace>/artifacts).")] = None,
        probe: Annotated[bool, typer.Option(
            "--probe", help="Only the probe set: thickest three, thinnest two.")] = False,
        only: Annotated[list[str] | None, typer.Option(
            "--only", help="One subject (person id, display name, alias, or `host`); "
                           "repeatable.")] = None,
        force: Annotated[bool, typer.Option(
            "--force", help="Call again even where the current version already passed.")]
            = False,
        endpoints: Annotated[Path | None, typer.Option(
            "--endpoints", help="Endpoints file (default: $DRAMATIS_HOME/endpoints.toml).")]
            = None,
    ) -> None:
        """Network: generate personas with the `persona` endpoint role, gate them, and
        write the passing ones into the folio's prompts table.

        Resumable: a subject whose rows carry the current generator version and still pass
        the gates is skipped, and a stored reply for the current version is reused without
        a call. Reports go to <home>/<pack>/persona/; with --probe, one readable file per
        subject goes to persona/probe/.
        """
        pk, paths = resolve(pack, home)
        tick = lambda m: console.print(f"  [dim]persona[/dim] {m}", highlight=False)  # noqa: E731
        try:
            result = execute(pk, paths, probe=probe, only=only, force=force,
                             endpoints_path=endpoints, progress=tick)
        except UsageError as exc:
            die(str(exc), exc.hint)
            return
        rep = result.report
        console.print(f"generator [bold]{rep.version}[/bold] · "
                      + " · ".join(f"{k} {v}" for k, v in sorted(rep.by_status().items())))
        console.print(f"persona.meta_leak {rep.meta_leak} · prescriptive.rate "
                      f"{rep.prescriptive_rate} · roster without a persona {len(rep.missing)} "
                      f"(skipped, no words of their own: {len(rep.skipped)})")
        for path in result.written[:2]:
            console.print(f"[cyan]{path}[/cyan]")
        if probe:
            console.print(f"[cyan]{paths.pack_dir / 'persona' / 'probe'}[/cyan]")
        if any(o.status in (FAILED, ERROR) for o in rep.outcomes):
            raise typer.Exit(1)
