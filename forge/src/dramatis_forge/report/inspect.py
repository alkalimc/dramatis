"""Dump one page at every stage, so a human can check the parse by reading.

Three files per page:

    <page>.1-source.wikitext   exactly as fetched
    <page>.2-records.json      the structured records
    <page>.3-archived.md       the archived text, as a reader would see it

The third one is the point. Guards catch assumptions that fail loudly; they cannot catch
a parse that is subtly wrong — a name attached to the wrong line, a section that reads as
nonsense because its heading was dropped. The only way to find those is for a person to
read the output next to the input, and that is only going to happen if it is pleasant to
read. So this renders prose, not a dump. All of its wording comes from `Pack.say`.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from ..archive import Archive
from ..pack import Pack
from ..text import table_head

SAFE = re.compile(r"[^\w.-]")
#: How many rows of a large per-page listing to show.
SHOW_REFS, SHOW_ALIASES = 40, 60
SUFFIXES = (".1-source.wikitext", ".2-records.json", ".3-archived.md")


def safe_name(title: str) -> str:
    return SAFE.sub("_", title)[:80]


def dump(archive: Archive, pack: Pack, title: str, outdir: Path) -> list[Path]:
    """Write the three files for a page the archive holds. Empty if it holds none."""
    row = archive.page(title)
    if row is None or not row["wikitext"]:
        return []
    return _write(pack, title, row["wikitext"], row["revid"], collect(archive, title, pack),
                  outdir)


def dump_snapshot(pack: Pack, snapshot, outdir: Path) -> list[Path]:
    """The same three files for a page that is gone, from its `raw.removed` snapshot."""
    return _write(pack, snapshot["title"], snapshot["wikitext"] or "", snapshot["revid"],
                  json.loads(snapshot["records"]), outdir)


def _write(pack: Pack, title: str, wikitext: str, revid, records: dict,
           outdir: Path) -> list[Path]:
    outdir.mkdir(parents=True, exist_ok=True)
    stem = safe_name(title)
    source, records_path, archived = (outdir / f"{stem}{s}" for s in SUFFIXES)
    source.write_text(wikitext, encoding="utf-8")
    records_path.write_text(json.dumps(records, ensure_ascii=False, indent=2), encoding="utf-8")
    archived.write_text(render(pack, title, {"wikitext": wikitext, "revid": revid}, records),
                        encoding="utf-8")
    return [source, records_path, archived]


def collect(archive: Archive, title: str, pack: Pack) -> dict:
    """Every record that came from this page, whatever kind it is.

    Alias-producing pages are included deliberately: a page whose entire output is alias
    entries would otherwise read as "produced no records", which makes a working parser
    look broken.
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
        "SELECT idx,title,trigger,text,condition FROM voices WHERE page=? ORDER BY idx", (title,))]
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

    terms = [
        {"term": r["term"], "translations": json.loads(r["translations"]),
         "category": r["category"]}
        for r in db.execute(
            "SELECT term,translations,category FROM terms WHERE page=? ORDER BY term", (title,))
    ]
    if terms:
        out["terms"] = terms

    refs = [dict(r) for r in db.execute(
        "SELECT name,grp,description FROM char_refs WHERE page=? ORDER BY name LIMIT ?",
        (title, SHOW_REFS))]
    if refs:
        out["char_refs"] = refs
        out["_char_refs_total"] = archive.count("char_refs", "page=?", (title,))

    # Aliases have no page column — they are a dictionary, not corpus. Attribute them by
    # kind so a page whose only output is aliases still reports its output.
    kind = pack.alias_pages.get(title)
    if kind:
        rows = [dict(r) for r in db.execute(
            "SELECT alias,target FROM aliases WHERE kind=? ORDER BY target LIMIT ?",
            (kind, SHOW_ALIASES))]
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


