"""Records → retrieval units.

Five shapes, one builder each, because the shapes are genuinely different and a
single windowing rule serves none of them well. A pack's template names its shape
(`ChunkTemplate.builder`); the builder is chosen by shape, never by template name. Two things every unit carries and
would be unusable without:

**A header.** A dialogue unit stripped of its scene and its speakers is close to
unsearchable — the words are there but nothing says whose they are. The header is
part of the embedded text and part of what is shown in a citation.

**Its span.** `(span_of, span_from, span_to)` locates the unit inside its source
sequence. Units do not overlap, so this is not for deduplication: it is what lets a
reader **widen a hit to its neighbours after ranking**, which is how adjacent context
is recovered without storing it twice. Paying for context on the handful of units
returned rather than on all of them is the whole argument for non-overlapping storage.

Every piece of punctuation a builder writes — label separators, list joiners, inline
marker brackets, preferred cut points — comes from `ChunkPolicy`, because punctuation is
a property of the corpus language.
"""

from __future__ import annotations

import json
from collections.abc import Iterator
from dataclasses import dataclass

from .records import sig
from .pack import ChunkTemplate, Pack
from .archive import Archive


@dataclass(frozen=True, slots=True)
class Chunk:
    template: str
    page: str
    text: str
    header: str = ""
    title: str = ""
    #: Everyone the unit belongs to — all speakers of a dialogue unit, the owner of a
    #: voice line or dossier section — resolved through identity where it knows the name.
    #: The folio builder keeps the roster members among them.
    persons: tuple[str, ...] = ()
    revid: int | None = None
    span_of: str | None = None
    span_from: int | None = None
    span_to: int | None = None

    @property
    def id(self) -> str:
        return f"{self.template}:{sig(self.page, self.span_from, self.span_to, self.text)}"

    @property
    def chars(self) -> int:
        return len(self.text)

    @property
    def embed_text(self) -> str:
        """What the encoder sees, and what the lexical index is built from."""
        return f"{self.header}\n{self.text}" if self.header else self.text

    def row(self, ord_: int) -> tuple:
        return (
            self.id, ord_, self.template, self.page, self.revid,
            self.title, self.header, self.text, self.chars,
            self.span_of, self.span_from, self.span_to,
        )


def _split(text: str, limit: int, breaks: tuple[str, ...]) -> list[str]:
    """Cut over-long text at the most natural boundary available.

    `breaks` is tried in order (paragraph, then sentence, then clause, typically). A
    hard character cut is the last resort rather than the rule: splitting mid-clause
    produces a unit whose first sentence is a fragment, and a fragment embeds as noise.
    """
    if len(text) <= limit:
        return [text]
    out: list[str] = []
    rest = text
    while len(rest) > limit:
        window = rest[:limit]
        cut = -1
        for sep in breaks:
            cut = window.rfind(sep)
            if cut > limit // 3:
                cut += len(sep)
                break
            cut = -1
        if cut <= 0:
            cut = limit
        out.append(rest[:cut].strip())
        rest = rest[cut:].lstrip()
    if rest:
        out.append(rest)
    return [p for p in out if p]


# --------------------------------------------------------------------------- #
# builders
# --------------------------------------------------------------------------- #


def _lore(archive: Archive, pack: Pack, t: ChunkTemplate, header: str) -> Iterator[Chunk]:
    for r in archive.db.execute("SELECT page,path,text,revid FROM lore ORDER BY page,seq"):
        path = json.loads(r["path"])
        breadcrumb = " › ".join(path)
        head = header.format(page=r["page"], path=f" › {breadcrumb}" if breadcrumb else "")
        for i, part in enumerate(_split(r["text"], t.max_chars, pack.chunking.breaks)):
            yield Chunk(
                template=t.name, page=r["page"], text=part, header=head,
                title=breadcrumb or r["page"], revid=r["revid"],
                span_of=f"{r['page']}#{breadcrumb}", span_from=i, span_to=i,
            )


