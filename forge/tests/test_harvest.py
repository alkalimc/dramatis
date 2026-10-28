"""Sync against a fake wiki: first sync, increments (changed / added / removed), a no-op
update, and a full re-sync all leave the archive in the same recorded shape."""

from __future__ import annotations

import pytest
from conftest import sync

from dramatis_forge import harvest
from dramatis_forge.archive import Archive


def meta(paths, key):
    with Archive(paths.archive, readonly=True) as a:
        return a.get_meta(key)


def test_first_sync_fetches_scope_and_sets_watermark(wiki, pack, paths):
    p, res = sync(wiki, pack, paths)
    assert p.first and p.full and res.fetched == 7
    assert meta(paths, "watermark") == wiki.rcid
    assert meta(paths, "last_update") == {
        "kind": "full", "first": True, "changed": [], "added": [], "removed": []}


def test_incremental_plan_then_apply(wiki, pack, paths):
    sync(wiki, pack, paths)
    wiki.edit("Chapter 1", "Alice: Rewritten.\n", story=True)
    wiki.edit("Chapter 3", "Bob: New story.\n", story=True)
    wiki.edit("Elsewhere", "not in any seed set")
    wiki.delete("Chapter 2")
    wiki.requested.clear()

    with Archive(paths.archive) as a:
        p = harvest.plan(wiki, a, pack)
        assert not p.full and p.watermark == 100
        assert (p.changed, p.added, p.gone) == (["Chapter 1"], ["Chapter 3"], ["Chapter 2"])
        assert p.ignored == 2  # the new story (found by re-scope instead) and the stray page
        assert a.page("Chapter 2") is not None, "planning must not touch the archive"
        harvest.apply(wiki, a, pack, p)

        assert sorted(wiki.requested) == ["Chapter 1", "Chapter 3"]
        assert a.page("Chapter 1")["wikitext"] == "Alice: Rewritten.\n"
        assert a.page("Chapter 2") is None
        [snap] = a.removed()
        assert snap["title"] == "Chapter 2" and "lighthouse" in snap["wikitext"]
        assert a.get_meta("watermark") == wiki.rcid
        assert a.get_meta("last_update") == {
            "kind": "incremental", "first": False,
            "changed": ["Chapter 1"], "added": ["Chapter 3"], "removed": ["Chapter 2"]}


def test_no_op_update_still_records_the_sync(wiki, pack, paths):
    sync(wiki, pack, paths)
    with Archive(paths.archive) as a:
        a.set_meta("synced_at", "earlier")
        a.set_meta("record_counts", {"line": 1})
        a.commit()
    wiki.requested.clear()
    p, res = sync(wiki, pack, paths)
    assert p.nothing_to_do and res.fetched == 0 and wiki.requested == []
    assert meta(paths, "synced_at") != "earlier"
    assert meta(paths, "record_counts_previous") == {"line": 1}
    assert meta(paths, "last_update")["kind"] == "incremental"
    assert meta(paths, "watermark") == wiki.rcid


def test_watermark_not_reached_is_high(wiki, pack, paths, monkeypatch):
    sync(wiki, pack, paths)
    for i in range(3):
        wiki.edit(f"Noise {i}", "x")
    monkeypatch.setattr(harvest, "FEED_LIMIT", 1)
    with Archive(paths.archive) as a:
        p = harvest.plan(wiki, a, pack, rescope=False)
    assert any(f.severity == "high" and "never reached watermark" in f.detail for f in p.findings)


def test_full_sync_refetches_everything_held(wiki, pack, paths):
    sync(wiki, pack, paths)
    wiki.requested.clear()
    p, _res = sync(wiki, pack, paths, full=True)
    assert p.full and not p.first
    assert sorted(wiki.requested) == sorted(wiki.pages)
    assert meta(paths, "last_update")["kind"] == "full"


def test_page_gone_from_site_is_dropped_with_snapshot(wiki, pack, paths):
    sync(wiki, pack, paths)
    del wiki.pages["Bob"]  # still listed by the fixed seed set, but the site has no page
    _p, res = sync(wiki, pack, paths, full=True)
    assert res.missing == ["Bob"]
    with Archive(paths.archive, readonly=True) as a:
        assert "Bob" not in a.have_pages()
        assert [r["title"] for r in a.removed()] == ["Bob"]


