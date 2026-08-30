"""Dump one page at every stage, so a human can check the parse by reading.

Three files per page:

    <page>.1-source.wikitext   exactly as fetched
    <page>.2-records.json      the structured records
    <page>.3-archived.md       the archived text, as a reader would see it

The third one is the point. Guards catch assumptions that fail loudly; they cannot catch
a parse that is subtly wrong — a name attached to the wrong line, a section that reads as
nonsense because its heading was dropped. The only way to find those is for a person to
read the output next to the input, and that is only going to happen if it is pleasant to
read. So this renders prose, not a dump.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from ..store.archive import Archive

SAFE = re.compile(r"[^\w一-鿿.-]")


def _safe(title: str) -> str:
    return SAFE.sub("_", title)[:80]


def dump(archive: Archive, title: str, outdir: Path) -> list[Path]:
    row = archive.page(title)
    if row is None or not row["wikitext"]:
        return []
    outdir.mkdir(parents=True, exist_ok=True)
    stem = _safe(title)
    paths: list[Path] = []

    source = outdir / f"{stem}.1-source.wikitext"
    source.write_text(row["wikitext"], encoding="utf-8")
    paths.append(source)

    records = collect(archive, title)
    records_path = outdir / f"{stem}.2-records.json"
    records_path.write_text(
        json.dumps(records, ensure_ascii=False, indent=2), encoding="utf-8")
    paths.append(records_path)

    archived = outdir / f"{stem}.3-archived.md"
    archived.write_text(render(title, row, records), encoding="utf-8")
    paths.append(archived)
    return paths


def collect(archive: Archive, title: str) -> dict:
    """Every record that came from this page, whatever kind it is.

    Alias-producing pages are included deliberately. A page whose entire output is two
    hundred alias entries used to be reported as "produced no records", because the
    reporter only looked at corpus tables — which made a working parser look broken.
    """
    db = archive.db
    out: dict = {"title": title, "revid": archive.page(title)["revid"]}

    scene = db.execute("SELECT * FROM scenes WHERE id=?", (title,)).fetchone()
    if scene:
        out["scene"] = dict(scene)
        out["lines"] = [dict(r) for r in db.execute(
            "SELECT seq,speaker,text,kind FROM lines WHERE scene=? ORDER BY seq", (title,))]
        out["choices"] = [
            {"seq": r["seq"], "options": json.loads(r["options"])}
            for r in db.execute(
                "SELECT seq,options FROM choices WHERE scene=? ORDER BY seq", (title,))
        ]

    dossier = db.execute("SELECT * FROM dossiers WHERE page=?", (title,)).fetchone()
    if dossier:
        out["dossier"] = {
            "fields": json.loads(dossier["fields"]),
            "sections": json.loads(dossier["sections"]),
            "items": json.loads(dossier["items"]),
        }

    voices = [dict(r) for r in db.execute(
        "SELECT idx,title,trigger,text,unlock FROM voices WHERE page=? ORDER BY idx", (title,))]
    if voices:
        out["voices"] = voices

    lore = [
        {"path": json.loads(r["path"]), "text": r["text"]}
        for r in db.execute("SELECT path,text FROM lore WHERE page=? ORDER BY path,sig", (title,))
    ]
    if lore:
        out["lore"] = lore

    letters = [dict(r) for r in db.execute(
        "SELECT sender,date,title,body FROM letters WHERE page=? ORDER BY sig", (title,))]
    if letters:
        out["letters"] = letters

    terms = [dict(r) for r in db.execute(
        "SELECT zh,en,other,category FROM terms WHERE page=? ORDER BY zh", (title,))]
    if terms:
        out["terms"] = terms

    refs = [dict(r) for r in db.execute(
        "SELECT name,story_group,description FROM char_refs WHERE page=? ORDER BY name LIMIT 40",
        (title,))]
    if refs:
        total = archive.count("char_refs", "page=?", (title,))
        out["char_refs"] = refs
        out["_char_refs_total"] = total

    # Aliases have no page column — they are a dictionary, not corpus. Attribute them by
    # kind so a page whose only output is aliases still reports its output.
    kinds = archive.get_meta("alias_page_kinds") or {}
    kind = kinds.get(title)
    if kind:
        rows = [dict(r) for r in db.execute(
            "SELECT alias,target FROM aliases WHERE kind=? ORDER BY target LIMIT 60", (kind,))]
        if rows:
            out["aliases"] = rows
            out["_alias_total"] = archive.count("aliases", "kind=?", (kind,))

    forms = [dict(r) for r in db.execute(
        "SELECT page,kind,ordinal FROM forms WHERE person_id=? ORDER BY ordinal", (title,))]
    if len(forms) > 1:
        out["forms"] = forms

    findings = [dict(r) for r in db.execute(
        "SELECT guard,severity,detail FROM guard_findings WHERE page=?", (title,))]
    if findings:
        out["guard_findings"] = findings

    return out


def render(title: str, row, rec: dict) -> str:
    L = [
        f"# {title}",
        "",
        f"> 归档文本。源文 {len(row['wikitext']):,} 字符，revid {row['revid']}。",
        "> 这是下游实际看到的内容：演出指令、排版、外语、现实考据均已剔除。",
        "",
    ]

    if "forms" in rec:
        L += ["**同一人的形态**：" + "、".join(
            f"{f['page']}（{f['kind']}）" for f in rec["forms"]), ""]

    if "scene" in rec:
        s = rec["scene"]
        L += [
            f"**剧情类型** {s['story_type']} · **分组** {s['story_group']}"
            f" · **脚本路径** `{s['text_path']}`",
            "",
            f"台词/旁白/字幕 {len(rec['lines']):,} 条，分支 {len(rec['choices']):,} 处。",
            "", "## 正文", "",
        ]
        by_seq = {c["seq"]: c for c in rec["choices"]}
        for ln in rec["lines"]:
            if ln["kind"] == "对白":
                L.append(f"**{ln['speaker']}**：{ln['text']}")
            elif ln["kind"] == "字幕":
                L.append(f"> 〔字幕〕{ln['text']}")
            else:
                L.append(f"> {ln['text']}")
            L.append("")
            if ln["seq"] + 1 in by_seq:
                L += [f"〔博士的选项〕{' / '.join(by_seq[ln['seq'] + 1]['options'])}", ""]

    if "dossier" in rec:
        d = rec["dossier"]
        L += ["## 档案字段（来自结构化表，不解析信息框）", ""]
        L += [f"- **{k}**：{v}" for k, v in d["fields"].items() if v]
        if d["sections"]:
            L += ["", "## 档案正文", ""]
            for sec in d["sections"]:
                L += [f"### {sec['title']}", "", sec["text"], ""]
        items = {k: v for k, v in d["items"].items() if k != "密录" and isinstance(v, str) and v}
        if items:
            L += ["## 相关文本", ""]
            for k, v in items.items():
                L += [f"**{k}**：{v}", ""]
        if d["items"].get("密录"):
            L += ["## 密录引言", ""]
            L += [f"- **{m['set']}**：{m['intro']}" for m in d["items"]["密录"]]
            L.append("")

    if "voices" in rec:
        L += [f"## 语音（{len(rec['voices'])} 条，仅中文）", ""]
        for v in rec["voices"]:
            cond = f"，{v['unlock']}" if v["unlock"] else ""
            L += [f"**【{v['title']}】** `{v['trigger']}`{cond}", "", v["text"], ""]

    if "lore" in rec:
        L += [f"## 设定条目（{len(rec['lore'])} 节）", ""]
        for s in rec["lore"]:
            L += [f"### {' › '.join(s['path']) or '（正文）'}", "", s["text"], ""]

    if "letters" in rec:
        L += [f"## 邮件（{len(rec['letters'])} 封）", ""]
        for m in rec["letters"]:
            L += [f"### {m['title']}", "", f"*寄件人 {m['sender']} · {m['date']}*", "",
                  m["body"], ""]

    if "terms" in rec:
        L += [f"## 术语（{len(rec['terms'])} 条）", "", "| 中文 | 英文 | 其他 | 分类 |",
              "| --- | --- | --- | --- |"]
        L += [f"| {t['zh']} | {t['en']} | {t['other']} | {t['category']} |" for t in rec["terms"]]
        L.append("")

    if "char_refs" in rec:
        L += [f"## 剧情角色（抽样 {len(rec['char_refs'])} / 全量 {rec['_char_refs_total']} 条）", ""]
        for c in rec["char_refs"]:
            L += [f"### {c['name']}　*（{c['story_group']}）*", "", c["description"], ""]

    if "aliases" in rec:
        L += [
            f"## 别名（抽样 {len(rec['aliases'])} / 全量 {rec['_alias_total']} 条）", "",
            "> 本页产出的是**别名词典**而非语料单元：它服务查询归一"
            "（用户说真名也能找到人），也是零标注评测集的来源。", "",
            "| 别名 | 指向 |", "| --- | --- |",
        ]
        L += [f"| {a['alias']} | {a['target']} |" for a in rec["aliases"]]
        L.append("")

    if "guard_findings" in rec:
        L += ["## 守卫记录", ""]
        L += [f"- `{f['guard']}` [{f['severity']}] {f['detail']}" for f in rec["guard_findings"]]
        L.append("")

    if len(L) <= 6:
        L += ["*（该页未产出任何记录——归因见守卫记录）*", ""]
    return "\n".join(L)
