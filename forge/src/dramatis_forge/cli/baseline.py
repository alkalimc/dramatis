"""Baseline commands: accept the current counts, or compare against the accepted ones."""

from __future__ import annotations

import typer
from rich.table import Table

from .. import baseline as baseline_mod
from ..store.archive import Archive
from .common import HomeOpt, PackOpt, console, need, resolve

app = typer.Typer(no_args_is_help=True)


def _table(current: baseline_mod.Baseline, accepted: baseline_mod.Baseline | None) -> Table:
    table = Table(title="baseline")
    table.add_column("group")
    table.add_column("key")
    table.add_column("accepted", justify="right")
    table.add_column("now", justify="right")
    for group, now, then in (
        ("seeds", current.seeds, accepted.seeds if accepted else {}),
        ("identity", current.identity, accepted.identity if accepted else {}),
    ):
        for key in sorted(set(now) | set(then)):
            n, b = now.get(key), then.get(key)
            style = "" if n == b else ("red" if b is not None and (n or 0) < b else "yellow")
            shown = f"[{style}]{n if n is not None else '—'}[/]" if style else str(n)
            table.add_row(group, key, "—" if b is None else f"{b:,}", shown)
    return table


@app.command("accept")
def accept_cmd(pack: PackOpt = "", home: HomeOpt = None) -> None:
    """Record what the archive measures now as the expected sizes.

    Run it after reviewing a sync: from then on a shrinking seed set or person count is a
    high-severity finding and growth a low one, until the next accept.
    """
    _pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")
    with Archive(paths.archive) as archive:
        before = baseline_mod.load(archive.path)
        path, current = baseline_mod.accept(archive)
    console.print(_table(current, before))
    console.print(f"[green]accepted[/green] [cyan]{path}[/cyan] "
                  f"(watermark {current.watermark:,}, {current.accepted_at})")


@app.command("show")
def show_cmd(pack: PackOpt = "", home: HomeOpt = None) -> None:
    """Compare the archive's current counts with the accepted baseline. Read-only."""
    _pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")
    with Archive(paths.archive, readonly=True) as archive:
        current = baseline_mod.measure(archive)
        accepted = baseline_mod.load(archive.path)
    console.print(_table(current, accepted))
    if accepted is None:
        console.print("[yellow]no accepted baseline[/yellow] — run `baseline accept`")
