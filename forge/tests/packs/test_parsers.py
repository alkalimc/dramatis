"""Page readers other than the AVG scripts.

Each test states a rule the data forced on us. Where a test names a count, that count
came from a full run — they are regression anchors, not illustrations.
"""

from __future__ import annotations

import pytest

from packs.arknights import parsers


# --------------------------------------------------------------------------- #
# operator dossiers
# --------------------------------------------------------------------------- #

DOSSIER = """
{{CharinfoV2
<!--auto-->
|干员名=测试
|精英0介绍=她在出发前定做的服装。
|获得方式=不该收
}}
{{人员档案
|档案1=基础档案
|档案1条件=初始开放
|档案1文本=【代号】测试<br/>【性别】女
|档案2=综合体检测试
|档案2文本=体细胞与源石融合率 7%。
}}
{{相关道具
|干员简介=罗德岛术师干员。
|信物描述=一只纸折的兽亲。
|信物用途=用于提升潜能。
}}
"""


def test_dossier_takes_narrative_and_drops_gameplay(ctx, of_kind):
    got = of_kind(list(parsers.parse_dossier(ctx(DOSSIER, title="测试", seed="S2"))), "dossier")
    dossier = got[0]
    assert [s["title"] for s in dossier.sections] == ["基础档案", "综合体检测试"]
    assert "【代号】测试" in dossier.sections[0]["text"]
    # unlock conditions, acquisition and item mechanics are gameplay
    assert "初始开放" not in str(dossier.sections)
    assert "信物用途" not in dossier.items
    assert "不该收" not in str(dossier.items)
    # outfit blurbs are short characterful prose and are kept
    assert "精英0介绍" in dossier.items


def test_dossier_reads_structured_fields_not_the_infobox(ctx, of_kind):
    """Attributes come from the site's own tables; re-deriving them from markup only
    adds a way to be wrong."""
    page = ctx(DOSSIER, title="测试", seed="S2",
               tables={"chara": {"测试": {"cn": "测试", "rarity": "5"}}})
    got = of_kind(list(parsers.parse_dossier(page)), "dossier")
    assert got[0].fields == {"代号": "测试", "稀有度": "5"}


def test_dossier_survives_comment_inside_template_name(ctx, of_kind):
    """455 of 456 operator pages write the infobox name with an embedded comment."""
    page = ctx("{{CharinfoV2\n<!--x-->\n|精英0介绍=文字。}}", title="测试", seed="S2")
    got = of_kind(list(parsers.parse_dossier(page)), "dossier")
    assert got and "精英0介绍" in got[0].items


def test_operator_with_no_prose_is_a_fact_about_the_source(ctx):
    """Real for ~24 reserve and support units: on the roster, nothing to say."""
    page = ctx("{{CharinfoV2|干员名=预备干员}}", title="预备干员-近战", seed="S2")
    list(parsers.parse_dossier(page))
    assert page.expected_empty or page.drain_warnings()


def test_record_set_intros_are_kept_as_tone_material(ctx, of_kind):
    page = ctx(DOSSIER, title="测试", seed="S2", tables={
        "char_memory": {"测试": [
            {"storySetName": "密录一", "storyIntro": "引言甲", "storyIndex": "1"},
            {"storySetName": "密录二", "storyIntro": "引言乙", "storyIndex": "2"},
        ]}})
    got = of_kind(list(parsers.parse_dossier(page)), "dossier")
    assert [m["set"] for m in got[0].items["密录"]] == ["密录一", "密录二"]


# --------------------------------------------------------------------------- #
# voice records
# --------------------------------------------------------------------------- #

VOICES = """
{{VoiceTable|表格标题=语音记录
|标题1=任命助理
|台词1={{VoiceData/word|中文|博士，今天没有急件。}}{{VoiceData/word|日文|ドクター}}
|触发类型1=HOME_PLACE
|标题2=交谈1
|台词2={{VoiceData/word|中文|你想听我说什么？}}{{VoiceData/word|英文|What?}}
|触发类型2=TALK
|条件2=信赖提升后
}}
"""


def test_voices_take_chinese_only(ctx, of_kind):
    page = ctx(VOICES, title="测试/语音记录", seed="S3")
    got = of_kind(list(parsers.parse_voices(page)), "voice")
    assert [v.text for v in got] == ["博士，今天没有急件。", "你想听我说什么？"]
    assert not any("ドクター" in v.text or "What" in v.text for v in got)


