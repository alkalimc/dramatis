"""Source order, transcluded bodies, voice variants and the yield guard, on toy records."""

from __future__ import annotations

import json

import pytest
from conftest import FakeWiki, build, sync

from dramatis_forge.archive import Archive
from dramatis_forge.folio import Folio
from dramatis_forge.pack import Route
from dramatis_forge.records import Lore, Scene, Voice
from dramatis_forge.report import inspect

#: Headings that sort differently from how the page lists them.
ZEBRA = (
    "Zebra lead text that is long enough to keep as a section on its own.\n"
    "== Zulu ==\nThe first section on the page, and long enough to be kept.\n"
    "== Alpha ==\nThe second section on the page, also long enough to be kept.\n"
    "== Mike ==\nThe third section on the page, long enough as well to be kept.\n"
)


def test_records_and_units_keep_the_page_order(pack, paths):
    wiki = FakeWiki(people={"Alice": ZEBRA, "Alice (Winter)": "{{AltForm|Alice}}\n", "Bob": ""})
    sync(wiki, pack, paths)
    build(pack, paths)
    with Archive(paths.archive, readonly=True) as a:
        stored = [json.loads(r[0]) for r in a.db.execute(
            "SELECT path FROM lore WHERE page='Alice' ORDER BY seq")]
        view = [s["path"] for s in inspect.collect(a, "Alice", pack)["lore"]]
    assert stored == view == [["lead"], ["Zulu"], ["Alpha"], ["Mike"]]
    with Folio(paths.folio, readonly=True) as f:
        titles = [r[0] for r in f.db.execute(
            "SELECT title FROM chunks WHERE page='Alice' AND template='lore' ORDER BY ord")]
    assert titles == ["lead", "Zulu", "Alpha", "Mike"]


def test_a_short_page_with_little_yield_is_reported_not_failed(pack, paths):
    # A page whose one kept section is a tiny share of what it shows.
    long_table = "{|\n" + "".join(f"|-\n| row {i} with words the parser skips\n" for i in range(40))
    wiki = FakeWiki(people={"Alice": long_table + "|}\n== Kept ==\nOnly this one paragraph of the page is kept by the parser.\n",
                            "Alice (Winter)": "{{AltForm|Alice}}\n", "Bob": ""})
    sync(wiki, pack, paths)
    build(pack, paths)
    with Archive(paths.archive, readonly=True) as a:
        [(severity, detail)] = a.db.execute(
            "SELECT severity, detail FROM guard_findings WHERE page='Alice' AND guard='G3'")
    assert severity == "low" and detail.startswith("yield ") and "floor of 50%" in detail


def test_the_yield_check_is_off_unless_a_route_sets_a_floor():
    assert Route("s", parser=lambda ctx: ()).min_yield == 0.0


def test_a_transcluded_body_is_stored_once_and_shown_from_both_pages(tmp_path, pack):
    with Archive(tmp_path / "t.archive") as a:
        a.insert_records([Scene(page="Tale", revid=2, source_page="Tale/data")])
        a.db.execute("INSERT INTO lines(scene,seq,speaker,text,kind) VALUES('Tale',0,'Ann','Hi','s')")
        a.put_page("Tale", "wrapper", 1, "now")
        a.put_page("Tale/data", "Ann: Hi", 2, "now")
        for page in ("Tale", "Tale/data"):
            got = inspect.collect(a, page, pack)
            assert got["scene"]["id"] == "Tale" and [ln["text"] for ln in got["lines"]] == ["Hi"]
        md = inspect.render(pack, "Tale/data", {"wikitext": "x", "revid": 2},
                            inspect.collect(a, "Tale/data", pack))
    assert "`Tale/data`" in md


def test_voice_variants_share_a_slot(tmp_path):
    with Archive(tmp_path / "t.archive") as a:
        stored, ignored = a.insert_records([
            Voice(page="P", subject="Ann", idx=1, text="base line", seq=0),
            Voice(page="P", subject="Ann", idx=1, text="outfit line", variant="outfit", seq=1),
        ])
    assert (stored, ignored) == (2, 0)


@pytest.mark.parametrize("record", [
    Lore(page="P", path=("A",), text="body"),
    Voice(page="P", subject="S", idx=0, text="said", title="t"),
])
def test_every_record_exposes_its_prose(record):
    assert ("body" in record.prose) or ("said" in record.prose and "t" in record.prose)
