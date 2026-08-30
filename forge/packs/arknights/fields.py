"""Structured table field names, and their translation back into the site's wording.

The site's structured tables use English column codes. Downstream consumers — the
persona generator, retrieval, the roster view — should see the vocabulary the wiki
itself uses, not a schema's shorthand: a generator handed `flex: 卓越` has to guess
what `flex` means, and will guess wrong.
"""

from __future__ import annotations

FIELD_NAMES: dict[str, str] = {
    "idx": "干员序号",
    "cn": "代号",
    "rarity": "稀有度",
    "profession": "职业",
    "subProfession": "分支",
    "tag": "标签",
    "logo": "所属阵营",
    "position": "位置",
    "sex": "性别",
    "combatExperience": "战斗经验",
    "birthPlace": "出身地",
    "dateOfBirth": "生日",
    "race": "种族",
    "height": "身高",
    "infectionStatus": "矿石病感染情况",
    "phy": "物理强度",
    "flex": "战场机动",
    "tolerance": "生理耐受",
    "plan": "战术规划",
    "skill": "战斗技巧",
    "adapt": "源石技艺适应性",
    "cellOriginiumAssimilation": "体细胞与源石融合率",
    "bloodOriginiumCrystalDensity": "血液源石结晶密度",
}

#: Fields that describe a person rather than a unit. Used to build the roster
#: facets the client shows and the retrieval layer filters on; the rest stay on the
#: dossier record but are not promoted to roster attributes.
ROSTER_FACETS: tuple[str, ...] = (
    "代号", "稀有度", "职业", "分支", "所属阵营", "性别", "种族", "出身地", "生日", "身高",
    "战斗经验", "矿石病感染情况",
)

#: Tables to pull during `scope`, with their field specs. `_pageName` must be
#: aliased; querying it bare returns an invalid-field-alias error that reads like
#: "this table does not exist" and has been misdiagnosed as exactly that.
TABLES: dict[str, str] = {
    "story": "_pageName=page,textPath,storyType,storyGroup",
    "chara": "_pageName=page,charIndex=idx,cn,rarity,profession,subProfession,tag,logo,position",
    "chara_extra_info": (
        "_pageName=page,sex,combatExperience,birthPlace,dateOfBirth,race,height,"
        "infectionStatus,phy,flex,tolerance,plan,skill,adapt,"
        "cellOriginiumAssimilation,bloodOriginiumCrystalDensity"
    ),
    "char_memory": "_pageName=page,storySetName,storyIntro,storyIndex,medal,favor,elite",
}

#: One page legitimately has several rows here: one operator can have several record
#: sets. Keying by page drops the extras silently — 55 rows, measured.
MULTI_ROW_TABLES = frozenset({"char_memory", "story"})