def test_an_edit_the_feed_never_listed_is_found_by_the_revision_check(wiki, pack, paths):
    sync(wiki, pack, paths)
    wiki.edit("Chapter 1", "Alice: Moved in silently.\n", story=True, log=False)
    wiki.edit("Chapter 2", "Bob: Listed in the feed.\n", story=True)
    p, res = sync(wiki, pack, paths)
    assert p.changed == ["Chapter 1", "Chapter 2"] and p.stale == ["Chapter 1"]
    assert any("newer on the site" in f.detail for f in p.findings)
    assert res.delta["changed"] == ["Chapter 1", "Chapter 2"]
    with Archive(paths.archive) as a:
        assert a.page("Chapter 1")["wikitext"] == "Alice: Moved in silently.\n"


def test_the_revision_check_is_quiet_when_nothing_moved(wiki, pack, paths):
    sync(wiki, pack, paths)
    p, _ = sync(wiki, pack, paths)
    assert p.stale == [] and p.nothing_to_do and p.findings == []


def test_the_discovered_set_is_what_the_last_run_found(wiki, pack, paths):
    from dataclasses import replace

    from dramatis_forge.pack import Followup, SeedSet

    hook = Followup(of_seed="story", seed="body",
                    discover=lambda title, text: [f"{title}/body"] if "[body]" in text else [])
    seeds = (*pack.seeds, SeedSet("body", "Bodies", "followup", discovered=True))
    q = replace(pack, seeds=seeds, followups=(hook,))
    wiki.edit("Chapter 1", "[body]\n", story=True, log=False)
    wiki.edit("Chapter 1/body", "Alice: Hello.\n", log=False)
    _, res = sync(wiki, q, paths)
    assert res.followups == {"body": 1} and not res.findings
    with Archive(paths.archive) as a:
        assert a.seed("body") == ["Chapter 1/body"]

    # the parent stops referring to its subpage: the page leaves the set and is reported
    wiki.edit("Chapter 1", "Alice: Inline now.\n", story=True)
    p, res = sync(wiki, q, paths)
    with Archive(paths.archive) as a:
        assert a.seed("body") == []
    assert [(f.guard, f.severity) for f in res.findings] == [("G1", "low")]
    assert "Chapter 1/body" in res.findings[0].detail


def test_only_throttling_and_server_errors_are_retried():
    import httpx

    from dramatis_forge import wiki

    def status(code):
        return httpx.HTTPStatusError("x", request=httpx.Request("GET", "http://x"),
                                     response=httpx.Response(code))

    assert all(wiki._retryable(status(c)) for c in (429, 500, 503))
    assert not any(wiki._retryable(status(c)) for c in (400, 403, 404))
    assert wiki._retryable(httpx.ConnectError("x")) and wiki._retryable(wiki.Throttled("x"))


def test_interrupted_sync_leaves_the_watermark_and_the_next_run_finishes(wiki, pack, paths):
    sync(wiki, pack, paths)
    wiki.edit("Chapter 1", "Alice: One.\n", story=True)
    wiki.edit("Chapter 2", "Bob: Two.\n", story=True)
    old = meta(paths, "watermark")
    real = wiki.content

    def dies_after_one(titles):
        for i, item in enumerate(real(titles)):
            if i == 1:
                raise KeyboardInterrupt
            yield item

    wiki.content = dies_after_one
    with pytest.raises(KeyboardInterrupt):
        sync(wiki, pack, paths)
    assert meta(paths, "watermark") == old, "an unfinished sync must not move the watermark"

    wiki.content = real
    sync(wiki, pack, paths)
    assert meta(paths, "watermark") == wiki.rcid
    with Archive(paths.archive, readonly=True) as a:
        assert a.page("Chapter 1")["wikitext"] == "Alice: One.\n"
        assert a.page("Chapter 2")["wikitext"] == "Bob: Two.\n"
