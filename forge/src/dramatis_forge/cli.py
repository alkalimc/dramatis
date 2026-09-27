"""`dramatis-forge`: one command per job.

    sync       network: bring the archive level with the site, then `build`
    build      offline: normalize, samples, folio, evals, reports
    baseline   accept or show the counts the guards compare against
    report     regenerate one report, or inspect a page

`sync` is the only command that makes requests. The first run (no watermark) and
`--full` enumerate and fetch everything; later runs read the change feed from the
watermark. Both are resumable: pages already held are never fetched twice.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Annotated

import typer
from rich.console import Console
from rich.table import Table

from . import baseline as baseline_mod
from . import corpus as corpus_mod
from . import evals as evals_mod
from . import harvest
from . import normalize as normalize_mod
from .archive import Archive
from .config import Paths
from .folio import Folio
from .guards import HIGH
from .pack import Pack, load_pack, pack_dir
from .report import attribution as attribution_mod
from .report import coverage as coverage_mod
from .report import figures as figures_mod
from .report import inspect as inspect_mod
from .report import samples as samples_mod
from .wiki import Wiki

console = Console()
app = typer.Typer(add_completion=False, no_args_is_help=True,
                  help="Turn a collaborative wiki into a retrieval-grounded character corpus.")
baseline_app = typer.Typer(no_args_is_help=True)
report_app = typer.Typer(no_args_is_help=True)
app.add_typer(baseline_app, name="baseline", help="Accept or compare the guards' expected counts.")
app.add_typer(report_app, name="report", help="Regenerate one report, or inspect a page.")

#: No default. This repository ships no pack, so a default would name someone else's rules.
PackOpt = Annotated[str, typer.Option("--pack", "-p", help="Pack to use (or set DRAMATIS_PACK).")]
HomeOpt = Annotated[Path | None, typer.Option(
    "--home", help="Artifact root (default: <workspace>/artifacts).")]


# --------------------------------------------------------------------------- #
# Plumbing
# --------------------------------------------------------------------------- #


def _resolve(pack_name: str, home: Path | None) -> tuple[Pack, Paths]:
    name = pack_name or os.environ.get("DRAMATIS_PACK", "")
    if not name:
        _die("no pack selected", "pass --pack <name> or set DRAMATIS_PACK; the directory "
             "holding packs/<name>/ must be on DRAMATIS_PACKS")
    try:
        pack = load_pack(name)
    except ModuleNotFoundError as exc:
        _die(f"no pack named {name!r}",
             f"{exc}. Point DRAMATIS_PACKS at the directory containing packs/{name}/")
    except TypeError as exc:
        _die(f"pack {name!r} is malformed", str(exc))
    return pack, Paths.for_pack(pack.name, home).ensure()


def _die(message: str, hint: str = "") -> None:
    console.print(f"[red]{message}[/red]")
    if hint:
        console.print(f"[dim]{hint}[/dim]")
    raise typer.Exit(1)


def _need(path: Path, what: str) -> None:
    if not path.exists():
        _die(f"no {what} at {path}", "run `forge sync` first")


def _tick(label: str):
    def tick(message: object) -> None:
        console.print(f"  [dim]{label}[/dim] {message}", highlight=False)
    return tick


def _wiki(pack: Pack, rate: float | None = None) -> Wiki:
    return Wiki(pack.wiki.api, pack.wiki.contact,
                rate=rate if rate is not None else pack.wiki.rate)


def _guards(archive: Archive, pack: Pack, *, show: int = 12) -> bool:
    """Print the tally and the findings behind it. High-severity ones are never truncated:
    they fail the release gate, and a truncated list of blockers is useless."""
    table = Table(title="guards")
    for col, justify in (("guard", "left"), ("checks", "left"), ("high", "right"),
                         ("low", "right")):
        table.add_column(col, justify=justify, overflow="fold")
    for guard, (high, low) in archive.tally().items():
        table.add_row(guard, pack.say(f"guard.{guard}"),
                      f"[red]{high:,}[/red]" if high else "0", f"{low:,}")
    console.print(table)
    rows = list(archive.db.execute(
        "SELECT guard, severity, page, detail FROM guard_findings ORDER BY severity<>?, guard",
        (HIGH,)))
    high = [r for r in rows if r["severity"] == HIGH]
    low = [r for r in rows if r["severity"] != HIGH]
    for r in high + low[:show]:
        mark = "[red]![/red]" if r["severity"] == HIGH else "[dim]·[/dim]"
        where = f" [{r['page']}]" if r["page"] else ""
        console.print(f"  {mark} {r['guard']}{where} {r['detail']}", highlight=False)
    if len(low) > show:
        console.print(f"  [dim]… {len(low) - show:,} more low-severity findings[/dim]")
    if high:
        console.print(f"[red]{len(high)} high-severity finding(s): the release gate fails[/red]")
    return not high


# --------------------------------------------------------------------------- #
# sync
# --------------------------------------------------------------------------- #


@app.command()
def sync(
    pack: PackOpt = "",
    home: HomeOpt = None,
    full: Annotated[bool, typer.Option(
        "--full", help="Enumerate and re-fetch every page in scope.")] = False,
    dry_run: Annotated[bool, typer.Option("--dry-run", help="Plan only; touch nothing.")] = False,
    rescope: Annotated[bool, typer.Option(
        "--rescope/--no-rescope",
        help="Re-enumerate the seed sets on an incremental sync (the slow part).")] = True,
    rate: Annotated[float | None, typer.Option("--rate", help="Max requests per second.")] = None,
    samples: Annotated[bool, typer.Option("--samples/--no-samples")] = True,
) -> None:
    """Bring the archive level with the site, then run `build`.

    Two questions get answered, and they are not the same question. The change feed says
    which held pages were edited; re-enumeration says whether the seed sets gained or lost
    members. Only the second notices a page leaving scope, which is why it defaults on;
    `--no-rescope` is there for a quick routine increment.
    """
    pk, paths = _resolve(pack, home)
    with Archive(paths.archive) as archive, _wiki(pk, rate) as wiki:
        p = harvest.plan(wiki, archive, pk, full=full, rescope=rescope, progress=_tick("plan"))
        table = Table(title="first sync" if p.first else ("full sync" if p.full else "sync plan"))
        table.add_column("item")
        table.add_column("count", justify="right")
        if not p.first:
            table.add_row("watermark", f"{p.watermark:,} → {p.new_watermark:,}")
        for label, n in (("changed, in scope", len(p.changed)),
                         ("changed, out of scope", p.ignored),
                         ("newly in scope", len(p.added)),
                         ("in scope, no body held", len(p.missing)),
                         ("left scope", len(p.gone))):
            table.add_row(label, f"{n:,}")
        console.print(table)
        if not p.full and not rescope:
            console.print("[yellow]--no-rescope[/yellow] [dim]pages that left scope will "
                          "not be noticed this run[/dim]")
        for label, items in (("changed", p.changed), ("added", p.added),
                             ("missing", p.missing), ("gone", p.gone)):
            for title in items[:10]:
                console.print(f"  [{label}] {title}", highlight=False)
            if len(items) > 10:
                console.print(f"  [dim]… {len(items) - 10:,} more {label}[/dim]")
        if dry_run:
            console.print("[dim]--dry-run: archive untouched[/dim]")
            return
        if p.nothing_to_do:
            # Still a sync: the archive was checked against the site and found level, so
            # the watermark, the sync time and the sample all say so.
            console.print("[green]nothing to fetch[/green]: the archive is level with the site")

        res = harvest.apply(wiki, archive, pk, p, progress=_tick("fetch"))
        console.print(f"fetched [bold]{res.fetched:,}[/bold] · removed "
                      f"{len(res.delta.get('removed', [])):,} · watermark {p.new_watermark:,}")
        for seed, n in sorted(res.followups.items()):
            console.print(f"  followup [cyan]{seed}[/cyan]: {n} page(s)")
        if res.missing:
            console.print(f"[yellow]{len(res.missing)} title(s) had no content[/yellow] "
                          "[dim]a scope problem rather than a transport one[/dim]")
        # Attribution needs the author of every held revision; only new ones cost a request.
        looked = harvest.refresh_editors(wiki, archive, progress=_tick("editors"))
        console.print(f"editors looked up: {looked:,} revision(s) · requests this run: "
                      f"{wiki.requests:,}")
    _build(pk, paths, samples=samples)


# --------------------------------------------------------------------------- #
# build
# --------------------------------------------------------------------------- #


@app.command()
def build(
    pack: PackOpt = "",
    home: HomeOpt = None,
    samples: Annotated[bool, typer.Option("--samples/--no-samples")] = True,
) -> None:
    """Offline: normalize → samples → folio → evals → reports. No network access.

    The command to re-run after changing a parse rule: it costs minutes and no bandwidth,
    which is what makes the rules safe to keep changing.
    """
    pk, paths = _resolve(pack, home)
    _need(paths.archive, "archive")
    _build(pk, paths, samples=samples)


def _build(pk: Pack, paths: Paths, *, samples: bool) -> None:
    console.rule("[bold]normalize")
    with Archive(paths.archive) as archive:
        rep = normalize_mod.run(archive, pk, progress=_tick("normalize"))
    table = Table(title="records")
    for col in ("kind", "what", "count", "chars"):
        table.add_column(col, justify="left" if col in ("kind", "what") else "right")
    for kind, n, chars in rep.rows():
        if n:
            table.add_row(kind, pk.say(f"record.{kind}"), f"{n:,}", f"{chars:,}" if chars else "—")
    console.print(table)
    console.print(f"usable text {rep.total_chars / 1e6:.2f} M chars · persons "
                  f"{rep.persons:,} · pages parsed {rep.pages_seen:,} "
                  f"({rep.pages_empty:,} yielded nothing)")

    console.rule("[bold]folio")
    with Archive(paths.archive) as archive:
        crep = corpus_mod.run(archive, pk, paths.folio, progress=_tick("folio"))
    console.print(f"units [bold]{crep.chunks:,}[/bold] · "
                  + " · ".join(f"{t} {n:,}" for t, n in sorted(crep.by_template.items()))
                  + f" · segmenter {crep.segmenter}")

    console.rule("[bold]evals")
    with Folio(paths.folio, readonly=True) as folio:
        suite = evals_mod.build(folio)
        evals_mod.write(suite, paths.evals, folio=folio)
    console.print(f"structural suite: {len(suite.queries):,} queries · "
                  + " · ".join(f"{f} {n:,}" for f, n in sorted(suite.by_family().items())))

    if samples:
        console.rule("[bold]samples")
        _samples(pk, paths)

    console.rule("[bold]reports")
    with Archive(paths.archive, readonly=True) as archive:
        ok = _guards(archive, pk)
    _coverage(pk, paths)
    failed = _figures(pk, paths, show=False)
    _attribution(pk, paths)
    if not ok or failed:
        raise typer.Exit(1)


def _samples(pk: Pack, paths: Paths) -> None:
    with Archive(paths.archive, readonly=True) as archive:
        index, count = samples_mod.write(archive, pk, paths.samples)
    console.print(f"samples: [bold]{count:,}[/bold] page(s) → [cyan]{index}[/cyan]")


def _coverage(pk: Pack, paths: Paths) -> None:
    with Archive(paths.archive, readonly=True) as archive:
        markdown = coverage_mod.build(archive, pk)
    paths.coverage.write_text(markdown, encoding="utf-8")
    console.print(f"[cyan]{paths.coverage}[/cyan]")


def _attribution(pk: Pack, paths: Paths) -> None:
    templates = sorted((pack_dir(pk.name) / "templates").glob("*.md"))
    if not templates:
        console.print("[dim]attribution: the pack ships no templates; skipped[/dim]")
        return
    with Archive(paths.archive, readonly=True) as archive, \
            Folio(paths.folio, readonly=True) as folio:
        data = attribution_mod.collect(archive, folio)
    if not data.complete:
        _die(f"{data.units_without_revid:,} unit(s) have no source revision",
             "attribution would claim a traceability that does not exist")
    for path in attribution_mod.render(data, pk, templates, paths.reports):
        console.print(f"[cyan]{path}[/cyan]")
    missing = data.page_count - data.editors_resolved
    if missing:
        console.print(f"[yellow]{missing:,} page(s) without a cached editor[/yellow] "
                      "[dim]run `forge report attribution --editors` or `forge sync`[/dim]")


def _figures(pk: Pack, paths: Paths, *, show: bool) -> list:
    figs = figures_mod.resolve(pk, paths)
    if show:
        table = Table(title="figures")
        for col in ("key", "value", "target", "note"):
            table.add_column(col, overflow="fold", justify="right" if col == "value" else "left")
        for fig in figs:
            table.add_row(fig.key, fig.shown, figures_mod.verdict(fig), f"[dim]{fig.note}[/dim]")
        console.print(table)
    paths.figures.write_text(figures_mod.render(figs, pk), encoding="utf-8")
    console.print(f"[cyan]{paths.figures}[/cyan]")
    failed = [f for f in figs if f.passed is False]
    for fig in failed:
        console.print(f"  [red]✗ {fig.key}[/red] = {fig.shown}, target {fig.target}")
    return failed


# --------------------------------------------------------------------------- #
# baseline
# --------------------------------------------------------------------------- #


def _baseline_table(current: baseline_mod.Baseline, accepted: baseline_mod.Baseline | None) -> Table:
    table = Table(title="baseline")
    for col in ("group", "key", "accepted", "now"):
        table.add_column(col, justify="right" if col in ("accepted", "now") else "left")
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


@baseline_app.command("accept")
def baseline_accept(pack: PackOpt = "", home: HomeOpt = None) -> None:
    """Record what the archive measures now as the expected sizes.

    Run it after reviewing a sync's samples: from then on a shrinking seed set or person
    count is a high-severity finding and growth a low one, until the next accept.
    """
    _pk, paths = _resolve(pack, home)
    _need(paths.archive, "archive")
    with Archive(paths.archive) as archive:
        before = baseline_mod.load(archive.path)
        path, current = baseline_mod.accept(archive)
    console.print(_baseline_table(current, before))
    console.print(f"[green]accepted[/green] [cyan]{path}[/cyan] "
                  f"(watermark {current.watermark:,}, {current.accepted_at})")


@baseline_app.command("show")
def baseline_show(pack: PackOpt = "", home: HomeOpt = None) -> None:
    """Compare the archive's current counts with the accepted baseline. Read-only."""
    _pk, paths = _resolve(pack, home)
    _need(paths.archive, "archive")
    with Archive(paths.archive, readonly=True) as archive:
        current = baseline_mod.measure(archive)
        accepted = baseline_mod.load(archive.path)
    console.print(_baseline_table(current, accepted))
    if accepted is None:
        console.print("[yellow]no accepted baseline[/yellow] — run `forge baseline accept`")


