"""Mechanisms of the data foundation: forms, exclusion, attribution, rendering."""

from __future__ import annotations

from dataclasses import replace

import pytest

from dramatis_forge import chunk
from dramatis_forge.archive import Archive
from dramatis_forge.guards import LOW
from dramatis_forge.identity import resolve
from dramatis_forge.pack import IdentityEvidence
from dramatis_forge.params import Params
from dramatis_forge.records import Dossier, Line, Voice


def test_pages_the_pack_excludes_leave_the_roster_with_their_forms(pack):
    seen = []

    def excluded(pages, roster, evidence):
        seen.append(evidence)
        return {"Ghost"}

    rules = replace(pack.identity, excluded=excluded)
    pages = {"Alice": "", "Ghost": "", "Ghost (Winter)": "{{AltForm|Ghost}}"}
    ev = IdentityEvidence(spoken={"Alice": 3})
    roster = resolve(rules, pages, frozenset(pages), ev)
    assert set(roster.people) == {"Alice"}
    assert sorted(roster.excluded) == ["Ghost", "Ghost (Winter)"]
    assert set(roster.of_page) == {"Alice"}
    assert [f.severity for f in roster.findings] == [LOW]
    assert seen == [ev]


@pytest.fixture
def forms_pack(pack):
    policy = replace(
        pack.chunking,
        headers={**pack.chunking.headers, "voice": "[voice] {person} · {name}{variant}"},
        templates=(*pack.chunking.templates,
                   replace(pack.chunking.templates[0], name="voice", sources=("voice",))),
        separators={"form": " <{}>"},
        card_omit=("rarity",),
        shared_min=2,
    )
    return replace(pack, chunking=policy, identity=replace(pack.identity, display_facet="name"))


def insert(archive, records):
    for kind in dict.fromkeys(type(r) for r in records):
        archive.insert_records([r for r in records if type(r) is kind])


def make(tmp_path, pack):
    a = Archive(tmp_path / "t.archive")
    a.write_identity({
        "Alice": [("Alice", "canonical", 0), ("Alice (Winter)", "alt", 1)],
        "Bob": [("Bob", "canonical", 0)],
    })
    insert(a, [
        Dossier(page="Alice", fields={"name": "Alice", "rarity": "6", "age": "30"},
                sections=({"title": "Life", "text": "Born in the north."},),
                items={"outfit": "A plain coat.", "memo": "Own words."}, revid=10),
        Dossier(page="Alice (Winter)", fields={"name": "Winter", "rarity": "6", "age": "31"},
                sections=({"title": "Life", "text": "Older now."},),
                items={"outfit": "A plain coat."}, revid=20),
        Dossier(page="Bob", fields={"name": "Bob"}, items={"outfit": "A plain coat."},
                revid=30),
        Voice(page="Alice/v", subject="Alice", idx=0, text="Hello.", title="Greeting"),
        Voice(page="Alice (Winter)/v", subject="Alice (Winter)", idx=0, text="Hi.",
              title="Greeting"),
    ])
    return a


def test_forms_are_told_apart_in_headers_and_cite_their_own_revision(tmp_path, forms_pack):
    with make(tmp_path, forms_pack) as a:
        units = list(chunk.build(a, forms_pack))
    voices = {c.header: c.text for c in units if c.template == "voice"}
    assert voices == {"[voice] Alice · Greeting": "Hello.",
                      "[voice] Alice <Winter> · Greeting": "Hi."}
    life = {c.header: c.revid for c in units if c.header.endswith("Life")}
    assert life == {"[file] Alice · Life": 10, "[file] Alice <Winter> · Life": 20}


def test_a_form_card_holds_only_what_differs_and_omits_mechanics(tmp_path, forms_pack):
    with make(tmp_path, forms_pack) as a:
        units = [c for c in chunk.build(a, forms_pack) if c.span_of and c.span_of.endswith("#card")]
    cards = {c.header: c.text for c in units}
    assert cards["[file] Alice · identity card"] == "name: Alice\nage: 30\nother forms: Winter"
    assert cards["[file] Alice <Winter> · identity card"] == "name: Winter\nage: 31"
    assert all("rarity" not in c.text for c in units)


