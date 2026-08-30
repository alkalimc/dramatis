"""The probe register and its runner.

Import `offline` for its side effect: registering every probe. Probes that cannot run
in a given environment stay in the register with their blocker named, so the report
distinguishes "answered", "answered badly", and "still unknown" — three states that a
prose checklist collapses into one.
"""

from __future__ import annotations

import inspect
from pathlib import Path

from ..pack import Pack
from ..store.archive import Archive
from ..store.folio import Folio
from . import offline as _offline  # noqa: F401  (registers the catalogue)
from .registry import BLOCKED, FAIL, PASS, REGISTER, UNRUN, Probe, Result, write_results

__all__ = [
    "BLOCKED", "FAIL", "PASS", "REGISTER", "UNRUN",
    "Probe", "Result", "run_all", "write_results",
]


def run_all(
    *,
    archive_path: Path,
    folio_path: Path,
    evals_dir: Path,
    workdir: Path,
    pack: Pack,
    only: list[str] | None = None,
    progress=None,
) -> dict[str, Result]:
    """Run every offline probe whose inputs exist; register the rest as blocked."""
    results: dict[str, Result] = {}
    have = {
        "archive": archive_path.exists(),
        "folio": folio_path.exists(),
        "evals": (evals_dir / "structural.manifest.json").exists(),
    }

    archive: Archive | None = None
    folio: Folio | None = None
    try:
        for probe in REGISTER:
            if only and probe.id not in only:
                continue
            if probe.run is None:
                results[probe.id] = Result(status=BLOCKED).note(
                    "needs " + ", ".join(probe.blocked_on()))
                continue
            missing = [n for n in probe.needs if not have.get(n, False)]
            if missing:
                results[probe.id] = Result(status=BLOCKED).note(
                    f"inputs not built: {', '.join(missing)}")
                continue

            # Opened lazily and shared: several probes read the same 80 MB file, and
            # read-only is deliberate — a probe must not be able to change what it measures.
            if archive is None and "archive" in probe.needs:
                archive = Archive(archive_path, readonly=True)
            if folio is None and "folio" in probe.needs:
                folio = Folio(folio_path, readonly=True)

            if progress is not None:
                progress(f"{probe.id} {probe.title}")
            kwargs = {
                "archive": archive, "folio": folio, "pack": pack,
                "evals_dir": evals_dir, "workdir": workdir,
            }
            accepted = inspect.signature(probe.run).parameters
            if not any(p.kind is p.VAR_KEYWORD for p in accepted.values()):
                kwargs = {k: v for k, v in kwargs.items() if k in accepted}
            try:
                results[probe.id] = probe.run(**kwargs)
            except Exception as exc:  # a probe that crashes is an unanswered question
                results[probe.id] = Result(status=FAIL).note(f"probe raised {exc!r}")
    finally:
        if archive is not None:
            archive.close()
        if folio is not None:
            folio.close()
    return results
