"""AVG script parsing.

Every test here corresponds to something a full run reported. The whitelist's variants
were not designed — they were found by guards and then encoded, so these tests are the
record of what the data actually contains.
"""

from __future__ import annotations

import pytest

from packs.arknights import avg


def script(body: str) -> str:
    return "{{剧情模拟器|文本数据=\n" + body + "\n}}"


@pytest.fixture
def lines(ctx, of_kind):
    """Parse a script body and return just its line records."""
    def parse(body: str):
        return of_kind(list(avg.parse_story(ctx(script(body)))), "line")
    return parse


# ---- the five whitelisted forms ----

def test_named_line_is_speech(lines):
    got = lines('[name="阿米娅"]  博士，我们该走了。')
    assert (got[0].speaker, got[0].kind) == ("阿米娅", "对白")
    assert got[0].text == "博士，我们该走了。"


def test_empty_speaker_is_narration(lines):
    got = lines('[name=""]  远处传来了声音。')
    assert got[0].speaker is None and got[0].kind == "旁白"


def test_bare_text_is_narration(lines):
    got = lines("天灾过后，什么都没有留下。")
    assert got[0].speaker is None and got[0].kind == "旁白"


def test_decision_becomes_a_choice(ctx, of_kind):
    got = list(avg.parse_story(ctx(script(
        '[name="X"] 问题\n[Decision(options="打回去;算了", values="1;2")]'))))
    choices = of_kind(got, "choice")
    assert choices[0].options == ("打回去", "算了")


def test_subtitle_is_caption(lines):
    got = lines('[Subtitle(text="第一章", size=32)]')
    assert got[0].kind == "字幕" and got[0].text == "第一章"


# ---- variants the guards reported ----

def test_single_quoted_speaker(lines):
    """Both quote styles occur; accepting only double quotes silently drops real lines."""
    assert lines("[name='寒檀']  我明白了。")[0].speaker == "寒檀"


def test_speaker_with_trailing_params(lines):
    assert lines('[name="可露希尔",delay=0.1]  等等！')[0].speaker == "可露希尔"


def test_multiline_continues_the_previous_speaker(lines):
    got = lines('[name="凯尔希"]  第一句。\n[multiline]  第二句。')
    assert len(got) == 1
    assert got[0].text == "第一句。\n第二句。"


def test_spellsticker_carries_incantation(lines):
    """It is dialogue — a spoken spell — not decoration."""
    got = lines('[spellsticker(text="以吾之名", size=24)]')
    assert got and got[0].text == "以吾之名"


def test_escapes_in_screen_text_are_decoded(lines):
    r"""Measured: 1,833 caption lines shipped a literal `\n` mid-sentence."""
    got = lines('[Sticker(id="s", text="\\n我深知我的职责。")]')
    assert "\\n" not in got[0].text
    assert got[0].text == "我深知我的职责。"


def test_player_name_macro_is_normalised(lines):
    """Measured: 467 units shipped the raw macro, so a character recited a variable."""
    got = lines('[name="杜宾"]  Dr.{@nickname}，集合队伍。')
    assert "{@" not in got[0].text and "<用户名>" in got[0].text


def test_capitalised_macro_also_normalised(lines):
    assert "{@" not in lines('[name="W"]  {@Nickname}。')[0].text


def test_nonbreaking_space_macro_becomes_a_space(lines):
    """`{@nbs}` sits inside a proper noun; leaving it makes the name unmatchable."""
    got = lines('[name="睦"]  Ave{@nbs}Mujica是我们的契约。')
    assert "Ave Mujica" in got[0].text


# ---- everything outside the whitelist is discarded ----

@pytest.mark.parametrize("directive", [
    "[Delay(time=1.3)]", "[dalay(time=1)]", "[palysound(key='x')]", "[stopmucis]",
    "[Character(name='char_002_amiya_1')]", "[Image(image='bg', fadetime=1)]",
    "[Blocker(block=true)]", "[charslsot(slot='m')]",
])
def test_directives_including_typos_are_dropped(lines, directive):
    """77 directive spellings exist, six of them misspellings of "delay". A blacklist
    cannot be completed, so only the whitelist produces records."""
    assert lines(directive) == []


def test_navigation_noise_dropped(lines):
    assert lines("{{剧情导航}}\n}}\n//comment") == []


def test_stray_character_after_directive_is_low_severity(ctx):
    page = ctx(script("[Character(name='x')]。"))
    list(avg.parse_story(page))
    severities = [sev for sev, _ in page.drain_warnings()]
    assert severities == ["低"]


def test_sentence_after_directive_is_high_severity(ctx):
    """A clause trailing a directive is the signal that content may be escaping."""
    page = ctx(script("[Character(name='x')]这句话可能是真的台词。"))
    list(avg.parse_story(page))
    assert "高" in [sev for sev, _ in page.drain_warnings()]


def test_header_trailing_text_is_not_reported(ctx):
    """It is an editor's chapter note, and it fires on every story page."""
    page = ctx(script("[HEADER(is_tutorial=true)] 第二关（前）"))
    list(avg.parse_story(page))
    assert page.drain_warnings() == []


# ---- legitimately empty, and genuinely broken ----

def test_transcluded_body_is_reported_not_failed(ctx):
    page = ctx(script("{{:测试页/data}}"), title="测试页")
    assert list(avg.parse_story(page)) == []
    assert "transcluded" in page.expected_empty


def test_data_subpage_is_discovered(ctx):
    found = list(avg.discover_data_subpages("梅尔/干员密录/1", script("{{:{{PAGENAME}}/data}}")))
    assert found == ["梅尔/干员密录/1/data"]


def test_data_subpage_parses_as_its_own_scene(ctx, of_kind):
    """The subpage holds the body and no wrapper, and its records belong to the parent."""
    page = ctx('[name="梅尔"]  台词。', title="梅尔/干员密录/1/data", seed="S1D")
    got = of_kind(list(avg.parse_story(page)), "line")
    assert got and got[0].scene == "梅尔/干员密录/1"


def test_video_only_scene_is_a_fact_not_a_failure(ctx):
    page = ctx(script("[video(name='op.mp4')]"))
    assert list(avg.parse_story(page)) == []
    assert "video" in page.expected_empty


def test_missing_body_parameter_is_high_severity(ctx):
    page = ctx("{{剧情模拟器|图片数据=x}}")
    list(avg.parse_story(page))
    assert "高" in [sev for sev, _ in page.drain_warnings()]


def test_beta_script_inside_canon_is_reported(ctx):
    """The seed set comes from the site's story table, so this means upstream changed."""
    page = ctx("{{剧情模拟器|内测=1|文本数据=\n[name=\"X\"] 台词\n}}")
    assert list(avg.parse_story(page)) == []
    assert "高" in [sev for sev, _ in page.drain_warnings()]