def test_a_text_every_person_shares_is_stored_once_and_owned_by_nobody(tmp_path, forms_pack):
    with make(tmp_path, forms_pack) as a:
        units = list(chunk.build(a, forms_pack))
    coat = [c for c in units if c.text == "A plain coat."]
    assert len(coat) == 1 and coat[0].persons == ()
    assert any(c.text == "Own words." and c.persons == ("Alice",) for c in units)


def test_markup_left_in_a_unit_is_a_high_finding(tmp_path, pack):
    from dramatis_forge import corpus
    from dramatis_forge.records import Lore
    with Archive(tmp_path / "t.archive") as a:
        insert(a, [Lore(page="P", path=("S",), text="Fine words. --> hidden tail", revid=1)])
        rep = corpus.run(a, pack, tmp_path / "t.folio", segmenter="bigram")
    assert rep.markup_residue == {"-->": 1}
    assert any(f.guard == "G5" and f.severity == "high" and "markup" in f.detail
               for f in rep.ledger.findings)


# ---- dialogue rendering and attribution ----

def test_a_unit_that_is_half_full_ends_before_a_change_of_scene():
    rows = [chunk.Row(i, "A", f"A: {i}", cut=(i == 3)) for i in range(8)]
    groups = chunk._turn_aligned(rows, target=6, cap=8, absorb=False)
    assert [[r.seq for r in g] for g in groups] == [[0, 1, 2], [3, 4, 5, 6, 7]]
    # not when the unit is nearly empty: a stub embeds as noise
    early = [chunk.Row(i, "A", f"A: {i}", cut=(i == 1)) for i in range(8)]
    assert [len(g) for g in chunk._turn_aligned(early, target=6, cap=8, absorb=False)] == [8]
    # nor when it is half full of records but short of characters
    assert [len(g) for g in chunk._turn_aligned(
        rows, target=6, cap=8, absorb=False, cut_chars=40)] == [8]
    assert [len(g) for g in chunk._turn_aligned(
        rows, target=6, cap=8, absorb=False, cut_chars=12)] == [3, 5]


def test_choices_and_lines_are_one_sequence_and_replies_are_introduced(tmp_path, pack):
    from dramatis_forge.records import Choice, Line, Scene
    policy = replace(pack.chunking, separators={"mark": "[{}]", "quote": "<{}>"},
                     labels={"options": "reply", "branch": "if", "merge": "all"})
    big = replace(pack.chunking.templates[1], target=12, max_span=18)
    p = replace(pack, chunking=replace(policy, templates=(
        pack.chunking.templates[0], big, pack.chunking.templates[2])))
    with Archive(tmp_path / "t.archive") as a:
        insert(a, [
            Scene(page="S"),
            Choice(scene="S", seq=1, options=("x",), values=("1",)),
            Choice(scene="S", seq=2, options=("p", "q"), values=("1", "2")),
            Line(scene="S", seq=3, speaker="Ann", text="to p", branch="2:1"),
            Line(scene="S", seq=4, speaker="Ann", text="to q", branch="2:2"),
            Line(scene="S", seq=5, speaker="Ann", text="after"),
        ])
        [unit] = [c for c in chunk.build(a, p) if c.template == "dialogue"]
    assert unit.text.splitlines() == [
        "[reply]x", "[reply]p / q", "[if<p>]", "Ann: to p", "[if<q>]", "Ann: to q",
        "[all]", "Ann: after"]
    assert (unit.span_from, unit.span_to) == (1, 5)


