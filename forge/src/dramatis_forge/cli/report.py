"""Report commands: coverage, and per-page inspection."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from ..report import attribution as attribution_mod
from ..report import coverage as coverage_mod
from ..report import figures as figures_mod
from ..report import inspect as inspect_mod
from ..pack import pack_dir
from ..store.archive import Archive
from ..store.folio import Folio
from ..wiki import Wiki
from .common import HomeOpt, PackOpt, console, die, need, resolve, ticker

app = typer.Typer(no_args_is_help=True)

_STYLE = {"archived": "green", "partial": "yellow", "excluded": "dim"}


@app.command("coverage")
def coverage_cmd(
    pack: PackOpt = "",
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


@app.command("figures")
def figures_cmd(
    pack: PackOpt = "",
    home: HomeOpt = None,
    check: Annotated[
        Path | None,
        typer.Option("--check", help="Document tree to scan against these figures."),
    ] = None,
    show: Annotated[bool, typer.Option("--show", help="Print the figure table.")] = False,
    out: Annotated[
        Path | None, typer.Option("--out", help="Where to write the figure report.")
    ] = None,
) -> None:
    """Print the figures an artifact actually supports, and optionally audit documents.

    Guards check the corpus; G6 checks that an artifact agrees with itself. Neither
    catches a document quoting a number the artifact never produced, which is its own
    failure mode: the number has a citation, the citation resolves, and the value is
    stale. Reading 22 documents cannot catch it either, because it requires remembering
    every figure that has ever been revised.
    """
    pk, paths = resolve(pack, home)
    need(paths.folio, "folio", "run `dramatis-forge corpus build` first")
    specs = figures_mod.figures_for(pk)
    if not specs:
        die(f"pack {pack!r} declares no figures", "add FIGURES to the pack")
    figs = figures_mod.resolve(specs, paths.archive, paths.folio)

    if show or not check:
        table = Table(title="figures")
        table.add_column("key")
        table.add_column("value", justify="right")
        table.add_column("note", overflow="fold")
        for fig in figs:
            table.add_row(fig.key, fig.shown, f"[dim]{fig.note}[/dim]")
        console.print(table)

    destination = out or paths.figures
    destination.parent.mkdir(parents=True, exist_ok=True)
    fingerprint = next((f.shown for f in figs if f.key == "fingerprint"), "")
    destination.write_text(
        figures_mod.render(figs, fingerprint=fingerprint), encoding="utf-8")
    console.print(f"[green]{destination}[/green]  {destination.stat().st_size:,} bytes")

    if not check:
        return

    if not check.is_dir():
        die(f"no document tree at {check}", "pass --check <dir>")
    report = figures_mod.scan(check, figs)
    console.print(
        f"scanned [bold]{report.scanned}[/bold] documents under [cyan]{check}[/cyan]")

    for label, hits, colour in (
        ("stale figure", report.stale, "red"),
        ("dangling reference", report.dangling, "yellow"),
    ):
        if not hits:
            continue
        console.print(f"\n[{colour}]{len(hits)} {label}(s)[/{colour}]")
        for hit in hits:
            console.print(f"  [{colour}]{hit.where}[/{colour}]  {hit.what} — {hit.detail}")
            console.print(f"    [dim]{hit.line[:150]}[/dim]")

    if report.clean:
        console.print(
            "[green]every quoted figure matches the artifact, every reference "
            "resolves[/green]")
    else:
        raise typer.Exit(1)


@app.command("inspect")
def inspect_cmd(
    title: Annotated[str, typer.Argument(help="Page title.")],
    pack: PackOpt = "",
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


@app.command("attribution")
def attribution_cmd(
    pack: PackOpt = "",
    home: HomeOpt = None,
    templates: Annotated[
        Path | None, typer.Option("--templates", help="Directory holding the templates.")
    ] = None,
    editors: Annotated[
        bool, typer.Option("--editors", help="Look up each page's last editor (network).")
    ] = False,
    issue_url: Annotated[str, typer.Option("--issue-url")] = "",
    contact: Annotated[str, typer.Option("--contact")] = "",
) -> None:
    """Generate the attribution and notice files that must accompany a published corpus.

    Refuses to write if any unit lacks a source revision: a notice claiming every record
    is traceable, produced from a corpus where they are not, would be a false statement in
    the one document that must not contain one.
    """
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")
    need(paths.folio, "folio", "run `dramatis-forge corpus build` first")

    template_dir = templates or pack_dir(pack) / "templates"
    found = sorted(template_dir.glob("*.md")) if template_dir.is_dir() else []
    if not found:
        die(f"no templates in {template_dir}", "pass --templates <dir>")

    with Archive(paths.archive, readonly=True) as archive, Folio(paths.folio, readonly=True) as folio:
        data = attribution_mod.collect(archive, folio)
        if editors:
            with Wiki(pk.wiki.api, pk.wiki.contact, rate=pk.wiki.rate) as wiki:
                attribution_mod.resolve_editors(wiki, data, progress=ticker("attribution"))

        console.print(
            f"pages [bold]{data.page_count:,}[/bold] · units [bold]{data.unit_count:,}[/bold] · "
            f"revid max {data.revid_max:,}")
        if data.complete:
            console.print(
                "[green]every unit traces to a source revision[/green] — "
                "[dim]the precondition for publishing at all[/dim]")
        else:
            die(f"{data.units_without_revid:,} unit(s) have no source revision",
                "attribution would claim a traceability that does not exist")
        if data.editors_resolved:
            console.print(f"resolved {data.editors_resolved:,} page editors")

        written = attribution_mod.render(
            data, found, paths.pack_dir,
            url_pattern=str(folio.get_meta("source_url_pattern") or ""),
            history_pattern=str(folio.get_meta("history_url_pattern") or ""),
            issue_url=issue_url, contact_email=contact,
        )
    for path in written:
        console.print(f"[green]{path}[/green]  {path.stat().st_size:,} bytes")
