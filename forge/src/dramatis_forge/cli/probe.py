"""Probe command: answer the design's open questions, or say why we cannot."""

from __future__ import annotations

from typing import Annotated

import typer
from rich.table import Table

from .. import probes
from .common import HomeOpt, PackOpt, console, resolve

app = typer.Typer(no_args_is_help=True)

_STATUS_STYLE = {
    probes.PASS: "green",
    probes.FAIL: "red",
    probes.BLOCKED: "yellow",
    probes.UNRUN: "dim",
}


@app.command("run")
def run_cmd(
    pack: PackOpt = "arknights",
    home: HomeOpt = None,
    only: Annotated[
        str | None, typer.Option("--only", help="Comma-separated probe ids, e.g. P1,P5.")
    ] = None,
) -> None:
    """Run every probe whose inputs exist; register the rest as blocked.

    A probe is not a test. A failing test says the code changed; a failing probe says a
    design document is claiming something untrue.
    """
    pk, paths = resolve(pack, home)
    ids = [x.strip().upper() for x in only.split(",")] if only else None

    results = probes.run_all(
        archive_path=paths.archive, folio_path=paths.folio, evals_dir=paths.evals,
        workdir=paths.pack_dir, pack=pk, only=ids,
        progress=lambda msg: console.print(f"  [dim]probe[/dim] {msg}"),
    )
    out = probes.write_results(results, paths.probes)

    table = Table(title="probes")
    table.add_column("id")
    table.add_column("status")
    table.add_column("question", overflow="fold")
    for pid in sorted(results, key=lambda x: int(x[1:])):
        probe = probes.REGISTER[pid]
        status = results[pid].status
        table.add_row(
            pid,
            f"[{_STATUS_STYLE[status]}]{status}[/{_STATUS_STYLE[status]}]",
            probe.title,
        )
    console.print(table)

    for pid in sorted(results, key=lambda x: int(x[1:])):
        result = results[pid]
        if result.status == probes.BLOCKED:
            continue
        console.print(f"\n[bold]{pid}[/bold] {probes.REGISTER[pid].title}")
        for note in result.notes:
            console.print(f"   [dim]·[/dim] {note}")
        for contradiction in result.contradicts:
            console.print(f"   [red]✗ contradicts:[/red] {contradiction}")

    tally = {s: sum(1 for r in results.values() if r.status == s) for s in _STATUS_STYLE}
    console.print(
        f"\n[green]{tally[probes.PASS]} pass[/green] · "
        f"[red]{tally[probes.FAIL]} fail[/red] · "
        f"[yellow]{tally[probes.BLOCKED]} blocked[/yellow]")
    console.print(f"wrote [cyan]{out}[/cyan]")


@app.command("list")
def list_cmd() -> None:
    """List the register: what each probe asks, and which decision it gates."""
    table = Table(title="probe register")
    table.add_column("id")
    table.add_column("title", overflow="fold")
    table.add_column("gates", overflow="fold")
    table.add_column("needs")
    for probe in probes.REGISTER:
        table.add_row(
            probe.id, probe.title, probe.gates,
            "[green]offline[/green]" if probe.offline else ", ".join(probe.blocked_on()),
        )
    console.print(table)