def _turn_aligned(
    rendered: list[tuple[int, str | None, str]], target: int, cap: int, absorb: bool
) -> list[list[tuple[int, str | None, str]]]:
    """Group a scene's lines into non-overlapping units that break between turns.

    Aim for `target` records, then keep extending — up to `cap` — until the speaker
    changes. Cutting a speaker off mid-thought is the failure that actually damages a
    dialogue unit, and a fixed window guarantees it at every boundary; this makes it
    rare at the cost of slightly variable unit length.
    """
    out: list[list[tuple[int, str | None, str]]] = []
    cur: list[tuple[int, str | None, str]] = []
    for i, row in enumerate(rendered):
        cur.append(row)
        if len(cur) < target:
            continue
        nxt = rendered[i + 1] if i + 1 < len(rendered) else None
        if nxt is None or nxt[1] != row[1] or len(cur) >= cap:
            out.append(cur)
            cur = []
    if cur:
        # A stub tail embeds as noise, so fold it back — but not past the cap, or a long
        # monologue produces one oversized unit precisely where the cap exists to prevent
        # it. When absorbing would breach the cap, emitting the short unit is the lesser
        # fault: it is still coherent, just brief.
        absorbable = (
            out and absorb
            and len(cur) < max(target // 2, 1)
            and len(out[-1]) + len(cur) <= cap
        )
        if absorbable:
            out[-1].extend(cur)
        else:
            out.append(cur)
    return out


def _dialogue(archive: Archive, pack: Pack, t: ChunkTemplate, header: str) -> Iterator[Chunk]:
    """One unit per stretch of a scene, non-overlapping, broken between speaker turns.

    Choices are merged into the line sequence rather than kept apart: a branch is part
    of the conversation, and a unit that silently skips it reads as if the reply came
    from nowhere.

    Units do not overlap. Adjacent context is recoverable at query time from
    `(span_of, span_from)`, which is both cheaper and more faithful than baking half a
    window of lookbehind into every unit — see this module's docstring.
    """
    policy = pack.chunking
    label, sep = policy.label, policy.sep
    person_of = archive.person_of()
    subtitle_kind = policy.kind("subtitle")
    target = t.target or 12
    cap = t.max_span or (target + 6)
    scenes = list(archive.db.execute(
        "SELECT id,category,grp,revid FROM scenes ORDER BY id"))
    choices_by_scene: dict[str, dict[int, list[str]]] = {}
    for r in archive.db.execute("SELECT scene,seq,options FROM choices"):
        choices_by_scene.setdefault(r["scene"], {})[r["seq"]] = json.loads(r["options"])

    for scene in scenes:
        rows = list(archive.db.execute(
            "SELECT seq,speaker,text,kind FROM lines WHERE scene=? ORDER BY seq",
            (scene["id"],)))
        if not rows:
            continue
        choices = choices_by_scene.get(scene["id"], {})
        rendered: list[tuple[int, str | None, str]] = []
        for r in rows:
            if r["speaker"]:
                rendered.append((r["seq"], r["speaker"], f"{r['speaker']}{sep('label')}{r['text']}"))
            elif r["kind"] == subtitle_kind:
                rendered.append((r["seq"], None, f"{policy.mark(label('subtitle'))}{r['text']}"))
            else:
                rendered.append((r["seq"], None, r["text"]))
            if r["seq"] + 1 in choices:
                opts = " / ".join(choices[r["seq"] + 1])
                rendered.append((r["seq"] + 1, None, f"{policy.mark(label('protagonist'))}{opts}"))

        for group in _turn_aligned(rendered, target, cap, t.absorb_tail):
            uniq: list[str] = []
            for _seq, speaker, _ in group:
                if speaker and speaker not in uniq:
                    uniq.append(speaker)
            # Every speaker, not only the four the header names: attribution is what
            # person-filtered retrieval reads, so dropping names here drops material.
            owners = tuple(dict.fromkeys(person_of.get(s, s) for s in uniq))
            head = header.format(
                group=scene["grp"] or scene["category"] or label("story"),
                scene=scene["id"],
                speakers=f" · {sep('list').join(uniq[:4])}" if uniq else "",
            )
            body = "\n".join(text for _seq, _s, text in group)
            # An over-long unit still splits on characters, but it keeps one span:
            # the parts are the same stretch of conversation, so a reader widening
            # from any of them should get the same neighbours.
            for part in _split(body, t.max_chars, pack.chunking.breaks):
                yield Chunk(
                    template=t.name, page=scene["id"], text=part, header=head,
                    title=scene["id"], persons=owners, revid=scene["revid"],
                    span_of=scene["id"], span_from=group[0][0], span_to=group[-1][0],
                )


def _voice(archive: Archive, pack: Pack, t: ChunkTemplate, header: str) -> Iterator[Chunk]:
    label = pack.chunking.label
    person_of = archive.person_of()
    # Voice records name their source page; the revision lives with the fetched page.
    # Attribution has to reach every unit, so it is looked up rather than left null.
    revids = {row["title"]: row["revid"]
              for row in archive.db.execute("SELECT title,revid FROM raw.pages")}
    for r in archive.db.execute(
        "SELECT page,subject,idx,variant,title,trigger,text FROM voices ORDER BY subject,page,seq"
    ):
        person = person_of.get(r["subject"], r["subject"])
        # A variant is the same slot recorded again with other words; naming it in the
        # header keeps the two lines from reading as one line said twice.
        title = " · ".join(x for x in (r["title"], r["variant"]) if x)
        head = header.format(
            person=person,
            trigger=r["trigger"] or label("voice"),
            title=f" · {title}" if title else "",
        )
        yield Chunk(
            template=t.name, page=r["page"], text=r["text"], header=head,
            title=r["title"] or r["trigger"] or label("voice"), persons=(person,),
            revid=revids.get(r["page"]),
            span_of=f"{r['subject']}#voice", span_from=r["idx"], span_to=r["idx"],
        )


def _profile(archive: Archive, pack: Pack, t: ChunkTemplate, header: str) -> Iterator[Chunk]:
    """One identity card per person, then one unit per dossier section.

    Cards are assembled per *person*, not per page, so a second form does not produce
    a second half-empty contact. Attribute text is kept out of the prose units
    and given a unit of its own: mixing a table of measurements into a paragraph of
    biography degrades retrieval for both kinds of question.
    """
    label, sep = pack.chunking.label, pack.chunking.sep
    memoir = pack.chunking.kind("memoir")
    person_of = archive.person_of()
    forms = archive.persons()

    for person_id, form_rows in sorted(forms.items()):
        pages = [f["page"] for f in form_rows]
        merged_fields: dict[str, str] = {}
        sections: list[tuple[str, str, str]] = []
        items: list[tuple[str, str, str]] = []
        revid: int | None = None

        for page in pages:
            row = archive.db.execute(
                "SELECT fields,sections,items,revid FROM dossiers WHERE page=?", (page,)
            ).fetchone()
            if row is None:
                continue
            revid = revid or row["revid"]
            for k, v in json.loads(row["fields"]).items():
                merged_fields.setdefault(k, v)
            for s in json.loads(row["sections"]):
                if s.get("text"):
                    sections.append((page, s.get("title", ""), s["text"]))
            for k, v in json.loads(row["items"]).items():
                if k == memoir:
                    for rec in v:
                        if rec.get("intro"):
                            items.append((page, rec.get("set", memoir), rec["intro"]))
                elif isinstance(v, str) and v:
                    items.append((page, k, v))

        if merged_fields:
            body = "\n".join(f"{k}{sep('label')}{v}" for k, v in merged_fields.items())
            others = [p for p in pages if p != person_id]
            if others:
                body += f"\n{label('other_forms')}{sep('label')}" + sep("list").join(others)
            head = header.format(person=person_id, section=f" · {label('card')}")
            yield Chunk(
                template=t.name, page=person_id, text=body, header=head,
                title=f"{person_id} {label('card')}", persons=(person_id,), revid=revid,
                span_of=f"{person_id}#card", span_from=0, span_to=0,
            )

        for i, (page, title, text) in enumerate(sections + items):
            head = header.format(person=person_id, section=f" · {title}" if title else "")
            for part in _split(text, t.max_chars, pack.chunking.breaks):
                yield Chunk(
                    template=t.name, page=page, text=part, header=head,
                    title=title or person_id, persons=(person_id,), revid=revid,
                    span_of=f"{person_id}#dossier", span_from=i, span_to=i,
                )

    ref_revids = {row["title"]: row["revid"]
                  for row in archive.db.execute("SELECT title,revid FROM raw.pages")}
    for r in archive.db.execute(
        "SELECT page,name,grp,description,source FROM char_refs ORDER BY page,seq"
    ):
        person = person_of.get(r["name"])
        head = header.format(
            person=r["name"],
            section=f" · {r['grp']}" if r["grp"] else "",
        )
        body = r["description"]
        if r["source"]:
            body += f"\n{label('source')}{sep('label')}{r['source']}"
        yield Chunk(
            template=t.name, page=r["page"], text=body, header=head,
            title=r["name"], persons=(person,) if person else (),
            revid=ref_revids.get(r["page"]),
            span_of=f"{r['name']}#ref", span_from=0, span_to=0,
        )


def _letter(archive: Archive, pack: Pack, t: ChunkTemplate, header: str) -> Iterator[Chunk]:
    label = pack.chunking.label
    person_of = archive.person_of()
    revids = {row["title"]: row["revid"]
              for row in archive.db.execute("SELECT title,revid FROM raw.pages")}
    for i, r in enumerate(archive.db.execute(
        "SELECT page,sender,date,title,body FROM letters ORDER BY page,seq"
    )):
        head = header.format(
            sender=r["sender"] or label("unsigned"),
            title=f" · {r['title']}" if r["title"] else "",
        )
        body = r["body"]
        if r["date"]:
            body = f"{r['date']}\n{body}"
        for part in _split(body, t.max_chars, pack.chunking.breaks):
            yield Chunk(
                template=t.name, page=r["page"], text=part, header=head,
                title=r["title"] or label("letter"),
                persons=(person_of[r["sender"]],) if r["sender"] in person_of else (),
                revid=revids.get(r["page"]),
                span_of=f"{r['page']}#letter{i}", span_from=i, span_to=i,
            )


BUILDERS = {
    "lore": _lore,
    "dialogue": _dialogue,
    "voice": _voice,
    "profile": _profile,
    "letter": _letter,
}


def build(archive: Archive, pack: Pack, *, only: str | None = None) -> Iterator[Chunk]:
    """Every unit the pack's policy defines, template by template."""
    for template in pack.chunking.templates:
        if only and template.name != only:
            continue
        builder = BUILDERS.get(template.builder)
        if builder is None:
            raise KeyError(
                f"chunk template {template.name!r} names shape {template.builder!r}, which "
                "has no builder — a pack cannot introduce a unit shape the engine cannot "
                "construct"
            )
        header = pack.chunking.headers.get(template.name, "")
        yield from builder(archive, pack, template, header)
