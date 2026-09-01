"""Corpus command: records to a folio."""

from __future__ import annotations

from typing import Annotated

import typer
from rich.table import Table

from ..corpus.build import run as corpus_run
from ..store.archive import Archive
from .common import HomeOpt, PackOpt, console, need, render_guards, resolve, ticker

app = typer.Typer(no_args_is_help=True)


@app.command("build")
def build_cmd(
    pack: PackOpt = "",
    home: HomeOpt = None,
    segmenter: Annotated[
        str, typer.Option("--segmenter", help="jieba | none | auto")
    ] = "auto",
) -> None:
    """Build the folio: units, lexical index, roster, aliases, stats, manifest.

    Vectors are not written here — encoding needs model weights, and the unit set is the
    part worth iterating on. `corpus encode` fills them in afterwards.
    """
    pk, paths = resolve(pack, home)
    need(paths.archive, "archive", "run `dramatis-forge harvest sync` first")

    with Archive(paths.archive, readonly=True) as archive:
        report = corpus_run(archive, pk, paths.folio, segmenter=segmenter,
                            progress=ticker("corpus"))

    table = Table(title="retrieval units")
    table.add_column("template")
    table.add_column("units", justify="right")
    table.add_column("body p50", justify="right")
    table.add_column("body p95", justify="right")
    table.add_column("embed p50", justify="right")
    for name, n in sorted(report.by_template.items(), key=lambda kv: -kv[1]):
        stats = report.size_stats.get(name, {})
        table.add_row(
            name, f"{n:,}",
            str(stats.get("body_p50", "—")), str(stats.get("body_p95", "—")),
            str(stats.get("embed_p50", "—")),
        )
    console.print(table)

    console.print(
        f"units [bold]{report.chunks:,}[/bold] · "
        f"text {report.total_chars / 1e6:.2f} M chars · "
        f"segmenter {report.segmenter}")
    console.print(
        "vectors: "
        + " · ".join(f"{dim}d {size / 1e6:.0f} MB"
                     for dim, size in sorted(report.vector_bytes.items(), reverse=True)))
    if report.requirements:
        console.print(
            f"folio requires: [cyan]{', '.join(report.requirements)}[/cyan] — "
            "[dim]a reader that does not implement these should refuse to load it[/dim]")
    render_guards(report.ledger)
    console.print(
        f"wrote [cyan]{paths.folio}[/cyan] "
        f"({paths.folio.stat().st_size / 1e6:.0f} MB, vectors not yet written)")
    console.print("  next: [bold]dramatis-forge evals build[/bold]")
