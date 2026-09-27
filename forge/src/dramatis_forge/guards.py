"""Guards: every assumption in the pipeline is an assertion that can fire.

A rule table's characteristic failure is not being wrong — it is being wrong
*quietly*. Output still appears, counts still look plausible, and the loss is
found months later by someone reading a sample. Guards exist so that each
assumption has a name, a measured baseline, and a place to complain.

Severity means one thing only: **could content have been lost?** A high-severity
finding fails the release gate. Low-severity findings must be *attributed* — an
unexplained low-severity count is a high-severity finding in waiting.

    G1  counts        seed-set drift, and produced-vs-stored reconciliation
    G2  constructs    markup the rules did not predict
    G3  empty         a page in scope that yielded nothing
    G4  identity      page-to-person resolution invariants
    G5  corpus        chunking invariants (coverage, size, redundancy)

The artifact cannot disagree with itself about any of this, because every count a report
shows is read from the table that holds it rather than from a copy written beside it.

Expected sizes (G1 seed counts, G4 identity counts) are never declared in code. They are
measured, accepted into a baseline file with `forge baseline accept`, and compared here.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field

from .text import TEXT

HIGH = "high"
LOW = "low"

#: Every guard, in order. Descriptions are text (`guard.<id>` in `text.TEXT`), so a pack
#: can word them; these are the framework defaults.
GUARDS: dict[str, str] = {g: TEXT[f"guard.{g}"] for g in ("G1", "G2", "G3", "G4", "G5")}


@dataclass(frozen=True, slots=True)
class Finding:
    guard: str
    severity: str
    detail: str
    page: str | None = None

    def __str__(self) -> str:
        where = f" [{self.page}]" if self.page else ""
        return f"{self.guard}{where} {self.detail}"

    def row(self) -> tuple[str, str, str | None, str]:
        return (self.guard, self.severity, self.page, self.detail)


class Ledger:
    """Accumulates one stage's findings until they are written to the archive, where the
    tally is read from (`Archive.tally`)."""

    def __init__(self) -> None:
        self.findings: list[Finding] = []

    def add(self, guard: str, detail: str, *, page: str | None = None, high: bool = False) -> None:
        self.findings.append(Finding(guard, HIGH if high else LOW, detail, page))

    def extend(self, findings: Iterable[Finding]) -> None:
        self.findings.extend(findings)

    def rows(self) -> list[tuple[str, str, str | None, str]]:
        return [f.row() for f in self.findings]


# --------------------------------------------------------------------------- #
# G1
# --------------------------------------------------------------------------- #


def check_drift(
    counts: Mapping[str, int],
    baselines: Mapping[str, int] | None,
    *,
    guard: str = "G1",
    what: str = "count",
    labels: Mapping[str, str] | None = None,
) -> list[Finding]:
    """Compare measured sizes against accepted ones. Three outcomes, not two.

    There is no tolerance band. A band that suppresses small deviations entirely lets a
    set sit one item off its baseline with *no finding of any kind*, so the artifact
    records neither "aligned" nor "drifted" — and a condition phrased as "every set
    aligns with its baseline" cannot be checked against silence.

    `baselines=None` means nothing has been accepted yet: one low finding says so, rather
    than a finding per set that would bury everything else.

    So every deviation is now recorded, and the three cases are kept apart because they
    mean different things:

    * **fell** — the site removed something, or our enumerator broke. The second is far
      more likely, so it is high severity.
    * **grew** — new material ships; expected. Low severity, but *recorded*, because the
      baseline now needs updating and an unrecorded difference is indistinguishable from
      one nobody looked at.
    * **equal** — nothing to say.
    """
    if baselines is None:
        return [Finding(guard, LOW, "no accepted baseline — run `forge baseline accept` "
                                    "once the current counts have been reviewed")]
    out: list[Finding] = []
    for key, base in baselines.items():
        if key not in counts:
            continue  # measured by another stage
        n = counts[key]
        if n == base:
            continue
        where = f" ({labels[key]})" if labels and key in labels else ""
        if n < base:
            out.append(Finding(
                guard, HIGH,
                f"{key} {what} FELL: {base} → {n}{where} — either the source shrank or "
                "enumeration broke",
            ))
        else:
            out.append(Finding(
                guard, LOW,
                f"{key} {what} grew: {base} → {n} (+{n - base}){where} — growth; accept a "
                "new baseline once reviewed",
            ))
    for key in counts:
        if key not in baselines:
            out.append(Finding(guard, LOW, f"{key} has no accepted baseline: {counts[key]}"))
    return out


@dataclass
class Reconciliation:
    """Produced vs stored, per record kind.

    The manifest used to publish produced counts while the tables held fewer rows,
    with no way to tell whether the difference was deduplication or destruction.
    Both numbers are now recorded and the difference has to be explained by the
    ignored-insert count, or G1 fires.
    """

    produced: dict[str, int] = field(default_factory=dict)
    stored: dict[str, int] = field(default_factory=dict)
    ignored: dict[str, int] = field(default_factory=dict)

    def note(self, kind: str, *, produced: int = 0, stored: int = 0, ignored: int = 0) -> None:
        for name, value in (("produced", produced), ("stored", stored), ("ignored", ignored)):
            d = getattr(self, name)
            d[kind] = d.get(kind, 0) + value

    def check(self) -> list[Finding]:
        out: list[Finding] = []
        for kind, made in sorted(self.produced.items()):
            kept = self.stored.get(kind, 0)
            skipped = self.ignored.get(kind, 0)
            if made == kept + skipped:
                if skipped:
                    out.append(Finding(
                        "G1", LOW,
                        f"{kind}: {skipped} exact duplicates collapsed "
                        f"({made} produced → {kept} stored)",
                    ))
                continue
            out.append(Finding(
                "G1", HIGH,
                f"{kind}: {made} produced but {kept} stored and only {skipped} "
                f"explained as duplicates — {made - kept - skipped} rows unaccounted for",
            ))
        return out

    def as_dict(self) -> dict[str, dict[str, int]]:
        return {"produced": dict(self.produced), "stored": dict(self.stored),
                "ignored": dict(self.ignored)}
