"""Guards, and the reconciliation that catches silent data loss.

Guard severity means one thing: could content have been lost? These tests pin that
meaning down, because a guard whose severity drifts stops being usable as a release gate.
"""

from __future__ import annotations

from dramatis_forge.normalize.guards import HIGH, LOW, Ledger, Reconciliation, check_drift


# ---- G1 drift ----

def test_shrinkage_is_high_growth_is_low():
    """Growth is expected — new material ships. Shrinkage means the site removed something
    or our enumerator broke, and the second is far more likely."""
    grown = check_drift({"A": 130}, {"A": 100})
    fell = check_drift({"A": 70}, {"A": 100})
    assert [f.severity for f in grown] == [LOW]
    assert [f.severity for f in fell] == [HIGH]


def test_only_an_exact_match_is_silent():
    """There is no tolerance band, and that is the point.

    A band that suppresses small deviations lets a set sit one item off its baseline with
    no finding of any kind, so the artifact records neither "aligned" nor "drifted". A
    condition phrased as "every set aligns with its baseline" cannot be checked against
    silence, so a small growth must still be *recorded* — quietly, but recorded.
    """
    assert check_drift({"A": 100}, {"A": 100}) == []

    tiny_growth = check_drift({"A": 101}, {"A": 100})
    assert [f.severity for f in tiny_growth] == [LOW]
    assert "+1" in tiny_growth[0].detail

    tiny_fall = check_drift({"A": 99}, {"A": 100})
    assert [f.severity for f in tiny_fall] == [HIGH]


def test_a_set_with_no_baseline_is_reported_not_ignored():
    """An unmeasured set is a fact worth surfacing, not an exemption to hide."""
    findings = check_drift({"B": 12}, {"A": 3})
    assert findings and "no accepted baseline" in findings[0].detail


def test_no_accepted_baseline_is_one_low_finding_not_one_per_set():
    findings = check_drift({"A": 1, "B": 2, "C": 3}, None)
    assert [f.severity for f in findings] == [LOW]
    assert "baseline accept" in findings[0].detail


def test_a_baseline_key_another_stage_measures_is_not_reported_missing():
    """Discovered sets are counted after fetch; scope must not call them "fell to zero"."""
    assert check_drift({"A": 5}, {"A": 5, "B": 9}) == []


def test_drift_can_speak_for_another_guard():
    findings = check_drift({"persons": 9}, {"persons": 10}, guard="G4")
    assert [(f.guard, f.severity) for f in findings] == [("G4", HIGH)]


def test_severity_values_are_english_data():
    assert (HIGH, LOW) == ("high", "low")


# ---- G1 reconciliation ----

def test_duplicates_are_explained_and_low():
    """The whole point: a difference that duplicates account for is bookkeeping."""
    recon = Reconciliation()
    recon.note("lore", produced=100, stored=98, ignored=2)
    findings = recon.check()
    assert [f.severity for f in findings] == [LOW]
    assert "2 exact duplicates" in findings[0].detail


def test_unexplained_loss_is_high():
    """This is the failure the old pipeline had: rows destroyed by a key collision while
    the manifest reported the produced count and looked fine."""
    recon = Reconciliation()
    recon.note("lore", produced=100, stored=90, ignored=0)
    findings = recon.check()
    assert [f.severity for f in findings] == [HIGH]
    assert "unaccounted for" in findings[0].detail


def test_exact_match_is_silent():
    recon = Reconciliation()
    recon.note("lore", produced=100, stored=100, ignored=0)
    assert recon.check() == []


# ---- the ledger ----

def test_clean_means_no_high_severity_anywhere():
    ledger = Ledger()
    ledger.add("G2", "an odd construct")
    ledger.add("G3", "an empty page, explained")
    assert ledger.clean
    ledger.add("G3", "an empty page with no reason", high=True)
    assert not ledger.clean


def test_tally_reports_both_severities_per_guard():
    ledger = Ledger()
    ledger.add("G2", "a", high=True)
    ledger.add("G2", "b")
    ledger.add("G3", "c")
    assert ledger.tally() == {"G2": (1, 1), "G3": (0, 1)}


def test_guards_with_no_findings_are_absent_from_the_tally():
    """So an empty tally reads as "nothing to look at" rather than a wall of zeroes."""
    ledger = Ledger()
    ledger.add("G2", "a")
    assert set(ledger.tally()) == {"G2"}
