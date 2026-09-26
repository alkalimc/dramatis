"""`dramatis-forge` — one CLI, one stage per subcommand.

    harvest scope       enumerate the seed sets
    harvest fetch       retrieve page bodies (the only network-heavy stage)
    harvest sync        scope + fetch + normalize + samples, resumable end to end
    harvest update      incremental: re-fetch what changed, then normalize + samples
    normalize run       wikitext -> records, with the guards
    baseline accept     record the current counts as the guards' expected sizes
    baseline show       compare the current counts with the accepted ones
    corpus build        records -> folio
    evals build         folio -> benchmark suites
    report coverage     what was taken, what was not, and why
    report figures      measured figures, with targets; optionally audit documents
    report samples      a broad reading sample of the archive
    report inspect      one page at every stage, for reading
    report attribution  per-page attribution and the notice file

Every stage is idempotent and re-runnable. Stages that touch the network say so, and
none of them require a stage that does when their inputs are already on disk.
"""

from __future__ import annotations

import typer

from . import baseline, corpus, evals, harvest, normalize, report

app = typer.Typer(
    add_completion=False,
    no_args_is_help=True,
    help="Turn a collaborative wiki into a retrieval-grounded character corpus.",
)
app.add_typer(harvest.app, name="harvest", help="Talk to the wiki: scope, fetch, sync, update.")
app.add_typer(normalize.app, name="normalize", help="Wikitext to records, offline.")
app.add_typer(baseline.app, name="baseline", help="Accept or compare the guards' expected counts.")
app.add_typer(corpus.app, name="corpus", help="Records to a folio.")
app.add_typer(evals.app, name="evals", help="Build benchmark suites.")
app.add_typer(report.app, name="report", help="Coverage, figures, samples, inspection, attribution.")


def main() -> None:  # pragma: no cover - entry point
    app()


if __name__ == "__main__":  # pragma: no cover
    main()
