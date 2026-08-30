"""Report commands: coverage, and per-page inspection."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from ..report import coverage as coverage_mod
from ..report import inspect as inspect_mod
from ..store.archive import Archive
from .common import HomeOpt, PackOpt, console, die, need, resolve

app = typer.Typer(no_args_is_help=True)

_STYLE = {"archived": "green", "partial": "yellow", "excluded": "dim"}


@app.command("coverage")
def coverage_cmd(
    pack: PackOpt = "arknights",
    home: HomeOpt = None,
    out: Annotated[Path | None, typer.Option("--out", help="Where to write the report.")] = None,
) -> None:
    """What was taken from the source, what was not, and why not.

    Organised by the source's own table of contents, so a reader who knows the site can
    check it section by section. Listing only what was taken cannot answer "did we miss
    something", which is the only question anyone asks of a coverage report.
    """
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")
    destination = out or paths.coverage

    with Archive(paths.archive, readonly=True) as archive:
        markdown, rows = coverage_mod.build(archive, pk)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(markdown, encoding="utf-8")

    table = Table(title="coverage")
    table.add_column("section", overflow="fold")
    table.add_column("disposition")
    table.add_column("units", justify="right")
    for section, disposition, count, _reason in rows:
        style = _STYLE.get(disposition, "white")
        table.add_row(f"[{style}]{section}[/{style}]",
                      f"[{style}]{disposition}[/{style}]", count)
    console.print(table)
    console.print(f"full report, with a reason for every exclusion: [cyan]{destination}[/cyan]")


@app.command("inspect")
def inspect_cmd(
    title: Annotated[str, typer.Argument(help="Page title.")],
    pack: PackOpt = "arknights",
    home: HomeOpt = None,
    out: Annotated[Path | None, typer.Option("--out", help="Sample directory.")] = None,
) -> None:
    """Dump one page at every stage: source, records, archived text.

    Guards catch assumptions that fail loudly. They cannot catch a parse that is subtly
    wrong — a line attributed to the wrong speaker, a section that reads as nonsense
    because its heading was dropped. Only reading the output next to the input finds
    those, so the third file is rendered to be read.
    """
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")

    with Archive(paths.archive, readonly=True) as archive:
        written = inspect_mod.dump(archive, title, out or paths.samples)
    if not written:
        die(f"{title!r} is not in the archive",
            "check `dramatis-forge report coverage` for what is in scope")
    for path in written:
        console.print(f"[green]{path}[/green]  {path.stat().st_size:,} bytes")
