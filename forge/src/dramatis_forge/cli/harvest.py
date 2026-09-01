"""Harvest commands: scope, fetch, sync, update.

`sync` is the one to reach for. The three stages are separately runnable because they
fail for different reasons and cost different amounts, but the normal case is "bring the
archive up to date", and making that one command means nobody has to remember the order.
"""

from __future__ import annotations

from typing import Annotated

import typer
from rich.table import Table

from ..config import RunInfo
from ..harvest import fetch as fetch_stage
from ..harvest import scope as scope_stage
from ..harvest import update as update_stage
from ..normalize.guards import Ledger
from ..normalize.runner import run as normalize_run
from ..store.archive import Archive
from ..wiki import Wiki
from .common import HomeOpt, PackOpt, console, die, need, render_guards, resolve, ticker

app = typer.Typer(no_args_is_help=True)

RateOpt = Annotated[float, typer.Option("--rate", help="Max requests per second.")]


def _wiki(pack, rate: float | None) -> Wiki:
    return Wiki(
        pack.wiki.api,
        pack.wiki.contact,
        rate=rate if rate is not None else pack.wiki.rate,
    )


def _scope_table(pack, counts: dict[str, int]) -> Table:
    table = Table(title="seed sets")
    table.add_column("set")
    table.add_column("what")
    table.add_column("authority", overflow="fold")
    table.add_column("count", justify="right")
    table.add_column("baseline", justify="right")
    table.add_column("body?", justify="center")
    for seed in pack.seeds:
        n = counts.get(seed.key, 0)
        base = seed.baseline
        drift = ""
        if base and n != base:
            drift = f"[{'yellow' if n > base else 'red'}]{n:,}[/]"
        table.add_row(
            seed.key, seed.label, seed.source,
            drift or f"{n:,}", f"{base:,}" if base else "—",
            "✓" if seed.fetch else "",
        )
    return table


@app.command()
def scope(
    pack: PackOpt = "",
    home: HomeOpt = None,
    rate: RateOpt = None,
) -> None:
    """Enumerate the seed sets. Index queries only — no page bodies."""
    pk, paths = resolve(pack, home)
    with _wiki(pk, rate) as wiki:
        sc = scope_stage.build(wiki, pk, progress=ticker("scope"))
    with Archive(paths.archive) as archive:
        scope_stage.write(archive, sc, pk)
        archive.set_meta("run_scope", RunInfo.create("scope", pk.name, pk.version).as_dict())

    console.print(_scope_table(pk, sc.counts))
    console.print(f"bodies to fetch: [bold]{len(sc.fetch_titles(pk)):,}[/bold]")
    ledger = Ledger()
    ledger.extend(sc.findings)
    render_guards(ledger)
    console.print(f"wrote [cyan]{paths.archive}[/cyan]")


