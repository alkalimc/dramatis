"""Evals command: build benchmark suites from the folio."""

from __future__ import annotations

from typing import Annotated

import typer
from rich.table import Table

from ..evals import structural
from ..store.folio import Folio
from .common import HomeOpt, PackOpt, console, need, resolve

app = typer.Typer(no_args_is_help=True)


@app.command("build")
def build_cmd(
    pack: PackOpt = "arknights",
    home: HomeOpt = None,
    max_per_family: Annotated[
        int, typer.Option("--cap", help="Cap queries per family (0 = no cap).")
    ] = 0,
) -> None:
    """Build the structural suite from relations the wiki's editors already wrote.

    No annotation budget and no model: the queries come from redirects, disambiguation
    pages, real-name tables, heading paths and voice triggers.
    """
    pk, paths = resolve(pack, home)
    need(paths.folio, "folio", "run `dramatis-forge corpus build` first")

    with Folio(paths.folio, readonly=True) as folio:
        suite = structural.build(folio, max_per_family=max_per_family)
        manifest = structural.write(suite, paths.evals, folio=folio)

    counts = manifest["counts"]
    table = Table(title="structural suite")
    table.add_column("family")
    table.add_column("what it tests", overflow="fold")
    table.add_column("queries", justify="right")
    table.add_column("verbatim", justify="right")
    table.add_column("hard negs", justify="right")
    for family in sorted(counts["by_family"]):
        strata = counts["strata_by_family"][family]
        total = sum(strata.values())
        verbatim = strata.get("verbatim", 0)
        negs = counts["hard_negatives"][family]
        table.add_row(
            family, structural.FAMILIES[family], f"{total:,}",
            f"{verbatim / total:.0%}" if total else "—",
            f"{negs['with_negatives'] / total:.0%}" if total else "—",
        )
    console.print(table)
    console.print(
        f"[bold]{counts['queries']:,}[/bold] graded queries · annotation cost [green]0[/green]")
    console.print(
        "[dim]report per family and per stratum; a family that is mostly verbatim is "
        "solvable by exact match and is a sanity floor, not evidence[/dim]")
    if counts["skipped_no_gold"]:
        console.print(f"[dim]skipped, no gold passage: {counts['skipped_no_gold']}[/dim]")
    console.print(f"wrote [cyan]{paths.evals}[/cyan]")