def test_voices_keep_their_trigger(ctx, of_kind):
    """The trigger is what makes these tone material rather than loose quotations."""
    page = ctx(VOICES, title="测试/语音记录", seed="S3")
    got = of_kind(list(parsers.parse_voices(page)), "voice")
    assert [v.trigger for v in got] == ["HOME_PLACE", "TALK"]
    assert got[1].unlock == "信赖提升后"


def test_voices_attribute_to_the_subject_not_the_page(ctx, of_kind):
    page = ctx(VOICES, title="凯尔希/语音记录", seed="S3")
    got = of_kind(list(parsers.parse_voices(page)), "voice")
    assert all(v.subject == "凯尔希" for v in got)


def test_voice_page_without_a_local_table_is_a_redirect(ctx):
    """The suffix plus a local table defines the set; a suffix alone is a stub."""
    page = ctx("#REDIRECT [[别处]]", title="测试/语音记录", seed="S3")
    assert list(parsers.parse_voices(page)) == []
    assert "redirect" in page.expected_empty


# --------------------------------------------------------------------------- #
# world codex
# --------------------------------------------------------------------------- #

def test_codex_keeps_the_world_pane_and_drops_commentary(ctx, of_kind):
    body = (
        '<div id="情报" class="section_tabcontent">'
        "==历史==\n乌萨斯是泰拉北方的帝国，以严酷的统治著称，其疆域覆盖广袤的雪原地带。"
        "</div>"
        '<div id="考据" class="section_tabcontent">'
        "==原型==\n名称来源于现实中的某个国家，设计参考了历史上的军装。"
        "</div>"
    )
    got = of_kind(list(parsers.parse_lore(ctx(body, title="泰拉大典:乌萨斯", seed="S4"))), "lore")
    joined = " ".join(r.text for r in got)
    assert "泰拉北方的帝国" in joined
    assert "现实中的某个国家" not in joined


def test_codex_reads_world_parameters_from_commentary_templates(ctx, of_kind):
    """Dropping a page because it contains a commentary box discarded the three
    highest-value pages in the namespace: races, the character register, concepts."""
    body = (
        "{{种族考据|种族=沃尔珀\n"
        "|情报=沃尔珀是泰拉大陆上分布广泛的种族，多见于叙拉古与哥伦比亚一带。\n"
        "|考据=名称取自现实中的一种动物。\n}}"
    )
    got = of_kind(list(parsers.parse_lore(ctx(body, title="泰拉大典:生物", seed="S4"))), "lore")
    assert got and "分布广泛的种族" in got[0].text
    assert "现实中的一种动物" not in got[0].text


def test_codex_meta_pages_are_excluded(ctx):
    page = ctx("==格式==\n条目应当这样写，字段顺序如下所述，请遵守。", title="泰拉大典:条目格式", seed="S4")
    assert list(parsers.parse_lore(page)) == []
    assert "meta" in page.expected_empty


def test_codex_does_not_repeat_the_title_in_the_body(ctx, of_kind):
    body = "{{角色考据|姓名=阿戈尔|情报=阿戈尔涉足的地区遍布深海，其文明形态与陆上迥异。}}"
    got = of_kind(list(parsers.parse_lore(ctx(body, title="泰拉大典:角色", seed="S4"))), "lore")
    assert not got[0].text.startswith("阿戈尔：阿戈尔")


def test_codex_heading_path_is_kept_as_context(ctx, of_kind):
    """An entry filed under one heading means something different under another."""
    body = (
        '<div id="情报" class="section_tabcontent">'
        "==人类种族==\n===阿达克利斯===\n"
        "阿达克利斯是人类的一支，体格强健，主要聚居在泰拉大陆西侧的山地，以狩猎与采矿为生。"
        "</div>"
    )
    got = of_kind(list(parsers.parse_lore(ctx(body, title="泰拉大典:生物", seed="S4"))), "lore")
    assert any("人类种族" in r.path for r in got)


# --------------------------------------------------------------------------- #
# index pages
# --------------------------------------------------------------------------- #

def test_letters_are_first_person_prose(ctx, of_kind):
    body = (
        "{{邮件|来自=阿米娅|日期=1096年|标题=致博士\n"
        "|内容=博士，欢迎回到罗德岛。这是我特意买来的饼干，希望你喜欢。}}"
    )
    got = of_kind(list(parsers.parse_letters(ctx(body, title="邮件记录", seed="S5"))), "letter")
    assert got[0].sender == "阿米娅" and "欢迎回到罗德岛" in got[0].body


