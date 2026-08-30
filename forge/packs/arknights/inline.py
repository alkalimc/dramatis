"""Inline markup rules for prts.wiki.

Every entry here was added because a guard reported it, not because someone
anticipated it. That order is the point: the table is allowed to be incomplete
because anything outside it is reported rather than guessed at, and the reports are
what grow the table.

The selection criterion for content is not "is this text good" but **"is it true
inside the world"**. A real-world sourcing note about which skateboard brand
inspired a design is well written and correctly cited, and an in-world character
must not know it. Those notes are dropped wholesale.
"""

from __future__ import annotations

import re

from dramatis_forge.pack import ContentSpec, InlineRules

#: Removed entirely, contents included.
DROP = frozenset({
    # real-world sourcing, wiki apparatus, hover notes, pure navigation
    "cite", "dead", "popup", "外部图像", "gallery", "css image crop", "ruby",
    "修正", "修正lite", "编辑中", "存疑内容", "模糊", "reflist", "fa", "cbox2",
    "锚点", "参阅", "参阅二", "参阅三", "剧情跳转", "剧情跳转/关卡", "引言框",
    "百科目录", "revisiontimestamp",
    # parser functions and variables written as templates
    "#time:y年n月j日h:i", "#vardefine:recborderline", "#var:recborderline",
    # icons and gameplay rewards: an item name and a quantity, no narrative
    "材料消耗", "道具图标", "名片头像", "皮肤头像", "家具", "招聘合同",
    "记录修复奖励", "记录修复奖励/子板", "收藏品考据/导航图标",
})

#: Keep one positional parameter's text. -1 = last, 0 = first.
TEXT_PARAM = {
    "color": -1,
    "术语": -1,
    "泰拉大典": 0,
    "角色资料": 0,
    "组织考据": 0,
}

#: Body-bearing templates. Without these five entries the "keep the last
#: positional parameter" default hollows out 992 story synopses and 59
#: encyclopaedia entries — measured, not hypothetical.
CONTENT = {
    # {{剧情简介|stage|stage name|before/after|body}}
    "剧情简介": ContentSpec(positional=(3,), prefix=(0, 1, 2)),
    # {{百科词条|term| | |情报=…}}
    "百科词条": ContentSpec(named=("情报",), prefix=(0,)),
    "事件考据": ContentSpec(named=("考据",), prefix=(0,)),
    "收藏品考据": ContentSpec(named=("考据",), prefix=(0,)),
    # {{游戏内容前瞻|name|blurb|where mentioned}} — keep name and blurb, drop the
    # list of citations, which is wiki bookkeeping rather than world content.
    "游戏内容前瞻": ContentSpec(positional=(1,), prefix=(0,)),
}

#: Rendered form of the player-name placeholder. One constant so the template and
#: the macro spellings cannot drift apart.
PLAYER_NAME = "Dr.<用户名>"

#: The player's name is a runtime substitution, so it must survive normalisation
#: as a marker rather than as whatever the wiki happens to render.
LITERAL = {
    "drname": PLAYER_NAME,
}

#: Substitutions written as game-engine macros rather than wiki templates. No template
#: pass can reach these, because they are not templates — the script text carries them
#: literally. Measured: 467 retrieval units shipped `{@nickname}` raw, so a character
#: would have greeted the player by reciting a variable name.
#:
#: Case matters: both `{@nickname}` and `{@Nickname}` occur, so the patterns are
#: case-insensitive rather than enumerating spellings.
MACROS = (
    (re.compile(r"\{@nickname\}", re.I), "<用户名>"),
    (re.compile(r"\{@name\}", re.I), "<用户名>"),
    # `{@nbs}` is a non-breaking space, used inside a proper noun that must not wrap.
    # It is punctuation, not content: rendering it as a space keeps the name readable
    # and searchable, whereas leaving it makes the name unmatchable.
    (re.compile(r"\{@nbs\}", re.I), " "),
)

#: Heading subtrees dropped wherever they occur: real-world sourcing and art
#: production notes.
DROP_SECTIONS = frozenset({
    "注释与链接", "参考来源", "参考资料", "注释", "数据来源",
    "原型", "立绘", "名称", "美术", "角色立绘", "干员模型", "设定图",
})

#: Wiki meta-space inside an otherwise in-world namespace: editing guidelines and
#: feedback pages. This is the *only* sanctioned use of page-level exclusion — the
#: rule against it targets discarding world content by subtree, not excluding the
#: wiki's own scaffolding.
META_PAGES = ("泰拉大典:条目格式", "泰拉大典:编辑指南", "泰拉大典:反馈与建议")

RULES = InlineRules(
    macros=MACROS,
    drop=DROP,
    text_param=TEXT_PARAM,
    content=CONTENT,
    literal=LITERAL,
    drop_sections=DROP_SECTIONS,
    meta_pages=META_PAGES,
)
