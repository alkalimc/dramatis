"""`dramatis-forge` — one CLI, one stage per subcommand.

    harvest scope       enumerate the seed sets
    harvest fetch       retrieve page bodies (the only network-heavy stage)
    harvest sync        scope + fetch + normalize, resumable end to end
    harvest update      incremental: re-fetch what changed
    normalize run       wikitext -> records, with the guards
    corpus build        records -> folio
    evals build         folio -> benchmark suites
    probe run           answer the design's open questions
    report coverage     what was taken, what was not, and why
    report inspect      one page at every stage, for reading
    store migrate       carry inputs forward from a previous-generation archive

Every stage is idempotent and re-runnable. Stages that touch the network say so, and
none of them require a stage that does when their inputs are already on disk.
"""

from __future__ import annotations

import typer

from . import corpus, evals, harvest, normalize, probe, report, store

app = typer.Typer(
    add_completion=False,
    no_args_is_help=True,
    help="Turn a collaborative wiki into a retrieval-grounded character corpus.",
)
app.add_typer(harvest.app, name="harvest", help="Talk to the wiki: scope, fetch, sync, update.")
app.add_typer(normalize.app, name="normalize", help="Wikitext to records, offline.")
app.add_typer(corpus.app, name="corpus", help="Records to a folio.")
app.add_typer(evals.app, name="evals", help="Build benchmark suites.")
app.add_typer(probe.app, name="probe", help="Answer the design's open questions.")
app.add_typer(report.app, name="report", help="Coverage report and per-page inspection.")
app.add_typer(store.app, name="store", help="Archive maintenance and migration.")


def main() -> None:  # pragma: no cover - entry point
    app()


if __name__ == "__main__":  # pragma: no cover
    main()