@app.command()
def fetch(
    pack: PackOpt = "",
    home: HomeOpt = None,
    rate: RateOpt = None,
    limit: Annotated[int, typer.Option("--limit", help="Only fetch N pages (0 = all).")] = 0,
    refetch: Annotated[bool, typer.Option("--refetch", help="Re-fetch pages already held.")] = False,
) -> None:
    """Retrieve page bodies. Resumable: pages already held are skipped."""
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest scope` first")
    with Archive(paths.archive) as archive, _wiki(pk, rate) as wiki:
        res = fetch_stage.run(
            wiki, archive, pk, limit=limit, refetch=refetch, progress=ticker("fetch"))
        archive.set_meta("run_fetch", RunInfo.create("fetch", pk.name, pk.version).as_dict())
    console.print(
        f"fetched [bold]{res.fetched:,}[/bold], skipped [dim]{res.skipped:,}[/dim] already held")
    for seed, n in sorted(res.followups.items()):
        console.print(f"  follow-up [cyan]{seed}[/cyan]: {n} page(s) discovered while fetching")
    if res.findings:
        ledger = Ledger()
        ledger.extend(res.findings)
        render_guards(ledger)
    if res.missing:
        console.print(
            f"[yellow]{len(res.missing)} enumerated title(s) had no content[/yellow] — "
            "a scope problem rather than a transport one")
        for title in res.missing[:10]:
            console.print(f"    [dim]{title}[/dim]")
    console.print(f"wrote [cyan]{paths.rawcache}[/cyan]")


@app.command()
def sync(
    pack: PackOpt = "",
    home: HomeOpt = None,
    rate: RateOpt = None,
    skip_scope: Annotated[bool, typer.Option("--skip-scope", help="Reuse the stored scope.")] = False,
) -> None:
    """Full archive sync: scope, fetch every body, normalize, set the watermark.

    Resumable at every step. Interrupt it and run it again; already-held pages are
    skipped, so the cost of a restart is one index pass rather than another full fetch.
    """
    pk, paths = resolve(pack, home)

    with _wiki(pk, rate) as wiki:
        if not skip_scope:
            console.rule("[bold]1/4 scope")
            sc = scope_stage.build(wiki, pk, progress=ticker("scope"))
            with Archive(paths.archive) as archive:
                scope_stage.write(archive, sc, pk)
            console.print(_scope_table(pk, sc.counts))
            ledger = Ledger()
            ledger.extend(sc.findings)
            render_guards(ledger)
        elif not paths.archive.exists():
            die("--skip-scope needs an existing archive", "drop the flag for a first run")

        console.rule("[bold]2/4 fetch")
        with Archive(paths.archive) as archive:
            todo = len(archive.titles_in(pk.fetch_seeds)) - len(archive.have_pages())
            console.print(f"  [dim]{max(todo, 0):,} page(s) to retrieve[/dim]")
            res = fetch_stage.run(wiki, archive, pk, progress=ticker("fetch"))
        console.print(
            f"fetched [bold]{res.fetched:,}[/bold], skipped [dim]{res.skipped:,}[/dim]")
        for seed, n in sorted(res.followups.items()):
            console.print(f"  follow-up [cyan]{seed}[/cyan]: {n} page(s)")
        if res.findings:
            fetch_ledger = Ledger()
            fetch_ledger.extend(res.findings)
            render_guards(fetch_ledger)

        console.rule("[bold]3/4 normalize")
        with Archive(paths.archive) as archive:
            report = normalize_run(archive, pk, progress=ticker("normalize"))
        _records_table(report)
        render_guards(report.ledger)

        console.rule("[bold]4/4 watermark")
        with Archive(paths.archive) as archive:
            top = update_stage.init_watermark(wiki, archive, pk)
            archive.set_meta("run_sync", RunInfo.create("sync", pk.name, pk.version).as_dict())
        console.print(
            f"watermark pinned at rcid=[bold]{top:,}[/bold] — "
            "[dim]later `harvest update` runs are incremental from here[/dim]")

    console.print()
    console.print(f"[green]archive synced[/green]  [cyan]{paths.archive}[/cyan]")
    console.print(f"  requests this run: {wiki.requests:,}")
    console.print("  next: [bold]dramatis-forge corpus build[/bold]")


@app.command()
def update(
    pack: PackOpt = "",
    home: HomeOpt = None,
    rate: RateOpt = None,
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Plan only; touch nothing.")] = False,
    rescope: Annotated[
        bool,
        typer.Option(
            "--rescope/--no-rescope",
            help="Re-enumerate the seed sets. On by default; the slow part.",
        ),
    ] = True,
) -> None:
    """Incremental update from the site's change feed.

    Two questions get answered, and they are not the same question. The change feed says
    which held pages were edited; re-enumeration says whether the seed sets gained or lost
    members. Only the second can notice that a page left scope, which is why it defaults
    on — but it costs about a minute against a change feed that costs under a second, so
    `--no-rescope` is there for a routine increment.
    """
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")

    with Archive(paths.archive) as archive, _wiki(pk, rate) as wiki:
        if not archive.get_meta("watermark"):
            top = update_stage.init_watermark(wiki, archive, pk)
            console.print(f"[yellow]first run: watermark initialised to rcid={top:,}[/yellow]")
            console.print("the archive is now level with the site; the next `update` is incremental")
            return

        plan = update_stage.plan(
            wiki, archive, pk, rescope=rescope, progress=ticker("plan"))
        if not rescope:
            console.print(
                "[yellow]--no-rescope: seed sets not re-enumerated[/yellow] "
                "[dim]pages that left scope will not be noticed this run[/dim]")
        table = Table(title="update plan")
        table.add_column("item")
        table.add_column("value", justify="right")
        table.add_row("watermark", f"{plan.watermark:,} → {plan.new_watermark:,}")
        table.add_row("changed, in scope", f"{len(plan.changed):,}")
        table.add_row("changed, out of scope", f"[dim]{plan.ignored:,}[/dim]")
        table.add_row("newly in scope", f"{len(plan.added):,}")
        table.add_row(
            "in scope, no body held",
            f"[yellow]{len(plan.missing):,}[/yellow]" if plan.missing else "0")
        table.add_row("left scope", f"{len(plan.gone):,}")
        console.print(table)

        ledger = Ledger()
        ledger.extend(plan.findings)
        render_guards(ledger)

        for label, items in (("changed", plan.changed), ("added", plan.added),
                             ("missing", plan.missing), ("gone", plan.gone)):
            for title in items[:10]:
                console.print(f"  [{label}] {title}")
            if len(items) > 10:
                console.print(f"  [dim]… {len(items) - 10:,} more {label}[/dim]")

        if plan.nothing_to_do:
            console.print("[green]nothing to update[/green]")
            return
        if dry_run:
            console.print("[dim]--dry-run: archive untouched[/dim]")
            return

        result = update_stage.apply(wiki, archive, pk, plan, progress=ticker("update"))

    console.print(
        f"re-fetched [bold]{result['refetched']:,}[/bold], removed {result['removed']:,}, "
        f"watermark {result['watermark']:,}")
    _records_table(result["report"])
    render_guards(result["report"].ledger)


def _records_table(report) -> None:
    from ..normalize.records import LABELS

    table = Table(title="records")
    table.add_column("kind")
    table.add_column("what", overflow="fold")
    table.add_column("count", justify="right")
    table.add_column("chars", justify="right")
    for kind, n, chars in report.rows():
        if not n:
            continue
        table.add_row(kind, LABELS.get(kind, kind), f"{n:,}", f"{chars:,}" if chars else "—")
    console.print(table)
    console.print(
        f"usable text: [bold]{report.total_chars / 1e6:.2f} M[/bold] chars · "
        f"people: [bold]{report.persons:,}[/bold] · "
        f"pages parsed: {report.pages_seen:,} ({report.pages_empty:,} yielded nothing)")
