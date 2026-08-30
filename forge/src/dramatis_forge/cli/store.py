"""Store commands: migration from a previous-generation archive."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer

from ..store.migrate import from_legacy
from .common import HomeOpt, PackOpt, console, resolve

app = typer.Typer(no_args_is_help=True)


@app.command("migrate")
def migrate_cmd(
    legacy: Annotated[Path, typer.Argument(help="Path to the previous-generation archive.")],
    pack: PackOpt = "arknights",
    home: HomeOpt = None,
    cache: Annotated[Path | None, typer.Option("--cache", help="Its page cache.")] = None,
) -> None:
    """Carry inputs forward from an older archive, then re-normalise.

    Only inputs move: page bodies, seed sets, structured tables, alias sources. Records
    are re-derived, because copying them would preserve whatever the old rules got wrong
    while making it look like fresh output.

    The alternative is re-fetching, which means thousands of requests to a volunteer-run
    site for text already sitting on disk.
    """
    pk, paths = resolve(pack, home)
    result = from_legacy(legacy, paths.archive, pk, legacy_cache=cache)

    console.print(f"carried [bold]{result.pages:,}[/bold] page bodies")
    console.print(f"seed sets: {result.seeds}")
    console.print(f"structured tables: {result.tables}")
    console.print(f"alias sources: {result.redirects:,} redirects, {result.disambigs:,} disambiguations")
    for note in result.notes:
        console.print(f"  [yellow]·[/yellow] {note}")
    console.print(f"wrote [cyan]{paths.archive}[/cyan]")
    console.print("  next: [bold]dramatis-forge normalize run[/bold]")