def render(pack: Pack, title: str, row, rec: dict) -> str:
    say, policy = pack.say, pack.chunking
    lsep, list_sep = policy.sep("label"), policy.sep("list")
    L = [
        f"# {title}",
        "",
        say("inspect.header", chars=len(row["wikitext"]), revid=row["revid"]),
        say("inspect.header_note"),
        "",
    ]
    preamble = len(L)

    if "forms" in rec:
        L += [say("inspect.forms", forms=list_sep.join(
            say("inspect.form_item", page=f["page"], kind=f["kind"]) for f in rec["forms"])), ""]

    if "scene" in rec:
        s = rec["scene"]
        L += [
            say("inspect.scene_meta", category=s["category"], group=s["grp"],
                source_ref=s["source_ref"]),
            "",
            say("inspect.scene_counts", lines=len(rec["lines"]), choices=len(rec["choices"])),
            "", say("inspect.body"), "",
        ]
        speech, caption = policy.kind("speech"), policy.kind("subtitle")
        by_seq = {c["seq"]: c for c in rec["choices"]}
        for ln in rec["lines"]:
            if ln["speaker"] or ln["kind"] == speech:
                L.append(f"**{ln['speaker']}**{lsep}{ln['text']}")
            elif ln["kind"] == caption:
                L.append(f"> {policy.mark(policy.label('subtitle'))}{ln['text']}")
            else:
                L.append(f"> {ln['text']}")
            L.append("")
            if ln["seq"] + 1 in by_seq:
                L += [say("inspect.choice", mark=policy.mark(policy.label("protagonist")),
                          options=" / ".join(by_seq[ln["seq"] + 1]["options"])), ""]

    if "dossier" in rec:
        d = rec["dossier"]
        memoir = policy.kind("memoir")
        L += [say("inspect.fields"), ""]
        L += [f"- **{k}**{lsep}{v}" for k, v in d["fields"].items() if v]
        if d["sections"]:
            L += ["", say("inspect.dossier_body"), ""]
            for sec in d["sections"]:
                L += [f"### {sec['title']}", "", sec["text"], ""]
        items = {k: v for k, v in d["items"].items() if k != memoir and isinstance(v, str) and v}
        if items:
            L += [say("inspect.items"), ""]
            for k, v in items.items():
                L += [f"**{k}**{lsep}{v}", ""]
        if d["items"].get(memoir):
            L += [say("inspect.memoir", kind=memoir), ""]
            L += [f"- **{m.get('set', '')}**{lsep}{m.get('intro', '')}" for m in d["items"][memoir]]
            L.append("")

    if "voices" in rec:
        L += [say("inspect.voices", n=len(rec["voices"])), ""]
        for v in rec["voices"]:
            cond = say("inspect.voice_condition", condition=v["condition"]) if v["condition"] else ""
            L += [say("inspect.voice_line", title=v["title"], trigger=v["trigger"], condition=cond),
                  "", v["text"], ""]

    if "lore" in rec:
        L += [say("inspect.lore", n=len(rec["lore"])), ""]
        for s in rec["lore"]:
            L += [f"### {' › '.join(s['path']) or say('inspect.untitled')}", "", s["text"], ""]

    if "letters" in rec:
        L += [say("inspect.letters", n=len(rec["letters"])), ""]
        for m in rec["letters"]:
            L += [f"### {m['title']}", "",
                  say("inspect.letter_meta", sender=m["sender"], date=m["date"]), "",
                  m["body"], ""]

    if "terms" in rec:
        labels = sorted({k for t in rec["terms"] for k in t["translations"]})
        L += [say("inspect.terms", n=len(rec["terms"])), "",
              *table_head("|".join([say("inspect.term"), *labels, say("inspect.category")]))]
        for t in rec["terms"]:
            cells = [t["term"], *(t["translations"].get(k, "") for k in labels), t["category"]]
            L.append("| " + " | ".join(cells) + " |")
        L.append("")

    if "char_refs" in rec:
        L += [say("inspect.char_refs", shown=len(rec["char_refs"]),
                  total=rec["_char_refs_total"]), ""]
        for c in rec["char_refs"]:
            L += [say("inspect.char_ref_title", name=c["name"], group=c["grp"]), "",
                  c["description"], ""]

    if "aliases" in rec:
        L += [say("inspect.aliases", shown=len(rec["aliases"]), total=rec["_alias_total"]), "",
              say("inspect.aliases_note"), "", *table_head(say("inspect.alias_head"))]
        L += [f"| {a['alias']} | {a['target']} |" for a in rec["aliases"]]
        L.append("")

    if "guard_findings" in rec:
        L += [say("inspect.findings"), ""]
        L += [f"- `{f['guard']}` [{f['severity']}] {f['detail']}" for f in rec["guard_findings"]]
        L.append("")

    if len(L) <= preamble:
        L += [say("inspect.empty"), ""]
    return "\n".join(L)
