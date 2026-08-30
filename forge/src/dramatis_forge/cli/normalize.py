"""Normalize command. Offline: reads held page bodies, writes records."""

from __future__ import annotations

import typer

from ..config import RunInfo
from ..normalize.runner import run as normalize_run
from ..store.archive import Archive
from .common import HomeOpt, PackOpt, console, need, render_guards, resolve, ticker

app = typer.Typer(no_args_is_help=True)


@app.command("run")
def run_cmd(pack: PackOpt = "arknights", home: HomeOpt = None) -> None:
    """Wikitext to records, with the guards. No network access.

    This is the stage to re-run after changing a parse rule: it costs minutes and no
    bandwidth, which is what makes the rules safe to keep changing.
    """
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")
    need(paths.rawcache, "page cache", "run `dramatis-forge harvest fetch` first")

    with Archive(paths.archive) as archive:
        report = normalize_run(archive, pk, progress=ticker("normalize"))
        archive.set_meta("run_normalize", RunInfo.create("normalize", pk.name, pk.version).as_dict())

    from .harvest import _records_table

    _records_table(report)
    render_guards(report.ledger)
    console.print(f"wrote [cyan]{paths.archive}[/cyan]")
    if report.clean:
        console.print("  next: [bold]dramatis-forge corpus build[/bold]")
