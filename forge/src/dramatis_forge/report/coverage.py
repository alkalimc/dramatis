"""Render the coverage report.

Generic: the annotated table of contents comes from the pack, the counts come from the
archive, and this module only knows how to evaluate a measure and lay out a page. The
previous version had the wiki's menu written into engine code, which meant a second
domain could not produce this report at all.
"""

from __future__ import annotations

from ..normalize.records import LABELS, ORDER
from ..pack import Pack
from ..store.archive import Archive

DISPOSITION_ORDER = {"archived": 0, "partial": 1, "excluded": 2}


def _measure(archive: Archive, spec: str | None) -> str:
    if not spec:
        return "—"
    if spec.startswith("seed:"):
        return f"{archive.count('seeds', 'seed=?', (spec[5:],)):,}"
    if spec.startswith("sql:"):
        try:
            return f"{archive.scalar(spec[4:]) or 0:,}"
        except Exception:
            return "?"
    return "—"


def build(archive: Archive, pack: Pack) -> tuple[str, list[tuple[str, str, str, str]]]:
    rows = [
        (row.section, row.disposition, _measure(archive, row.measure), row.reason)
        for row in pack.coverage
    ]

    counts = archive.get_meta("record_counts") or {}
    chars = archive.get_meta("record_chars") or {}
    tally = archive.get_meta("guard_tally") or {}
    recon = archive.get_meta("reconciliation") or {}

    taken = sum(1 for r in pack.coverage if r.disposition == "archived")
    partial = sum(1 for r in pack.coverage if r.disposition == "partial")
    left = sum(1 for r in pack.coverage if r.disposition == "excluded")

    md: list[str] = [
        f"# 归档内容清单 · {pack.name}",
        "",
        "本文由 `dramatis-forge report coverage` 生成。它按**源站自己的目录**逐节列出"
        "「取了什么、没取什么、为什么」——只列取到的内容无法回答「是不是漏了」，"
        "而那是唯一真正会被问到的问题。",
        "",
        "标准始终是**取叙事，留机制**：数值、养成、关卡、界面文案、现实原型考据一律不取。"
        "不是因为它们质量低，而是因为戏内人物本就无从知晓，"
        "而人物不可能知道的材料只会让他说错话。",
        "",
        f"- 归档库：`{archive.path.name}`",
        f"- 抓取时间：{archive.get_meta('fetched_at', '—')}",
        f"- 解析器版本：{archive.get_meta('parser_version', '—')} · "
        f"适配包版本：{archive.get_meta('pack_version', '—')}",
        f"- 已抓页面：{archive.get_meta('pages_held', 0):,}",
        f"- 处置：已归档 {taken} 节 · 部分归档 {partial} 节 · 不归档 {left} 节",
        "",
        "## 1. 按源站目录",
        "",
        "| 源站板块 | 处置 | 单元数 | 理由 |",
        "| --- | --- | --- | --- |",
    ]
    for section, disposition, count, reason in sorted(
        rows, key=lambda r: (DISPOSITION_ORDER.get(r[1], 9), r[0])
    ):
        md.append(f"| {section} | {disposition} | {count} | {reason} |")

    md += [
        "",
        "## 2. 归一产出",
        "",
        "| 记录 | 含义 | 条数 | 字数 |",
        "| --- | --- | --- | --- |",
    ]
    for kind in ORDER:
        n = counts.get(kind, 0)
        c = chars.get(kind, 0)
        md.append(f"| `{kind}` | {LABELS.get(kind, kind)} | {n:,} | " + (f"{c:,} |" if c else "— |"))
    md += [
        "",
        f"**可用文本合计约 {sum(chars.values()) / 1e6:.2f} M 字**"
        "（已剔除演出指令、外语台词、现实考据、数值与玩法内容）。",
        "",
    ]

    md += ["## 3. 身份：页面不等于人", "", "| 项 | 数 |", "| --- | --- |"]
    md.append(f"| 名册页面 | {archive.count('seeds', 'seed=?', ('S2',)):,} |")
    md.append(f"| 实际人数 | {archive.count('persons'):,} |")
    for r in archive.db.execute(
        "SELECT kind, COUNT(*) AS n FROM forms WHERE kind<>'canonical' GROUP BY kind"
    ):
        md.append(f"| 形态 · {r['kind']} | {r['n']:,} |")
    md.append(f"| 拥有多个形态的人 | {archive.count('persons', 'form_count>1'):,} |")
    md.append("")

    md += ["## 4. 剧情分布", "", "| 剧情类型 | 场景 | 台词 |", "| --- | --- | --- |"]
    for r in archive.db.execute(
        "SELECT s.story_type AS t, COUNT(DISTINCT s.id) AS n, COUNT(l.seq) AS m "
        "FROM scenes s LEFT JOIN lines l ON l.scene = s.id "
        "GROUP BY s.story_type ORDER BY m DESC"
    ):
        md.append(f"| {r['t'] or '（未标注）'} | {r['n']:,} | {r['m']:,} |")

    md += ["", "## 5. 语音按触发情境（前 12）", "", "| 触发 | 条数 |", "| --- | --- |"]
    for r in archive.db.execute(
        "SELECT trigger AS t, COUNT(*) AS n FROM voices GROUP BY trigger "
        "ORDER BY n DESC LIMIT 12"
    ):
        md.append(f"| {r['t'] or '—'} | {r['n']:,} |")

    md += ["", "## 6. 守卫", "", "| 守卫 | 高 | 低 |", "| --- | --- | --- |"]
    from ..normalize.guards import GUARDS

    for guard, description in GUARDS.items():
        hi, lo = tally.get(guard, (0, 0))
        md.append(f"| {guard} {description} | {hi:,} | {lo:,} |")
    md += [
        "",
        "高优先为 0 是设计冻结的出口条件之一。低优先项必须**逐条有归因**——"
        "没有归因的低优先计数是尚未被发现的高优先项。",
        "",
    ]

    if recon.get("produced"):
        md += [
            "## 7. 产出与入库对账",
            "",
            "产出条数与入库条数分别记录，两者之差必须能被「完全重复」解释，否则守卫 G1 报警。"
            "此前只发布产出数而表里更少，于是无法区分去重与覆盖丢失。",
            "",
            "| 记录 | 产出 | 入库 | 重复折叠 |",
            "| --- | --- | --- | --- |",
        ]
        for kind in ORDER:
            made = recon["produced"].get(kind)
            if made is None:
                continue
            md.append(
                f"| `{kind}` | {made:,} | {recon.get('stored', {}).get(kind, 0):,} | "
                f"{recon.get('ignored', {}).get(kind, 0):,} |")
        md.append("")

    return "\n".join(md), rows
