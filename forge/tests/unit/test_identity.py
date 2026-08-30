"""Identity: a page is not a person.

The measured baseline is 456 roster pages resolving to 416 people. These tests fix the
*rules* that produce that number, and the guard behaviour that catches it changing.
"""

from __future__ import annotations

from dramatis_forge import identity as ident
from dramatis_forge.identity import CANONICAL, resolve
from packs.arknights.identity import ALTER, VARIANT


def test_alter_declaration_folds_into_the_prototype(pack):
    pages = {
        "安洁莉娜": "{{异格干员|原型={{BASEPAGENAME}}|非异格=1}}",
        "予愿安洁莉娜": "{{异格干员|原型=安洁莉娜}}",
    }
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert len(roster) == 1
    person = roster.people["安洁莉娜"]
    assert [(f.page, f.kind) for f in person.forms] == [
        ("安洁莉娜", CANONICAL), ("予愿安洁莉娜", ALTER)]


def test_self_reference_does_not_make_a_page_its_own_alter(pack):
    """The prototype carries `原型={{BASEPAGENAME}}`; treating that as a title would
    make every prototype an alter of itself."""
    pages = {"安洁莉娜": "{{异格干员|原型={{BASEPAGENAME}}|非异格=1}}"}
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert roster.people["安洁莉娜"].forms[0].kind == CANONICAL


def test_variant_needs_both_the_template_and_a_resolvable_title(pack):
    """Title shape alone is fragile — parentheses are a general disambiguation device.
    Requiring the marker as well makes the intersection exact."""
    pages = {
        "阿米娅": "{{异格干员/升变}}",
        "阿米娅(医疗)": "{{异格干员/升变}}",
        "某人(消歧义)": "无模板，只有括号",
    }
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert [(f.page, f.kind) for f in roster.people["阿米娅"].forms] == [
        ("阿米娅", CANONICAL), ("阿米娅(医疗)", VARIANT)]
    # the parenthetical without the marker stands alone
    assert "某人(消歧义)" in roster.people


def test_name_similarity_is_never_used(pack):
    """Real counterexamples: 推进之王/维娜·维多利亚 share nothing; 凯尔希/凯尔希·思衡托
    share a prefix by coincidence. Prefix or edit-distance pairing both misses and
    misfires, and a misfire is undiscoverable."""
    pages = {"凯尔希": "无声明", "凯尔希·思衡托": "无声明"}
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert len(roster) == 2


def test_declaration_outside_the_roster_is_high_severity(pack):
    pages = {"某异格": "{{异格干员|原型=不在名册里的人}}"}
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert any(f.severity == "高" for f in roster.findings)


def test_identity_chains_are_rejected(pack):
    """Identity would otherwise depend on traversal order."""
    pages = {
        "甲": "无声明",
        "乙": "{{异格干员|原型=甲}}",
        "丙": "{{异格干员|原型=乙}}",
    }
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert any("chain" in f.detail for f in roster.findings if f.severity == "高")


def test_person_count_shrinking_is_high_severity(pack):
    """Growth means new characters shipped; shrinkage means an enumerator broke."""
    pages = {"甲": "无声明"}
    roster = resolve(pack.identity, pages, frozenset(pages))
    assert any(f.severity == "高" and "fell" in f.detail for f in roster.findings)


def test_alter_names_become_aliases_for_the_person(pack):
    """A query naming an alternate form must reach the one person, or the alias
    dictionary silently disagrees with the roster."""
    pages = {"安洁莉娜": "无声明", "予愿安洁莉娜": "{{异格干员|原型=安洁莉娜}}"}
    roster = resolve(pack.identity, pages, frozenset(pages))
    aliases = [(a.alias, a.target, a.kind) for a in roster.aliases()]
    assert aliases == [("予愿安洁莉娜", "安洁莉娜", "alter")]


def test_forms_sort_canonical_first(pack):
    """The roster entry expands in the order a reader expects, and the persona generator
    sees "who she is" before "who she became"."""
    pages = {
        "陈": "无声明",
        "假日威龙陈": "{{异格干员|原型=陈}}",
        "赤刃明霄陈": "{{异格干员|原型=陈}}",
    }
    roster = resolve(pack.identity, pages, frozenset(pages))
    forms = roster.people["陈"].forms
    assert forms[0].kind == CANONICAL
    assert all(f.kind == ALTER for f in forms[1:])