def test_tips_keep_worldbuilding_and_drop_the_manual(ctx, of_kind):
    body = """
==通常==
{| class="wikitable"
! rowspan="2" | 战斗
| 部署干员时请注意费用。
|-
| 阻挡数决定了能拦住几个敌人。
|-
! rowspan="1" | 背景
| 源石是泰拉世界普遍存在的矿物，是天灾的首要成因，也被广泛用作能源。
|}
==世界观==
{| class="wikitable"
|-
|源石||泰拉世界普遍存在的一种矿物，大部分呈黑色半透光晶体，蕴藏巨大能量。
|}
"""
    got = of_kind(list(parsers.parse_tips(ctx(body, title="贴士一览", seed="S5"))), "lore")
    joined = " ".join(r.text for r in got)
    assert "天灾的首要成因" in joined
    assert "黑色半透光晶体" in joined
    assert "部署干员" not in joined and "阻挡数" not in joined


def test_glossary_drops_rows_with_unparsed_markup(ctx, of_kind):
    """A glossary entry that is partly markup is worse than a missing one: it will be
    matched against a query and returned as if it meant something."""
    body = """
==角色==
{| class="wikitable"
|-
|?干员外文名||?干员名jp||limit=500
|-
|源石技艺||Originium Arts||Arts
|}
"""
    got = of_kind(list(parsers.parse_glossary(ctx(body, title="泰拉词库", seed="S5"))), "term")
    assert [t.zh for t in got] == ["源石技艺"]


def test_glossary_splits_stacked_names(ctx, of_kind):
    """The wiki stacks a code name over its translation with `<br>` in one cell, to say
    both name the same thing. Measured: seven such entries. Keeping the break produces
    one unsearchable two-line token."""
    body = '==角色==\n{| class="wikitable"\n|-\n|Ace<br>王牌||-||Ace\n|}' 
    got = of_kind(list(parsers.parse_glossary(ctx(body, title="泰拉词库", seed="S5"))), "term")
    assert {t.zh for t in got} == {"Ace", "王牌"}


def test_real_names_locate_columns_by_header(ctx, of_kind):
    """Column position cannot be assumed: taking column 0 yields 4 rows out of 200."""
    body = """
{| class="wikitable"
! colspan="4" | 干员真名一览
|-
! 干员头像 !! 干员代号 !! 真名 !! 出处
|-
| [[File:x.png]] || 可露希尔 || 阿达·丘奇 || 档案资料二
|}
"""
    got = of_kind(list(parsers.parse_real_names(ctx(body, title="角色真名", seed="S5"))), "alias")
    assert ("阿达·丘奇", "可露希尔") in [(a.alias, a.target) for a in got]


def test_real_names_treat_the_arrow_icon_as_a_separator(ctx, of_kind):
    r"""`{{mdi|arrow-right}}` renders as an arrow and *means* "renamed to". Left as text
    it produces one name reading `佐原田金兵卫arrow-right三船光平`."""
    body = """
{| class="wikitable"
! 干员代号 !! 真名
|-
| 某干员 || 佐原田金兵卫{{mdi|arrow-right}}三船光平
|}
"""
    got = of_kind(list(parsers.parse_real_names(ctx(body, title="角色真名", seed="S5"))), "alias")
    names = {a.alias for a in got}
    assert names == {"佐原田金兵卫", "三船光平"}


def test_char_refs_keep_short_but_complete_descriptions(ctx, of_kind):
    """The floor exists to skip blanks and headers, not short entries. Measured: 16 rows
    are complete in about eleven characters."""
    body = """
==== 黑暗时代 ====
{| class="wikitable"
|-
| 名称/代号 || 简介 || 出处
|-
| 海伦 || 爱国者的妻子，已故。 || 主线
|}
"""
    got = of_kind(list(parsers.parse_char_refs(ctx(body, title="剧情角色一览", seed="S5"))), "char_ref")
    assert [(r.name, r.story_group) for r in got] == [("海伦", "黑暗时代")]


def test_synopses_are_read_per_entry_not_per_section(ctx, of_kind):
    """The page is one table, so section splitting yields three sections and loses every
    synopsis."""
    body = (
        "<big><big>某活动</big></big>\n"
        "{{剧情简介|GT-1|日正当中|行动前|因为宝藏的传说，一行人来到了荒漠边缘的小镇。}}\n"
        "{{剧情简介|GT-2|风沙渐起|行动后|沙暴掩盖了踪迹，线索到此中断。}}\n"
    )
    got = of_kind(list(parsers.parse_synopses(ctx(body, title="情报处理室", seed="S5"))), "lore")
    assert len(got) == 2
    # the qualifier goes into the text: it is what makes the unit findable
    assert "GT-1 日正当中 行动前" in got[0].text
    assert "某活动" in got[0].path
