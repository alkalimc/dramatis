"""Guards, and the reconciliation that catches silent data loss.

Guard severity means one thing: could content have been lost? These tests pin that
meaning down, because a guard whose severity drifts stops being usable as a freeze gate.
"""

from __future__ import annotations

from dramatis_forge.normalize.guards import HIGH, LOW, Ledger, Reconciliation, check_drift


# ---- G1 drift ----

def test_shrinkage_is_high_growth_is_low():
    """Growth is expected — new story ships. Shrinkage means the site removed something
    or our enumerator broke, and the second is far more likely."""
    grown = check_drift({"S1": 2200}, {"S1": 2086})
    fell = check_drift({"S1": 1900}, {"S1": 2086})
    assert [f.severity for f in grown] == [LOW]
    assert [f.severity for f in fell] == [HIGH]


def test_only_an_exact_match_is_silent():
    """There is no tolerance band, and that is the point.

    A band used to suppress small deviations, and it hid a real one: a seed set sat one
    page above its baseline with no finding of any kind, so the artifact recorded neither
    "aligned" nor "drifted". A freeze condition phrased as "every seed set aligns with its
    baseline" cannot be checked against silence, so a small growth must still be *recorded*
    — quietly, but recorded.
    """
    assert check_drift({"S1": 2086}, {"S1": 2086}) == []

    tiny_growth = check_drift({"S1": 2087}, {"S1": 2086})
    assert [f.severity for f in tiny_growth] == [LOW]
    assert "+1" in tiny_growth[0].detail

    tiny_fall = check_drift({"S1": 2085}, {"S1": 2086})
    assert [f.severity for f in tiny_fall] == [HIGH]


def test_a_set_with_no_baseline_is_reported_not_ignored():
    """An unmeasured set is a fact worth surfacing, not an exemption to hide."""
    findings = check_drift({"S9": 12}, {})
    assert findings and "no baseline" in findings[0].detail


def test_zero_baseline_is_exempt():
    assert check_drift({"S9": 12}, {"S9": 0}) == []


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
