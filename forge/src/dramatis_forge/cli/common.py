"""Shared CLI plumbing: option definitions, output helpers, guard rendering.

Kept in one place because consistency across stages matters more here than anywhere
else in the codebase: these are the surfaces a maintainer reads at 2am while deciding
whether a run went wrong.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Annotated

import typer
from rich.console import Console
from rich.table import Table

from ..config import Paths
from ..normalize.guards import GUARDS, Ledger
from ..pack import Pack, load_pack

console = Console()

#: No default. This repository ships no pack, so a default here would name someone
#: else's rules and fail confusingly when they are absent.
PackOpt = Annotated[
    str,
    typer.Option("--pack", "-p", help="Domain pack to use (or set DRAMATIS_PACK)."),
]
HomeOpt = Annotated[
    Path | None,
    typer.Option("--home", help="Artifact root (default: <workspace>/artifacts)."),
]


def resolve(pack_name: str, home: Path | None) -> tuple[Pack, Paths]:
    """Load the selected pack, or explain what is missing.

    The framework has no rules of its own, so every stage needs a pack named. Failing
    with a traceback here would be the least useful moment for one: the likely cause is
    an unset search path, not a bug.
    """
    name = pack_name or os.environ.get("DRAMATIS_PACK", "")
    if not name:
        die("no pack selected",
            "pass --pack <name>, or set DRAMATIS_PACK; "
            "the directory holding packs/<name>/ must be on DRAMATIS_PACKS")
    try:
        pack = load_pack(name)
    except ModuleNotFoundError as exc:
        die(f"no pack named {name!r}",
            f"{exc}. Point DRAMATIS_PACKS at the directory containing packs/{name}/")
    except TypeError as exc:
        die(f"pack {name!r} is malformed", str(exc))
    return pack, Paths.for_pack(pack.name, home).ensure()


def die(message: str, hint: str = "") -> None:
    console.print(f"[red]{message}[/red]")
    if hint:
        console.print(f"[dim]{hint}[/dim]")
    raise typer.Exit(1)


def need(path: Path, what: str, hint: str) -> None:
    if not path.exists():
        die(f"no {what} at {path}", hint)


def ticker(label: str):
    """Progress callback that prints, rather than animating.

    A spinner is worse than lines here: these runs take minutes, are often piped to a
    log, and what a reader wants afterwards is the sequence of stages with their counts.
    """
    def tick(message: object) -> None:
        console.print(f"  [dim]{label}[/dim] {message}", highlight=False)
    return tick


def fmt_count(n: int, *, warn: bool = False) -> str:
    if n == 0:
        return "[green]0[/green]"
    return f"[{'yellow' if warn else 'cyan'}]{n:,}[/{'yellow' if warn else 'cyan'}]"


def render_guards(ledger: Ledger, *, show: int = 12) -> None:
    """Print the guard tally, then the high-severity findings in full.

    High-severity findings are printed entire and never truncated: they are the ones
    that block a design freeze, and a truncated list of blockers is useless.
    """
    tally = ledger.tally()
    if not tally:
        console.print("[green]no guard findings[/green]")
        return

    table = Table(title="guards", show_header=True)
    table.add_column("guard")
    table.add_column("what it checks", overflow="fold")
    table.add_column("high", justify="right")
    table.add_column("low", justify="right")
    for guard, description in GUARDS.items():
        if guard not in tally:
            continue
        high, low = tally[guard]
        table.add_row(
            guard, description,
            f"[red]{high:,}[/red]" if high else "[green]0[/green]",
            f"[dim]{low:,}[/dim]" if low else "0",
        )
    console.print(table)

    for finding in ledger.high():
        console.print(f"  [red]![/red] {finding}")
    lows = ledger.low()
    for finding in lows[:show]:
        console.print(f"  [dim]·[/dim] {finding}")
    if len(lows) > show:
        console.print(f"  [dim]… {len(lows) - show:,} more low-severity findings[/dim]")

    if ledger.clean:
        console.print(
            "[green]no high-severity findings[/green] — "
            "[dim]low-severity items must each be attributed; an unexplained one is a "
            "high-severity finding in waiting[/dim]"
        )
    else:
        console.print(
            f"[red]{len(ledger.high())} high-severity finding(s) — "
            "the design freeze is blocked while any remain[/red]"
        )
