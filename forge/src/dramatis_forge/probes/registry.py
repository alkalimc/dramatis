"""Probes: the questions that must be answered before the design is frozen.

A probe is not a test. A test asserts that code does what it was written to do; a
probe answers a question about the world that a design decision is currently *assuming*
the answer to. The distinction matters because the failure modes differ: a broken test
tells you the code changed, whereas a failed probe tells you a document is lying.

Every probe therefore records four things, and the third is the one usually missing:

    question    what we do not know
    gates       **which decision becomes unsupported if this comes back wrong**
    criterion   what counts as pass, decided before running it
    needs       what it requires — archive, folio, network, weights, toolchain, hardware

`gates` is what stops the register from becoming a wish list. A probe that gates nothing
is curiosity; it can be interesting, but it does not block anything and should not be
allowed to look as if it does. `criterion` is written before the run for the obvious
reason: a threshold chosen afterwards is a description, not a test.

Probes that cannot run here are not omitted. They are registered with their blocker
named, because "we have not measured this" is a fact the design needs to state, and an
unregistered unknown is indistinguishable from a settled question.
"""

from __future__ import annotations

import datetime as dt
import json
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

PASS, FAIL, BLOCKED, UNRUN = "pass", "fail", "blocked", "unrun"

#: What a probe can require. Anything beyond `archive`/`folio` cannot run offline.
NEEDS = {
    "archive": "a normalised record archive",
    "folio": "a built corpus folio",
    "evals": "a built benchmark suite",
    "network": "access to the source wiki",
    "weights": "encoder / reranker / generator weights",
    "endpoint": "a reachable OpenAI-compatible endpoint",
    "toolchain": "a Rust or C++ toolchain",
    "hardware": "the target machine, measured cold and warm",
    "legal": "reading licence texts and writing the notices",
}


@dataclass
class Result:
    status: str = UNRUN
    measurements: dict[str, Any] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)
    #: Set when a probe's answer contradicts something a document currently asserts.
    contradicts: list[str] = field(default_factory=list)
    ran_at: str = ""

    def note(self, text: str) -> Result:
        self.notes.append(text)
        return self

    def contradiction(self, text: str) -> Result:
        self.contradicts.append(text)
        return self


@dataclass(frozen=True)
class Probe:
    id: str
    title: str
    question: str
    gates: str
    method: str
    criterion: str
    needs: tuple[str, ...]
    cost: str = "minutes"
    run: Callable[..., Result] | None = None

    @property
    def offline(self) -> bool:
        return all(n in ("archive", "folio", "evals") for n in self.needs)

    def blocked_on(self) -> tuple[str, ...]:
        return tuple(n for n in self.needs if n not in ("archive", "folio", "evals"))


class Register:
    def __init__(self) -> None:
        self.probes: dict[str, Probe] = {}

    def add(self, probe: Probe) -> Probe:
        if probe.id in self.probes:
            raise ValueError(f"duplicate probe id {probe.id}")
        self.probes[probe.id] = probe
        return probe

    def __iter__(self):
        return iter(sorted(self.probes.values(), key=lambda p: p.id))

    def __getitem__(self, pid: str) -> Probe:
        return self.probes[pid]

    def offline(self) -> list[Probe]:
        return [p for p in self if p.offline and p.run is not None]

    def blocked(self) -> list[Probe]:
        return [p for p in self if not p.offline]


REGISTER = Register()


def register(**kwargs: Any) -> Probe:
    return REGISTER.add(Probe(**kwargs))


def write_results(results: dict[str, Result], outdir: Path) -> Path:
    """Persist results as JSON next to the artifacts they measured.

    Results live with the artifacts rather than in the design repo: the design cites
    numbers, but the numbers belong to a build, and a number copied into prose with no
    build behind it is exactly what probes exist to prevent.
    """
    outdir.mkdir(parents=True, exist_ok=True)
    payload = {
        "written_at": dt.datetime.now(dt.UTC).isoformat(timespec="seconds"),
        "results": {
            pid: {
                "status": r.status,
                "measurements": r.measurements,
                "notes": r.notes,
                "contradicts": r.contradicts,
                "ran_at": r.ran_at,
            }
            for pid, r in sorted(results.items())
        },
    }
    path = outdir / "probe-results.json"
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    return path
