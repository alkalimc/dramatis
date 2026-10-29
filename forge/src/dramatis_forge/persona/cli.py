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
            "--home", help="Artifact root (default: <workspace>/dramatis-artifacts).")] = None,
        probe: Annotated[bool, typer.Option(
            "--probe", help="Only the probe set: thickest three, thinnest two.")] = False,
        only: Annotated[list[str] | None, typer.Option(
            "--only", help="One subject (person id, display name, alias, or `host`); "
                           "repeatable.")] = None,
        force: Annotated[bool, typer.Option(
            "--force", help="Call again even where the current version already passed.")]
            = False,
        stale: Annotated[bool, typer.Option(
            "--stale", help="No network: list personas whose source material has changed "
                            "since they were written, in persona/STALE.md.")] = False,
        regenerate: Annotated[bool, typer.Option(
            "--regenerate", help="With --stale: call again for the stale ones only.")] = False,
        jobs: Annotated[int | None, typer.Option(
            "--jobs", "-j", min=1,
            help="Model calls in flight at once. Default: 3 when the persona model is "
                 "billed per request, else 1. On HTTP 429 every worker waits together.")]
            = None,
        endpoints: Annotated[Path | None, typer.Option(
            "--endpoints", help="Endpoints file (default: $DRAMATIS_HOME/endpoints.toml).")]
            = None,
    ) -> None:
        """Network: generate personas with the `persona` endpoint role, gate them, and
        write the passing ones into the folio's prompts table.

        Resumable: a subject whose rows carry the current generator version and still pass
        the gates is skipped, and a stored reply for the current version is reused without
        a call. With --stale, nothing is generated: the personas are compared with the
        folio and persona/STALE.md lists those whose sent material changed; add
        --regenerate to redo exactly those. Reports go to <home>/<pack>/persona/; with
        --probe, one readable file per subject goes to persona/probe/.
        """
        pk, paths = resolve(pack, home)
        tick = lambda m: console.print(f"  [dim]persona[/dim] {m}", highlight=False)  # noqa: E731
        try:
            result = execute(pk, paths, probe=probe, only=only, force=force,
                             stale=stale, regenerate=regenerate, endpoints_path=endpoints,
                             progress=tick, jobs=jobs)
        except UsageError as exc:
            die(str(exc), exc.hint)
            return
        found = result.stale
        if found is not None:
            console.print(f"stale {len(found.stale)} · new material only "
                          f"{len(found.new_only)} · never generated {len(found.never)} · "
                          f"no source record {len(found.no_record)} · up to date {found.current}")
            if not regenerate:
                console.print(f"[cyan]{result.written[0]}[/cyan]")
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
