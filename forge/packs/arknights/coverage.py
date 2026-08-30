"""The source's table of contents, annotated with what we took and why not.

This is the document a person reads to decide whether the archive is trustworthy, so it
is organised by the *wiki's* menu rather than by our pipeline: someone who knows the site
can check it section by section without knowing anything about the code.

The standing criterion is **take narrative, leave mechanics**. Numbers, progression,
levels, interface text and real-world sourcing commentary are all excluded — not because
they are low quality, but because a character inside the world has no access to them, and
material a character cannot know is material that can only make them sound wrong.
"""

from __future__ import annotations

from dramatis_forge.pack import CoverageRow

ARCHIVED, PARTIAL, EXCLUDED = "archived", "partial", "excluded"

COVERAGE: tuple[CoverageRow, ...] = (
    # ---- 通用 ----
    CoverageRow(
        "通用 · 干员一览", ARCHIVED,
        "干员档案：客观履历 · 临床诊断 · 档案资料 · 简介 · 信物描述 · 密录引言",
        "seed:S2"),
    CoverageRow(
        "通用 · 干员一览/语音记录", ARCHIVED,
        "中文台词，带触发情境；不取其余十余个语种与音频文件",
        "sql:SELECT COUNT(DISTINCT subject) FROM voices"),
    CoverageRow(
        "通用 · 干员一览/干员密录", ARCHIVED,
        "AVG 脚本，与剧情同一套白名单",
        "sql:SELECT COUNT(*) FROM scenes WHERE story_type='干员密录'"),
    CoverageRow(
        "通用 · 剧情一览（主线）", ARCHIVED,
        "说话人 · 台词 · 旁白 · 字幕 · 分支选项",
        "sql:SELECT COUNT(*) FROM scenes WHERE story_type='主线'"),
    CoverageRow(
        "通用 · 剧情一览（支线/活动）", ARCHIVED, "同上",
        "sql:SELECT COUNT(*) FROM scenes WHERE story_type IN ('支线','剧情')"),
    CoverageRow(
        "通用 · 敌人一览", EXCLUDED,
        "条目以数值与机制为主；剧情中有分量的敌人已由剧情角色一览覆盖"),
    CoverageRow(
        "通用 · 道具一览", EXCLUDED,
        "干员信物已在干员页内；其余是养成材料"),
    CoverageRow(
        "通用 · 关卡一览", EXCLUDED, "地图与数值，戏内人物无从知晓"),
    # ---- 档案 ----
    CoverageRow(
        "档案 · 剧情角色一览", ARCHIVED,
        "未实装为干员的角色，人工撰写的描述 + 所属剧情",
        "sql:SELECT COUNT(*) FROM char_refs"),
    CoverageRow(
        "档案 · 角色真名", ARCHIVED,
        "代号 ↔ 真名，进别名词典而非语料",
        "sql:SELECT COUNT(*) FROM aliases WHERE kind='realname'"),
    CoverageRow(
        "档案 · 角色真名/编辑指南", EXCLUDED,
        "该页的编辑规范，属维基元层"),
    CoverageRow(
        "档案 · 干员剧情一览", PARTIAL,
        "出场矩阵。已枚举但暂不抓取——名册视图尚未消费它，抓下来会留下无人处理的页面"),
    CoverageRow(
        "档案 · 档案信息一览", EXCLUDED, "干员档案字段的汇总，已由结构化表直接取得"),
    CoverageRow("档案 · 干员模组一览", EXCLUDED, "模组数值"),
    CoverageRow("档案 · 干员编号一览", EXCLUDED, "按阵营排列的编号表，语义密度过低"),
    CoverageRow("档案 · 干员预告 / 干员专精", EXCLUDED, "预告属营销，专精属数值"),
    # ---- 系统 ----
    CoverageRow(
        "系统 · 邮件记录", ARCHIVED,
        "干员写给博士的信：无叙述者中介的第一人称语料",
        "sql:SELECT COUNT(*) FROM letters"),
    CoverageRow(
        "系统 · 邮件记录/国际服", EXCLUDED,
        "日文与英文文本，且标注编辑中；与语音只取中文同一条规则"),
    CoverageRow(
        "系统 · 情报处理室", ARCHIVED,
        "按关卡与时点组织的剧情梗概，每条本身即一段完整摘要",
        "sql:SELECT COUNT(*) FROM lore WHERE page='情报处理室'"),
    CoverageRow("系统 · 光荣之路", EXCLUDED, "成就与奖励"),
    # ---- 玩法 ----
    CoverageRow(
        "玩法（全部章节）", EXCLUDED,
        "危机合约 · 集成战略 · 保全派驻 · 生息演算 等，规则与数值"),
    # ---- 拓展 ----
    CoverageRow(
        "拓展 · 贴士一览", PARTIAL,
        "取世界观全节与通常节的背景分类；丢战斗、招募、基建三类操作提示",
        "sql:SELECT COUNT(*) FROM lore WHERE page='贴士一览'"),
    CoverageRow(
        "拓展 · 泰拉词库", ARCHIVED,
        "术语对照，取开发方文本三列，丢海外运营商译文",
        "sql:SELECT COUNT(*) FROM terms"),
    CoverageRow(
        "拓展 · 制作组通讯", EXCLUDED,
        "开发者视角，会把元层词汇带进角色口中"),
    # ---- 趣味 ----
    CoverageRow(
        "趣味 · 泰拉大典", PARTIAL,
        "命名空间全取，页内只取情报面与考据模板的戏内字段；考据面（现实原型、"
        "立绘、名称出处）整体丢弃",
        "sql:SELECT COUNT(DISTINCT page) FROM lore WHERE page LIKE '泰拉大典%'"),
    # ---- 衍生 ----
    CoverageRow(
        "（衍生）游戏内容前瞻", ARCHIVED, "已提及未登场的角色与设定",
        "sql:SELECT COUNT(*) FROM lore WHERE page='游戏内容前瞻'"),
    CoverageRow(
        "（衍生）重定向与消歧义", ARCHIVED,
        "编者写下的同义与易混关系：既是查询归一的词典，也是零标注评测集的来源",
        "sql:SELECT COUNT(*) FROM aliases WHERE kind IN ('redirect','disambig')"),
    CoverageRow(
        "（衍生）异格与升变", ARCHIVED,
        "站点声明的同一性关系，决定一人一 agent",
        "sql:SELECT COUNT(*) FROM forms WHERE kind<>'canonical'"),
    CoverageRow(
        "（全站）媒体文件", EXCLUDED,
        "一份都不归档。运行时按需从源站取用并本地缓存，因此没有再分发问题"),
)