def test_a_unit_that_starts_inside_a_reply_still_says_which(tmp_path, pack):
    from dramatis_forge.records import Choice, Line, Scene
    policy = replace(pack.chunking, separators={"mark": "[{}]", "quote": "<{}>"},
                     labels={"branch": "if"})
    template = replace(pack.chunking.templates[1], target=2, max_span=2)
    p = replace(pack, chunking=replace(policy, templates=(
        pack.chunking.templates[0], template, pack.chunking.templates[2])))
    with Archive(tmp_path / "t.archive") as a:
        insert(a, [Scene(page="S"), Choice(scene="S", seq=1, options=("p", "q"), values=("1", "2"))]
               + [Line(scene="S", seq=i, speaker=f"S{i}", text=f"l{i}", branch="1:1")
                  for i in range(2, 6)])
        units = [c for c in chunk.build(a, p) if c.template == "dialogue"]
    assert len(units) >= 2
    assert all("[if<p>]" in u.text for u in units)


def test_speakers_resolve_by_page_then_portrait_then_alias(tmp_path, pack):
    from dramatis_forge.records import Alias
    from dramatis_forge.speakers import Speakers
    ident = replace(pack.identity, portrait_key=lambda w: w.strip() or None)
    p = replace(pack, identity=ident, chunking=replace(pack.chunking, generic_speakers=("???",)))
    with Archive(tmp_path / "t.archive") as a:
        a.write_identity({"Ann": [("Ann", "canonical", 0)], "Bob": [("Bob", "canonical", 0)],
                          "Cy": [("Cy", "canonical", 0)]})
        a.put_page("Ann", "k-ann", 1, "now")
        a.put_page("Bob", "k-bob", 2, "now")
        a.put_page("Cy", "k-cy", 3, "now")
        insert(a, [Alias(alias="Cyrus", target="Cy", kind="redirect"),
                   Alias(alias="Cyrus II", target="Cy", kind="disambig"),
                   Line(scene="S", seq=1, speaker="Annie", text="a", portrait="k-ann"),
                   Line(scene="S", seq=2, speaker="Annie", text="b", portrait="k-ann")])
        sp = Speakers(a, p, replace(Params().speaker, portrait_lines=2))
        rows = [{"speaker": "Annie", "portrait": "k-ann"}, {"speaker": "Annie", "portrait": "k-ann"},
                {"speaker": "Cyrus", "portrait": None}, {"speaker": "Cyrus II", "portrait": None},
                {"speaker": "???", "portrait": "k-bob"}, {"speaker": "Ann", "portrait": "k-bob"}]
        got = sp.scene_owners("S", rows)
    assert got == {"Annie": "Ann", "Cyrus": "Cy", "Cyrus II": None, "???": None, "Ann": "Ann"}
    # a label settled by portraits across the corpus is a name for that person
    assert sp.speaker_aliases() == [("Annie", "Ann")]


def test_slot_names_are_reworded_and_interface_words_are_caught(pack):
    policy = replace(
        pack.chunking, voice_words=((r"^Touch$", "when touched"), (r"^Squad(\d*)$", r"in a team \1")),
        header_taboo=("Touch|Squad",), labels={"voice": "voice"}, triggers={"T": "Squad"},
        section_words=((r"^Elite(\d)$", r"outfit \1"),))
    assert policy.voice_name("", "Touch") == "when touched"
    assert policy.voice_name("", "Squad2") == "in a team 2"
    assert policy.voice_name("T", "") == "in a team"          # the trigger's words are reworded
    assert policy.voice_name("", "Other") == "Other"
    assert policy.section_name("Elite0") == "outfit 0"
    test = chunk.engine_meta_test(policy, [])
    assert test("voice", "p", "[voice] A · Touch") and not test("voice", "p", "[voice] A · when touched")