# --------------------------------------------------------------------------- #
# report
# --------------------------------------------------------------------------- #


@report_app.command("samples")
def report_samples(pack: PackOpt = "", home: HomeOpt = None) -> None:
    """Rebuild the reading sample alone (it also runs at the end of every build)."""
    pk, paths = _resolve(pack, home)
    _need(paths.archive, "archive")
    _samples(pk, paths)


@report_app.command("inspect")
def report_inspect(
    title: Annotated[str, typer.Argument(help="Page title.")],
    pack: PackOpt = "",
    home: HomeOpt = None,
    out: Annotated[Path | None, typer.Option("--out", help="Output directory.")] = None,
) -> None:
    """Dump one page at every stage: source, records, archived text."""
    pk, paths = _resolve(pack, home)
    _need(paths.archive, "archive")
    with Archive(paths.archive, readonly=True) as archive:
        written = inspect_mod.dump(archive, pk, title, out or paths.samples / "inspect")
    if not written:
        _die(f"{title!r} is not in the archive", "see COVERAGE.md for what is in scope")
    for path in written:
        console.print(f"[green]{path}[/green]  {path.stat().st_size:,} bytes")


@report_app.command("figures")
def report_figures(
    pack: PackOpt = "",
    home: HomeOpt = None,
    check: Annotated[Path | None, typer.Option(
        "--check", help="Document tree to audit against the registered figures.")] = None,
    show: Annotated[bool, typer.Option("--show", help="Print the figure table.")] = False,
) -> None:
    """Rewrite FIGURES.md; with `--check`, audit a document tree.

    The audit reports dangling relative `.md` links, cross-reference ids nothing defines,
    and backticked figure keys that are neither computed nor declared planned by the pack.
    Figures with a target are judged pass or fail; any failure exits non-zero.
    """
    pk, paths = _resolve(pack, home)
    _need(paths.folio, "folio")
    failed = _figures(pk, paths, show=show or not check)
    if check is None:
        if failed:
            raise typer.Exit(1)
        return
    if not check.is_dir():
        _die(f"no document tree at {check}")
    rep = figures_mod.scan(check, pk)
    console.print(f"scanned [bold]{rep.scanned}[/bold] documents under [cyan]{check}[/cyan]")
    for hit in rep.hits:
        console.print(f"  [yellow]{hit.where}[/yellow]  {hit.what}: {hit.detail}",
                      highlight=False)
    if not rep.hits:
        console.print("[green]every link resolves, every id is defined, every key is known[/green]")
    if failed or rep.hits:
        raise typer.Exit(1)


@report_app.command("coverage")
def report_coverage(pack: PackOpt = "", home: HomeOpt = None) -> None:
    """Rewrite COVERAGE.md: the source's table of contents, what was taken and why not."""
    pk, paths = _resolve(pack, home)
    _need(paths.archive, "archive")
    _coverage(pk, paths)


@report_app.command("attribution")
def report_attribution(
    pack: PackOpt = "",
    home: HomeOpt = None,
    editors: Annotated[bool, typer.Option(
        "--editors", help="First look up editors the cache lacks (network).")] = False,
) -> None:
    """Rewrite ATTRIBUTION.md and NOTICE.md from the cached editors.

    Refuses to write if any unit lacks a source revision: a notice claiming every record
    is traceable, produced from a corpus where it is not, would be a false statement.
    """
    pk, paths = _resolve(pack, home)
    _need(paths.folio, "folio")
    if editors:
        with Archive(paths.archive) as archive, _wiki(pk) as wiki:
            looked = harvest.refresh_editors(wiki, archive, progress=_tick("editors"))
        console.print(f"editors looked up: {looked:,} revision(s)")
    _attribution(pk, paths)


def main() -> None:  # pragma: no cover - entry point
    app()
