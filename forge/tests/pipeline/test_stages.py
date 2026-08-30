"""Cross-stage contracts.

These guard the seams rather than any single function: what `scope` may clear, what
`fetch` discovers, how aliases resolve through identity, and what the incremental update
is allowed to ignore.
"""

from __future__ import annotations

import pytest

from dramatis_forge.harvest import scope as scope_stage
from dramatis_forge.harvest import update as update_stage
from dramatis_forge.harvest.scope import Scope
from dramatis_forge.normalize.runner import run as normalize_run
from dramatis_forge.store.archive import Archive


class FakeWiki:
    """Enough of the client for the stages under test. Deliberately not a mock library:
    the shapes returned here are the shapes the real API returns, so a change in
    expectation shows up as a change in this file."""

    def __init__(self, changes: list[dict] | None = None) -> None:
        self.changes = changes or []
        self.requests = 0

    def recentchanges(self, namespaces, *, types="edit|new"):
        yield from self.changes

    def latest_change_id(self, namespaces) -> int:
        return max((c["rcid"] for c in self.changes), default=0)

    def redirect_targets(self, titles):
        return {}

    def links(self, titles, ns=0):
        return {}

    def cargo(self, table, fields, *, limit=500):
        return []


# ---- scope ----

def test_scope_writes_alias_sources_to_tables(archive: Archive, pack):
    sc = Scope(
        seeds={"S2": ["阿米娅"]},
        redirects={"兔子": "阿米娅"},
        disambigs={"米兰": ["凯尔希"]},
    )
    scope_stage.write(archive, sc, pack)
    assert archive.redirects() == {"兔子": "阿米娅"}
    assert archive.seed("S2") == ["阿米娅"]


def test_scope_does_not_clear_discovered_sets(archive: Archive, pack):
    """The bug this pins: `scope` used to wipe the whole seed table, so the two
    transcluded story bodies found during fetch were forgotten on every re-run."""
    archive.add_seeds("S1D", ["梅尔/干员密录/1/data"])
    scope_stage.write(archive, Scope(seeds={"S1": ["剧情页"]}), pack)
    assert archive.seed("S1D") == ["梅尔/干员密录/1/data"]


def test_scope_reports_overlap_between_corpus_sets(pack):
    """Not automatically wrong, but it must be a decision rather than an accident: two
    readers would parse the same page."""
    sc = Scope(seeds={"S1": ["共享页"], "S4": ["共享页"]})
    findings = scope_stage._overlap_findings(sc, pack)
    assert findings and "共享页" in findings[0].detail


# ---- normalize ----

def test_aliases_resolve_through_identity(archive: Archive, pack):
    """A redirect aimed at an alternate form must land on the person, or the alias
    dictionary silently disagrees with the roster."""
    archive.write_seeds({"S2": ["安洁莉娜", "予愿安洁莉娜"]})
    archive.put_page("安洁莉娜", "{{CharinfoV2|干员名=安洁莉娜}}", 1, "now")
    archive.put_page("予愿安洁莉娜", "{{异格干员|原型=安洁莉娜}}", 2, "now")
    archive.write_aliases_source({"信使": "予愿安洁莉娜"}, {})
    archive.commit()

    normalize_run(archive, pack)
    targets = dict(archive.db.execute(
        "SELECT alias,target FROM aliases WHERE kind='redirect'"))
    assert targets.get("信使") == "安洁莉娜"


def test_aliases_pointing_outside_the_archive_are_dropped(archive: Archive, pack):
    """Normalising a query onto something unretrievable is worse than not recognising
    the alias. Measured: most redirects point at levels and items, out of scope by
    decision."""
    archive.write_seeds({"S2": ["阿米娅"]})
    archive.put_page("阿米娅", "{{CharinfoV2|干员名=阿米娅}}", 1, "now")
    archive.write_aliases_source({"兔子": "阿米娅", "0-1": "0-1 坍塌"}, {})
    archive.commit()

    normalize_run(archive, pack)
    aliases = {r["alias"] for r in archive.db.execute("SELECT alias FROM aliases")}
    assert "兔子" in aliases and "0-1" not in aliases


def test_an_unroutable_page_is_reported_not_silently_skipped(archive: Archive, pack):
    archive.write_seeds({"S5R": ["干员剧情一览/六星干员"]})
    archive.put_page("干员剧情一览/六星干员", "内容", 1, "now")
    archive.commit()
    report = normalize_run(archive, pack)
    # S5R is not a fetch seed, so it is not parsed at all — and that must be visible
    assert report.pages_seen == 0


def test_a_parser_yielding_nothing_without_a_reason_is_high_severity(archive: Archive, pack):
    """The distinction guard G3 exists for: nothing-with-a-reason is a fact about the
    source; nothing-in-silence is a failure."""
    archive.write_seeds({"S5": ["泰拉词库"]})
    archive.put_page("泰拉词库", "没有任何表格的正文", 1, "now")
    archive.commit()
    report = normalize_run(archive, pack)
    assert any(f.guard == "G3" for f in report.ledger.findings)


# ---- update ----

def test_update_ignores_changes_outside_the_seed_sets(archive: Archive, pack):
    """The payoff of enumerate-don't-classify: an out-of-scope change needs no
    identification to be dismissed."""
    archive.write_seeds({"S1": ["剧情页"], "S2": ["阿米娅"]})
    archive.set_meta("watermark", 100)
    archive.commit()

    wiki = FakeWiki([
        {"title": "阿米娅", "rcid": 105},
        {"title": "0-1 坍塌", "rcid": 104},
        {"title": "旧变更", "rcid": 99},
    ])
    plan = update_stage.plan(wiki, archive, pack, rescope=False)
    assert plan.changed == ["阿米娅"]
    assert plan.ignored == 1
    assert plan.new_watermark == 105


def test_update_warns_when_the_watermark_is_out_of_reach(archive: Archive, pack):
    """Beyond the scan limit the increment is incomplete, and a silent partial update is
    worse than none."""
    archive.write_seeds({"S1": ["剧情页"]})
    archive.set_meta("watermark", 1)
    archive.commit()

    wiki = FakeWiki([{"title": f"p{i}", "rcid": 10_000 - i} for i in range(200)])
    plan = update_stage.plan(wiki, archive, pack, limit=50, rescope=False)
    assert any("watermark" in f.detail for f in plan.findings)


def test_first_update_pins_the_watermark_instead_of_replaying_history(archive: Archive, pack):
    wiki = FakeWiki([{"title": "x", "rcid": 4242}])
    assert update_stage.init_watermark(wiki, archive, pack) == 4242
    assert archive.get_meta("watermark") == 4242


def test_nothing_to_do_is_distinguishable_from_a_failure(archive: Archive, pack):
    archive.write_seeds({"S1": ["剧情页"]})
    archive.set_meta("watermark", 500)
    archive.commit()
    plan = update_stage.plan(FakeWiki([{"title": "x", "rcid": 400}]), archive, pack, rescope=False)
    assert plan.nothing_to_do