def test_a_skipped_slot_makes_no_unit_and_is_counted(tmp_path, pack):
    from dramatis_forge import corpus
    policy = replace(
        pack.chunking, voice_skip=("TITLE",),
        headers={**pack.chunking.headers, "voice": "[voice] {person} · {name}{variant}"},
        templates=(*pack.chunking.templates,
                   replace(pack.chunking.templates[0], name="voice", sources=("voice",))))
    p = replace(pack, chunking=policy)
    with Archive(tmp_path / "t.archive") as a:
        a.write_identity({"Ann": [("Ann", "canonical", 0)]})
        insert(a, [Voice(page="Ann/v", subject="Ann", idx=0, text="Name.", trigger="TITLE"),
                   Voice(page="Ann/v", subject="Ann", idx=1, text="Hi.", title="Hello", seq=1)])
        env = chunk.Env.for_build(a, p)
        got = [c for c in chunk.build(a, p, env=env) if c.template == "voice"]
        rep = corpus.run(a, p, tmp_path / "t.folio", segmenter="bigram")
    assert [c.text for c in got] == ["Hi."] and env.skipped == {"voice": 1}
    assert any("made no unit" in f.detail for f in rep.ledger.findings)


def test_a_set_with_layouts_is_sampled_by_layout(tmp_path, pack):
    from dramatis_forge.report import samples
    seeds = tuple(replace(s, layout=lambda t, w: "long" if len(w) > 30 else "short")
                  if s.key == "people" else s for s in pack.seeds)
    p = replace(pack, seeds=seeds)
    with Archive(tmp_path / "t.archive") as a:
        a.write_seeds({"people": ["P1", "P2", "P3", "P4"]})
        for title, text in (("P1", "x" * 10), ("P2", "y" * 50), ("P3", "z" * 40), ("P4", "w" * 20)):
            a.put_page(title, text, 1, "now")
        a.put_page("P5", "v" * 5, 1, "now")
        a.write_seeds({"people": ["P1", "P2", "P3", "P4", "P5"]})
        # P5 produced nothing: it is not drawn into the layers of pages that did
        from dramatis_forge.records import Lore
        insert(a, [Lore(page=t, path=("x",), text="body", revid=1) for t in ("P1", "P2", "P3", "P4")])
        a.commit()
        sel = samples.select(a, p)
    picked = {route: set(titles) for route, titles in sel.routes.items() if "people" in route}
    layer = {r: t for r, t in picked.items() if r.endswith(("long", "short"))}
    # each class is read through its first and its largest page, and nothing else
    assert {r.rsplit(" · ", 1)[1]: ts for r, ts in layer.items()} == {
        "long": {"P2"}, "short": {"P1", "P4"}}
    assert {r.rsplit(" · ", 1)[1]: ts for r, ts in picked.items()
            if r.endswith("excluded")} == {"excluded": {"P5"}}


def test_every_rendering_of_a_term_is_an_alias_for_it():
    from dramatis_forge.normalize import TERM_ALIAS, term_aliases
    from dramatis_forge.records import Term
    got = term_aliases([
        Term(page="g", term="Home", translations={"en": "Homeland/Home Town", "jp": "ホーム\nHome"}),
        Term(page="g", term="Ann", translations={"en": "Annie"}),
    ], {"Ann": "Ann (person)"})
    assert sorted((a.alias, a.target, a.kind) for a in got) == [
        ("Annie", "Ann (person)", TERM_ALIAS), ("Home Town", "Home", TERM_ALIAS),
        ("Homeland", "Home", TERM_ALIAS), ("ホーム", "Home", TERM_ALIAS)]


def test_an_archived_kind_nothing_reads_is_a_high_finding(tmp_path, pack):
    from dramatis_forge import corpus
    from dramatis_forge.records import Letter
    with Archive(tmp_path / "t.archive") as a:
        insert(a, [Letter(page="m", body="Dear reader.", sender="Ann")])
        rep = corpus.CorpusReport()
        corpus._check_consumers(rep, a, pack)
    assert [f.severity for f in rep.ledger.findings] == ["high"]
    assert "letter" in rep.ledger.findings[0].detail


def test_a_form_kind_without_a_card_adds_none(tmp_path, forms_pack):
    p = replace(forms_pack, chunking=replace(forms_pack.chunking, cardless_forms=("alt",)))
    with make(tmp_path, p) as a:
        units = [c for c in chunk.build(a, p) if c.span_of and c.span_of.endswith("#card")]
    assert [c.header for c in units] == ["[file] Alice · identity card", "[file] Bob · identity card"]


