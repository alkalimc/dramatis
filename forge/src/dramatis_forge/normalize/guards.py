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
    G6  self          the artifact agrees with its own bookkeeping

G6 is the odd one out: G1–G5 check the corpus, G6 checks *us*. It exists because a
probe found three places where this artifact disagreed with itself — a findings table
holding one high-severity row while the manifest tally published zero, a seed count
published from a snapshot taken before the seeds were discovered, and a drift of one
page that no guard recorded at all. None of them lost content. All three made the
artifact impossible to audit, and two freeze conditions are stated in terms of numbers
the artifact reports about itself.
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
    "G6": "self: the artifact agrees with its own bookkeeping",
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
    labels: Mapping[str, str] | None = None,
) -> list[Finding]:
    """Compare seed-set sizes against measured baselines. Three outcomes, not two.

    There used to be a tolerance band that suppressed small deviations entirely, and it
    hid a real one: a seed set sat one page above its baseline with *no finding of any
    kind*, so the artifact recorded neither "aligned" nor "drifted". A condition phrased
    as "every seed set aligns with its baseline" cannot be checked against silence.

    So every deviation is now recorded, and the three cases are kept apart because they
    mean different things:

    * **fell** — the site removed something, or our enumerator broke. The second is far
      more likely, so it is high severity.
    * **grew** — new material ships; expected. Low severity, but *recorded*, because the
      baseline now needs updating and an unrecorded difference is indistinguishable from
      one nobody looked at.
    * **equal** — nothing to say.
    """
    out: list[Finding] = []
    for key, base in baselines.items():
        if not base:
            continue
        n = counts.get(key, 0)
        if n == base:
            continue
        where = f"（{labels[key]}）" if labels and key in labels else ""
        if n < base:
            out.append(Finding(
                "G1", HIGH,
                f"{key} count FELL: {base} → {n}{where} — either the source shrank or "
                "enumeration broke",
            ))
        else:
            out.append(Finding(
                "G1", LOW,
                f"{key} count grew: {base} → {n} (+{n - base}){where} — monotonic growth, "
                "baseline needs updating",
            ))
    for key in counts:
        if key not in baselines:
            out.append(Finding("G1", LOW, f"{key} has no baseline yet: {counts[key]}"))
    return out


# --------------------------------------------------------------------------- #
# G6
# --------------------------------------------------------------------------- #


def check_self_consistency(
    *,
    table_tally: Mapping[str, tuple[int, int]],
    manifest_tally: Mapping[str, Sequence[int]],
    table_seed_counts: Mapping[str, int],
    manifest_seed_counts: Mapping[str, int],
) -> list[Finding]:
    """Reconcile what the tables hold against what the manifest publishes.

    Every other guard reads the corpus. This one reads the artifact's own account of
    itself, because that account is what the freeze conditions are stated in terms of:
    "high-severity guard findings are zero" is a claim the artifact makes *about itself*,
    and nothing verified it. A file asserting its own cleanliness is not evidence.

    Any disagreement is high severity regardless of direction. The question is not which
    number is right — it is whether the artifact can be audited at all, and one that
    contradicts itself cannot be.
    """
    out: list[Finding] = []

    for guard in sorted(set(table_tally) | set(manifest_tally)):
        in_table = tuple(table_tally.get(guard, (0, 0)))
        published = tuple(manifest_tally.get(guard, (0, 0)))[:2]
        if in_table != published:
            out.append(Finding(
                "G6", HIGH,
                f"{guard}: findings table holds {in_table} (high, low) but the manifest "
                f"publishes {published} — the artifact disagrees with itself",
            ))

    for seed in sorted(set(table_seed_counts) | set(manifest_seed_counts)):
        held = table_seed_counts.get(seed, 0)
        published = manifest_seed_counts.get(seed, 0)
        if held != published:
            out.append(Finding(
                "G6", HIGH,
                f"{seed}: seeds table holds {held} rows but the manifest publishes "
                f"{published} — most likely a snapshot taken before a later stage "
                "added to the set",
            ))
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
