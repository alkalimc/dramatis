"""Guards: every assumption in the pipeline is an assertion that can fire.

A rule table's characteristic failure is not being wrong — it is being wrong
*quietly*. Output still appears, counts still look plausible, and the loss is
found months later by someone reading a sample. Guards exist so that each
assumption has a name, a measured baseline, and a place to complain.

Severity means one thing only: **could content have been lost?** High-severity
findings block the design freeze. Low-severity findings must be *attributed* —
an unexplained low-severity count is a high-severity finding in waiting.

    G1  counts        seed-set drift, and produced-vs-stored reconciliation
    G2  constructs    markup the rules did not predict
    G3  empty         a page in scope that yielded nothing
    G4  identity      page-to-person resolution invariants
    G5  corpus        chunking invariants (coverage, size, redundancy)
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field

HIGH = "高"
LOW = "低"

GUARDS: dict[str, str] = {
    "G1": "counts: seed drift and produced-vs-stored reconciliation",
    "G2": "constructs: markup outside the declared rules",
    "G3": "empty: an in-scope page produced no records",
    "G4": "identity: page-to-person resolution invariants",
    "G5": "corpus: chunking coverage, size and redundancy invariants",
}


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
    """Accumulates findings and answers the only two questions that matter:
    is anything high-severity outstanding, and is every low-severity count
    attributed to a known cause?"""

    def __init__(self) -> None:
        self.findings: list[Finding] = []

    def add(self, guard: str, detail: str, *, page: str | None = None, high: bool = False) -> None:
        self.findings.append(Finding(guard, HIGH if high else LOW, detail, page))

    def extend(self, findings: Iterable[Finding]) -> None:
        self.findings.extend(findings)

    def high(self, guard: str | None = None) -> list[Finding]:
        return [f for f in self.findings
                if f.severity == HIGH and (guard is None or f.guard == guard)]

    def low(self, guard: str | None = None) -> list[Finding]:
        return [f for f in self.findings
                if f.severity == LOW and (guard is None or f.guard == guard)]

    def tally(self) -> dict[str, tuple[int, int]]:
        return {
            g: (len(self.high(g)), len(self.low(g)))
            for g in GUARDS
            if self.high(g) or self.low(g)
        }

    @property
    def clean(self) -> bool:
        """The design-freeze predicate: no high-severity finding anywhere."""
        return not self.high()

    def rows(self) -> list[tuple[str, str, str | None, str]]:
        return [f.row() for f in self.findings]


# --------------------------------------------------------------------------- #
# G1
# --------------------------------------------------------------------------- #


def check_drift(
    counts: Mapping[str, int],
    baselines: Mapping[str, int],
    *,
    tolerance: float = 0.02,
    labels: Mapping[str, str] | None = None,
) -> list[Finding]:
    """Compare seed-set sizes against measured baselines.

    Growth and shrinkage are both reported but they are not the same event.
    Growth is expected — new story ships. Shrinkage means either the site removed
    something or our enumerator broke, and the second is far more likely, so it
    is high severity while growth is low.
    """
    out: list[Finding] = []
    for key, base in baselines.items():
        if not base:
            continue
        n = counts.get(key, 0)
        if abs(n - base) / base <= tolerance:
            continue
        grew = n > base
        where = f"（{labels[key]}）" if labels and key in labels else ""
        out.append(Finding(
            "G1", LOW if grew else HIGH,
            f"{key} count {'grew' if grew else 'FELL'}: {base} → {n}{where}",
        ))
    for key in counts:
        if key not in baselines:
            out.append(Finding("G1", LOW, f"{key} has no baseline yet: {counts[key]}"))
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