def test_a_declared_name_outranks_a_portrait_and_a_skipped_section_makes_no_unit(tmp_path, pack):
    from dramatis_forge.records import Alias
    from dramatis_forge.speakers import Speakers
    ident = replace(pack.identity, portrait_key=lambda w: w.strip() or None)
    p = replace(pack, identity=ident)
    with Archive(tmp_path / "t.archive") as a:
        a.write_identity({"Ann": [("Ann", "canonical", 0)], "Bob": [("Bob", "canonical", 0)]})
        a.put_page("Ann", "k-ann", 1, "now")
        a.put_page("Bob", "k-bob", 2, "now")
        insert(a, [Alias(alias="Annie", target="Ann", kind="redirect")])
        sp = Speakers(a, p, replace(Params().speaker, portrait_lines=1))
        # the label is declared to be Ann's, whatever the portrait on screen
        assert sp.owner("Annie", "s") == "Ann"
        # an unresolved label stays open, whatever the portraits in its scene show
        assert sp.owner("Stranger", "s") is None

    policy = replace(pack.chunking, section_skip=("^Ratings$",))
    q = replace(pack, chunking=policy)
    with Archive(tmp_path / "u.archive") as a:
        a.write_identity({"Ann": [("Ann", "canonical", 0)]})
        insert(a, [Dossier(page="Ann", fields={"name": "Ann"}, revid=1, sections=(
            {"title": "Ratings", "text": "Strength: high"}, {"title": "Life", "text": "Born north."}))])
        env = chunk.Env.for_build(a, q)
        texts = [c.text for c in chunk.build(a, q, env=env) if c.template == "profile"]
    assert "Born north." in texts and "Strength: high" not in texts
    assert env.skipped == {"profile": 1}


def test_placeholders_and_unresolved_labels_are_nobodys_and_exceptions_are_declared(tmp_path, pack):
    from dramatis_forge.speakers import Speakers
    ident = replace(pack.identity, portrait_key=lambda w: w.strip() or None)
    policy = replace(pack.chunking, generic_speakers=("???",),
                     speaker_as=(("s1", "Bob", "Ann"),))
    p = replace(pack, identity=ident, chunking=policy)
    rows = [{"speaker": s, "portrait": "k-ann"} for s in ("???", "Bob", "Bob", "Bob", "Crowd")]
    with Archive(tmp_path / "t.archive") as a:
        a.write_identity({"Ann": [("Ann", "canonical", 0)], "Bob": [("Bob", "canonical", 0)]})
        a.put_page("Ann", "k-ann", 1, "now")
        a.commit()
        sp = Speakers(a, p, Params().speaker)
        got = sp.scene_owners("s1", rows)
        other = sp.scene_owners("s2", rows)
    # a placeholder and an unknown label are nobody's even when one portrait fills the scene
    assert got == {"???": None, "Bob": "Ann", "Crowd": None}
    # the exception is for one scene only
    assert other["Bob"] == "Bob"
    # a declared exception is already the explanation; the same label elsewhere is a conflict
    assert list(sp.conflicts) == [("s2", "Bob")]


def test_a_portrait_id_spelled_differently_resolves_a_non_page_label(tmp_path, pack):
    from dramatis_forge.speakers import Speakers
    ident = replace(pack.identity, portrait_key=lambda w: w.strip() or None,
                    portrait_loose=lambda k: k.split("_")[0])
    p = replace(pack, identity=ident)
    with Archive(tmp_path / "t.archive") as a:
        a.write_identity({"Ann": [("Ann", "canonical", 0)]})
        a.put_page("Ann", "7_ann", 1, "now")
        a.commit()
        sp = Speakers(a, p, replace(Params().speaker, portrait_lines=1))
    assert sp.loose_person == {"7": "Ann"}
    assert sp._portrait_person("7_other") == "Ann"
    assert sp._portrait_person("8_other") is None


