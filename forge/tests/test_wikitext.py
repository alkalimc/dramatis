"""Wikitext flattening under a pack's InlineRules: every construct has one defined outcome,
and anything outside the rules is reported rather than guessed at silently."""

from __future__ import annotations

import pytest

from dramatis_forge.pack import InlineRules
from dramatis_forge.wikitext import Cleaner, find_header, strip_comments, table_rows


@pytest.fixture
def clean(pack):
    return Cleaner(pack.inline)


def flat(clean: Cleaner, s: str) -> tuple[str, list[str]]:
    warnings: list[str] = []
    return clean.text(s, warnings), warnings


@pytest.mark.parametrize(("source", "expected"), [
    ("a {{cite|a source}}b", "a b"),                        # drop
    ("{{color|red|Hello}} there", "Hello there"),           # text_param, last positional
    ("{{Quote|Captain|Hold fast}}", "Captain: Hold fast"),  # content template with qualifier
    ("Hi {{reader}}", "Hi the reader"),                     # literal
    ("x{{#if:1|y}}z", "xz"),                                # parser function removed
    ("{{Cite\n<!-- generated -->\n|n}}kept", "kept"),       # name normalised past a comment
])
def test_templates_follow_the_rules(clean, source, expected):
    assert flat(clean, source) == (expected, [])


def test_unknown_template_keeps_last_param_and_warns(clean):
    assert flat(clean, "{{Mystery|a|b}}") == ("b", ["unknown inline template: Mystery"])


def test_comments_including_an_unterminated_one():
    assert strip_comments("a<!-- note -->b<!-- never closed\nmore") == "ab"


def test_nested_tables_are_removed_whole(clean):
    assert flat(clean, "before {|\n|a\n{|\n|inner\n|}\n|} after")[0] == "before after"


def test_orphan_closer_is_swept(clean):
    assert flat(clean, "tail of a cut template}} then prose")[0] == "tail of a cut template then prose"


def test_html_entities_decoded_after_tag_pass(clean):
    # `&lt;i&gt;` must come out as literal text, not be mistaken for a tag and stripped.
    assert flat(clean, "A &amp; B&nbsp;C &lt;i&gt;")[0] == "A & B C <i>"


def test_links_namespaces_and_external(clean):
    text = "[[Category:X]][[File:y.png|thumb]][[Target|label]] [[Plain]] [https://e.invalid ext] [https://e.invalid]"
    assert flat(clean, text)[0] == "label Plain ext"


def test_pack_can_add_localised_namespace():
    cleaner = Cleaner(InlineRules(link_namespaces=("Kategorie",)))
    assert cleaner.text("[[Kategorie:Z]]kept [[Category:Y|shown]]") == "kept shown"


def test_macros_substituted_and_unknown_shape_warned(clean):
    assert flat(clean, "Hi ${player} and ${ghost}") == (
        "Hi the reader and ${ghost}", ["unknown engine macro: ${ghost}"])


def test_bold_and_generic_tags_stripped(clean):
    assert flat(clean, "'''bold''' <span class='x'>s</span><br/>next")[0] == "bold s\nnext"


def test_split_sections_keeps_heading_path_and_lead(clean):
    body = ("Lead paragraph long enough to be kept as a section.\n"
            "== Top ==\nTop level text that is long enough to keep.\n"
            "=== Child ===\nChild text under the top heading, also long.\n")
    sections = clean.split_sections(body)
    assert [s["path"] for s in sections] == [("lead",), ("Top",), ("Top", "Child")]


def test_drop_sections_removes_subtree():
    cleaner = Cleaner(InlineRules(drop_sections=frozenset({"Trivia"})))
    body = ("== Story ==\nThe story section text, long enough to keep.\n"
            "== Trivia ==\nReal-world notes that must not be kept at all.\n"
            "=== Sub ===\nA child of trivia, also dropped entirely.\n")
    assert [s["path"] for s in cleaner.split_sections(body)] == [("Story",)]


def test_table_rows_and_header_after_title_row():
    table = ("{|\n|-\n! colspan=2 | Title row\n|-\n! Name !! Role\n|-\n| Ann || Pilot\n"
             "|-\n| style=\"x\" | Ben || Cook\n|}")
    header, index = find_header(table, "Name", "Role")
    assert header == ["Name", "Role"] and index == 2
    assert table_rows(table)[-2:] == [["Ann", "Pilot"], ["Ben", "Cook"]]
