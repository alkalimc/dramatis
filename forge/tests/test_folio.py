"""The built folio: units, lexical index, roster, manifest shape and a reproducible
fingerprint. Also the forge side of the forge → engine contract (see `make contract`)."""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
from pathlib import Path

import pytest

from conftest import KNOWN_TERM, build

from dramatis_forge import corpus, segment
from dramatis_forge.archive import Archive
from dramatis_forge.folio import FOLIO_FORMAT_VERSION, Folio
from dramatis_forge.pack import PersonSource
from dramatis_forge.params import Params


def manifest(path: Path) -> dict:
    with Folio(path, readonly=True) as f:
        return f.manifest()


def test_units_by_template_with_spans(built):
    with Folio(built.folio, readonly=True) as f:
        by = dict(f.db.execute("SELECT template, COUNT(*) FROM chunks GROUP BY 1"))
        assert by == {"lore": 5, "dialogue": 8, "profile": 2}
        spans = [tuple(r) for r in f.db.execute(
            "SELECT span_from, span_to FROM chunks WHERE span_of='Chapter 1' ORDER BY span_from")]
        # Non-overlapping and contiguous: neighbour expansion depends on it.
        assert all(a[1] + 1 == b[0] for a, b in zip(spans, spans[1:], strict=False))
        assert f.db.execute("SELECT COUNT(*) FROM chunks WHERE revid IS NULL").fetchone()[0] == 0
        ords = [r[0] for r in f.db.execute("SELECT ord FROM chunks ORDER BY ord")]
        assert ords == list(range(len(ords)))


def test_fts_rows_equal_units_and_match(built):
    with Folio(built.folio, readonly=True) as f:
        units = f.db.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
        assert f.db.execute("SELECT COUNT(*) FROM chunks_fts").fetchone()[0] == units
        [(text,)] = f.db.execute(
            "SELECT c.text FROM chunks_fts JOIN chunks c ON c.ord = chunks_fts.rowid "
            "WHERE chunks_fts MATCH ?", (KNOWN_TERM,)).fetchall()
        assert KNOWN_TERM in text


def test_roster_and_alias_reach_the_person(built):
    with Folio(built.folio, readonly=True) as f:
        forms = f.db.execute("SELECT forms FROM persons WHERE person_id='Alice'").fetchone()[0]
        assert '"Alice (Winter)"' in forms
        assert tuple(f.db.execute("SELECT * FROM aliases").fetchone()) == (
            "Alice (Winter)", "Alice", "form")


def test_manifest_shape(built, pack):
    m = manifest(built.folio)
    assert m["format_version"] == FOLIO_FORMAT_VERSION
    assert "neighbor_expand" in m["requires"]
    assert m["segmenter"] == "char-bigram/1"
    assert m["stopwords"] == list(pack.stopwords)
    assert m["template_shapes"] == {"lore": "lore", "dialogue": "dialogue", "profile": "profile"}
    assert m["chunk_count"] == 15 and m["build_fingerprint"].startswith("sha256:")
    assert m["requires"] == ["neighbor_expand"]
    assert m["wording"] == dict(pack.wording) and m["scene_marker"] == ["*", "*"]
    assert m["clock.year_offset"] == 100
    assert m["user_placeholder"] == "{user}"