def test_a_reply_id_no_choice_offers_is_counted_as_unresolved(pack):
    unresolved: list[str] = []
    choices = {1: (["a", "b"], ["x", "y"], None)}
    ok = chunk._reply_marker(pack.chunking, choices, "1:x", unresolved, "S")
    bad = chunk._reply_marker(pack.chunking, choices, "1:2", unresolved, "S")
    assert unresolved == ["S"] and "a" in ok and "2" in bad


def test_term_aliases_land_on_the_canonical_name_and_conflicts_are_reported():
    from dramatis_forge.guards import Ledger
    from dramatis_forge.normalize import check_aliases, term_aliases
    from dramatis_forge.records import Alias, Term
    redirects = {"Edge": "Sharp", "Isles": "Isles (region)"}
    terms = [Term(page="g", term="Edge", translations={"en": "Sharp/Blade"}),
             Term(page="g", term="Isles", translations={"en": "Isle", "note": "A place. Long."}),
             Term(page="g", term="Mis", translations={"en": "Mish"})]
    got = term_aliases(terms, {}, redirects)
    # the rendering that equals the canonical name says nothing; the target is never an alias
    assert sorted((a.alias, a.target) for a in got) == [
        ("A place. Long.", "Isles (region)"), ("Blade", "Sharp"),
        ("Isle", "Isles (region)"), ("Mish", "Mis")]
    assert all(a.target not in redirects for a in got)
    assert ("Sharp", "Sharp") not in {(a.alias, a.target) for a in got}
    rep = type("R", (), {"ledger": Ledger()})()
    check_aliases([*got, Alias(alias="Mish", target="Other", kind="realname"),
                   Alias(alias="x", target="Edge", kind="redirect")], redirects, set(), rep)
    details = [f.detail for f in rep.ledger.findings]
    assert any("another alias" in d for d in details)
    assert any("'Mish'" in d for d in details)


def test_nested_form_names_short_items_and_placeholders(tmp_path, forms_pack):
    p = replace(forms_pack, chunking=replace(
        forms_pack.chunking, placeholder_values=("none",), item_min_chars=20, foldable_items=("^tiny$",),
        cardless_forms=("mode",)))
    with Archive(tmp_path / "t.archive") as a:
        a.write_identity({"Alice": [("Alice", "canonical", 0), ("Alice(Winter)", "alt", 1),
                                    ("Alice(Mode)", "mode", 2)]})
        insert(a, [
            Dossier(page="Alice", fields={"name": "Alice", "team": "none", "job": "x"},
                    items={"first": "A long enough entry that stands alone.", "tiny": "Short.",
                           "blurb": "Brief."},
                    revid=1),
            Dossier(page="Alice(Winter)", fields={"name": "Alice(Winter)", "team": "none"}, revid=2),
            Dossier(page="Alice(Mode)", fields={"name": "Alice(Mode)"}, revid=3),
        ])
        units = list(chunk.build(a, p))
    cards = {c.header: c.text for c in units if c.span_of and c.span_of.endswith("#card")}
    # the person is not repeated inside the form's name, "none" is not a field, a mode copy is
    # not listed among the other forms
    assert cards["[file] Alice · identity card"] == "name: Alice\njob: x\nother forms: Alice(Winter)"
    assert cards["[file] Alice <Winter> · identity card"] == "name: Alice(Winter)"
    items = [c.text for c in units if c.span_of == "Alice#dossier"]
    # only the kind of item the pack names is folded; "blurb" is short and complete alone
    assert items == ["A long enough entry that stands alone.\ntiny: Short.", "Brief."]


def test_lore_runs_follow_the_pages_declared_shape(pack):
    policy = replace(pack.chunking, lore_spans=((r"^List$", 0), (r"^Story$", 1)))
    assert policy.lore_run("Plain", ("a",), 3) == "Plain"
    assert policy.lore_run("List", ("a",), 3) == "List#3"
    assert policy.lore_run("Story", ("tale", "part 2"), 5) == "Story#tale"