def test_v2_tables_exist_and_enforce_their_checks(built):
    with Folio(built.folio, readonly=True) as f:
        tables = {r[0] for r in f.db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        assert {"unit_persons", "cooccur", "knowledge_scope", "topic_terms"} <= tables
        cols = {r[1] for r in f.db.execute("PRAGMA table_info(persons)")}
        assert {"birthday", "persona_confidence"} <= cols and "confidence" not in cols
        assert "person" not in {r[1] for r in f.db.execute("PRAGMA table_info(chunks)")}
    with sqlite3.connect(built.folio) as con:
        for bad in (
            "INSERT INTO cooccur VALUES ('b', 'a', 1)",
            "INSERT INTO knowledge_scope VALUES ('a', 'c', 'heard')",
            "INSERT INTO prompts(subject, slot, body) VALUES ('a', 'schedule', '')",
            "UPDATE persons SET birthday = '2024-01-01'",
        ):
            with pytest.raises(sqlite3.IntegrityError):
                con.execute(bad)
    con.close()


def rows(path: Path, sql: str) -> list[tuple]:
    with Folio(path, readonly=True) as f:
        return [tuple(r) for r in f.db.execute(sql)]


def test_unit_persons_holds_every_roster_speaker(built):
    """A dialogue unit belongs to all of its roster speakers; names off the roster
    (Carol) and units nobody speaks in carry no row."""
    per_unit = rows(built.folio,
                    "SELECT c.text, group_concat(u.person_id) FROM chunks c "
                    "LEFT JOIN unit_persons u ON u.chunk_id = c.id "
                    "WHERE c.template = 'dialogue' GROUP BY c.id ORDER BY c.ord")
    shared = [t for t, who in per_unit if who and set(who.split(",")) == {"Alice", "Bob"}]
    assert shared and all("Alice:" in t and "Bob:" in t for t in shared)
    assert all("Carol" not in (who or "") for _t, who in per_unit)
    assert rows(built.folio, "SELECT DISTINCT person_id FROM unit_persons ORDER BY 1") == [
        ("Alice",), ("Bob",)]


def test_cooccur_and_knowledge_scope(built):
    assert rows(built.folio, "SELECT * FROM cooccur") == [("Alice", "Bob", 2)]
    kinds = dict(rows(built.folio, "SELECT kind, COUNT(*) FROM knowledge_scope GROUP BY 1"))
    attributed = rows(built.folio, "SELECT COUNT(*) FROM unit_persons")[0][0]
    assert kinds["self"] == attributed and kinds["lived"] > 0
    # Lived = other dialogue units of a scene the person speaks in, never outside it.
    outside = rows(built.folio,
                   "SELECT COUNT(*) FROM knowledge_scope k JOIN chunks c ON c.id = k.chunk_id "
                   "WHERE k.kind = 'lived' AND (c.template <> 'dialogue' OR c.span_of NOT IN ("
                   "  SELECT c2.span_of FROM unit_persons u JOIN chunks c2 ON c2.id = u.chunk_id"
                   "  WHERE u.person_id = k.person_id))")
    assert outside == [(0,)]


def test_roster_fields_and_coldstart(built):
    people = {r[0]: r[1:] for r in rows(
        built.folio, "SELECT person_id, material, persona_confidence, birthday FROM persons")}
    (alice_m, alice_c, alice_b), (bob_m, bob_c, bob_b) = people["Alice"], people["Bob"]
    # m / (m + median): the median person (of two, the upper) sits at exactly 0.5.
    assert alice_m > bob_m > 0 and alice_c == 0.5 and 0 < bob_c < alice_c
    assert (alice_b, bob_b) == ("03-14", None)
    assert rows(built.folio, "SELECT * FROM topic_terms WHERE person_id = 'Bob' ORDER BY 2") == [
        ("Bob", "Fisherman with two boats"), ("Bob", "Harbour")]
    [(body, version)] = rows(built.folio, "SELECT body, generator_version FROM prompts "
                                          "WHERE subject = 'host' AND slot = 'coldstart'")
    assert json.loads(body) == [
        {"person_id": "Alice", "reason": "Keeper of the northern light"},
        {"person_id": "Bob", "reason": "Fisherman with two boats"}]
    assert version.startswith("forge/")


def test_coldstart_spreads_factions_then_fills(pack):
    src = {p: PersonSource(p, {"home": h, "role": f"{p} role"})
           for p, h in (("a", "x"), ("b", "x"), ("c", "y"), ("d", "x"))}
    conf = {"a": 0.9, "b": 0.8, "c": 0.1, "d": 0.7}
    picked = [p["person_id"] for p in corpus.coldstart(src, conf, pack, 3)]
    assert picked == ["a", "c", "b"]
    assert [p["person_id"] for p in corpus.coldstart(src, conf, pack, 1)] == ["a"]


def test_picks_come_from_params(built, pack, paths, tmp_path):
    (tmp_path / "params.toml").write_text("[coldstart]\npicks = 1\nunknown = 2\n")
    params = Params.load(tmp_path / "params.toml")
    assert params.coldstart.picks == 1 and Params.load(None) == Params()
    with Archive(paths.archive) as archive:
        corpus.run(archive, pack, tmp_path / "one.folio", segmenter="bigram", params=params)
    [(body,)] = rows(tmp_path / "one.folio", "SELECT body FROM prompts WHERE slot='coldstart'")
    assert len(json.loads(body)) == 1


def test_a_malformed_birthday_from_the_pack_is_refused():
    assert corpus._is_birthday("02-29") and not corpus._is_birthday("13-01")
    assert not corpus._is_birthday("3-14") and not corpus._is_birthday("ab-cd")


def test_fingerprint_deterministic_and_rebuild_replaces(built, pack, tmp_path):
    other = tmp_path / "again.folio"
    build(pack, built, folio=other)
    assert manifest(other)["build_fingerprint"] == manifest(built.folio)["build_fingerprint"]

    with sqlite3.connect(built.folio) as con:
        con.execute("INSERT INTO manifest VALUES ('stale', '1')")
    con.close()
    build(pack, built)
    assert "stale" not in manifest(built.folio)


def test_bigram_segmenter_on_cjk_placeholders():
    seg = segment.load("bigram")
    assert seg("甲乙丙 ab1 丁") == "甲乙 乙丙 ab1 丁"


def test_contract_folio(built):
    """Always checks the shape; with DRAMATIS_CONTRACT_OUT set, also exports the folio
    for the engine's contract tests (`engine/crates/{folio,index}/tests/contract.rs`)."""
    m = manifest(built.folio)
    assert {"format_version", "requires", "segmenter", "stopwords", "chunk_count"} <= set(m)
    out = os.environ.get("DRAMATIS_CONTRACT_OUT")
    if out:
        target = Path(out)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(built.folio, target)
        assert manifest(target)["build_fingerprint"] == m["build_fingerprint"]
