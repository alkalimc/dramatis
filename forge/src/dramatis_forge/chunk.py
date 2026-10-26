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
import re
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field
from typing import NamedTuple

from .records import sig
from .pack import ChunkPolicy, ChunkTemplate, Pack
from .params import Params
from .archive import Archive
from .speakers import Speakers


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


#: Suffix of the span address of an identity card. A card is a table of attributes, not
#: something the person said or that was written about them in prose, so readers that
#: measure a person's own material leave it out.
CARD_SPAN = "#card"


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


@dataclass
class Env:
    """What a builder may use besides the archive and the pack."""

    params: Params
    speakers: Speakers = field(init=False)
    notes: list[str] = field(default_factory=list)
    #: Records a policy chose not to make units of, by template.
    skipped: dict[str, int] = field(default_factory=dict)

    @classmethod
    def for_build(cls, archive: Archive, pack: Pack, params: Params | None = None) -> Env:
        params = params or Params()
        env = cls(params)
        env.speakers = Speakers(archive, pack, params.speaker)
        return env


# --------------------------------------------------------------------------- #
# builders
# --------------------------------------------------------------------------- #


def _lore(
    archive: Archive, pack: Pack, t: ChunkTemplate, header: str, env: Env
) -> Iterator[Chunk]:
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


class Row(NamedTuple):
    seq: int
    #: Who speaks, as the script writes it; None for narration, captions and choices.
    speaker: str | None
    text: str
    #: A good place to start a new unit: the scene changes just before this row.
    cut: bool = False
    #: The reply of an earlier choice this row follows (see `Line.branch`).
    branch: str | None = None


def _turn_aligned(rendered: list[Row], target: int, cap: int, absorb: bool) -> list[list[Row]]:
    """Group a scene's rows into non-overlapping units that break between turns.

    Aim for `target` rows, then keep extending — up to `cap` — until the speaker
    changes. Cutting a speaker off mid-thought is the failure that actually damages a
    dialogue unit, and a fixed window guarantees it at every boundary; this makes it
    rare at the cost of slightly variable unit length. A unit that is at least half full
    also ends before a row the script marks as a change of scene: a new place is a better
    seam than a new speaker.
    """
    half = max(target // 2, 1)
    out: list[list[Row]] = []
    cur: list[Row] = []
    for i, row in enumerate(rendered):
        cur.append(row)
        nxt = rendered[i + 1] if i + 1 < len(rendered) else None
        if nxt is None:
            break
        if len(cur) >= target:
            if nxt.speaker != row.speaker or len(cur) >= cap:
                out.append(cur)
                cur = []
        elif len(cur) >= half and nxt.cut:
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


def reply_text(policy: ChunkPolicy, choice: tuple[list[str], list[str]] | None,
                refs: list[str]) -> str:
    """The marker for a stretch that follows some replies of a choice."""
    options, values = choice if choice else ([], [])
    picked: list[str] = []
    for ref in refs:
        if ref in values:
            i = values.index(ref)
        elif not values and ref.isdigit():
            i = int(ref) - 1
        else:
            continue
        if 0 <= i < len(options):
            picked.append(options[i])
    inner = " / ".join(picked) if picked else policy.sep("list").join(refs)
    return policy.mark(f"{policy.label('branch')}{policy.sep('quote').format(inner)}")


def _reply_marker(
    policy: ChunkPolicy, choices: dict[int, tuple[list[str], list[str], str | None]],
    branch: str | None,
) -> str:
    """The line that opens a stretch following some replies, or closes one (`branch` None)."""
    if branch is None:
        return policy.mark(policy.label("merge"))
    at, _, refs = branch.partition(":")
    choice = choices.get(int(at))
    return reply_text(policy, (choice[0], choice[1]) if choice else None, refs.split(";"))


def _dialogue(
    archive: Archive, pack: Pack, t: ChunkTemplate, header: str, env: Env
) -> Iterator[Chunk]:
    """One unit per stretch of a scene, non-overlapping, broken between speaker turns.

    Lines and choices are merged into one sequence by position: a branch is part of the
    conversation, and a unit that silently skips it reads as if the reply came from
    nowhere. Replies that follow different options of a choice are mutually exclusive, so
    the stretch that follows each is introduced by the option it answers; read without
    that, two alternative answers look like one conversation.

    Units do not overlap. Adjacent context is recoverable at query time from
    `(span_of, span_from)`, which is both cheaper and more faithful than baking half a
    window of lookbehind into every unit — see this module's docstring.
    """
    policy = pack.chunking
    label, sep = policy.label, policy.sep
    subtitle_kind = policy.kind("subtitle")
    target = t.target or 12
    cap = t.max_span or (target + 6)
    speakers = env.speakers
    scenes = list(archive.db.execute(
        "SELECT id,category,grp,revid,title FROM scenes ORDER BY id"))
    choices_by_scene: dict[str, dict[int, tuple[list[str], list[str], str | None]]] = {}
    for r in archive.db.execute("SELECT scene,seq,options,vals,branch FROM choices"):
        choices_by_scene.setdefault(r["scene"], {})[r["seq"]] = (
            json.loads(r["options"]), json.loads(r["vals"]), r["branch"])

    for scene in scenes:
        rows = list(archive.db.execute(
            "SELECT seq,speaker,text,kind,portrait,branch,brk FROM lines WHERE scene=? "
            "ORDER BY seq", (scene["id"],)))
        choices = choices_by_scene.get(scene["id"], {})
        if not rows and not choices:
            continue
        owners = speakers.scene_owners(scene["id"], rows)
        by_seq: dict[int, Row] = {}
        for r in rows:
            if r["speaker"]:
                first, *rest = r["text"].split("\n")
                # A line break inside one speaker's words is still that speaker talking.
                text = "\n".join(f"{r['speaker']}{sep('label')}{x}" for x in (first, *rest))
                by_seq[r["seq"]] = Row(r["seq"], r["speaker"], text, bool(r["brk"]), r["branch"])
            elif r["kind"] == subtitle_kind:
                by_seq[r["seq"]] = Row(
                    r["seq"], None, f"{policy.mark(label('subtitle'))}{r['text']}",
                    bool(r["brk"]), r["branch"])
            else:
                by_seq[r["seq"]] = Row(r["seq"], None, r["text"], bool(r["brk"]), r["branch"])
        for seq, (options, _values, branch) in choices.items():
            # Options the player could pick, not words anyone said: labelled as such, so a
            # reader never takes an unchosen reply for one that happened.
            by_seq[seq] = Row(
                seq, None, f"{policy.mark(label('options'))}{' / '.join(options)}",
                False, branch)
        rendered = [by_seq[k] for k in sorted(by_seq)]

        for group in _turn_aligned(rendered, target, cap, t.absorb_tail):
            uniq: list[str] = []
            for row in group:
                if row.speaker and row.speaker not in uniq:
                    uniq.append(row.speaker)
            # Every speaker, not only the four the header names: attribution is what
            # person-filtered retrieval reads, so dropping names here drops material.
            persons = tuple(dict.fromkeys(owners[s] for s in uniq if owners.get(s)))
            head = header.format(
                group=scene["grp"] or scene["category"] or label("story"),
                scene=scene["title"] or policy.scene_name(scene["id"]),
                speakers=f" · {sep('list').join(uniq[:4])}" if uniq else "",
            )
            parts, here = [], None
            for row in group:
                if row.branch != here:
                    parts.append(_reply_marker(policy, choices, row.branch))
                    here = row.branch
                parts.append(row.text)
            body = "\n".join(parts)
            # An over-long unit still splits on characters, but it keeps one span:
            # the parts are the same stretch of conversation, so a reader widening
            # from any of them should get the same neighbours.
            for part in _split(body, t.max_chars, pack.chunking.breaks):
                yield Chunk(
                    template=t.name, page=scene["id"], text=part, header=head,
                    title=scene["id"], persons=persons, revid=scene["revid"],
                    span_of=scene["id"], span_from=group[0].seq, span_to=group[-1].seq,
                )


def _display_names(archive: Archive, pack: Pack) -> dict[str, str]:
    """Page -> the name a reader sees for it, where the dossier gives one."""
    key = pack.identity.display_facet
    out: dict[str, str] = {}
    if key:
        for r in archive.db.execute("SELECT page,fields FROM dossiers"):
            value = json.loads(r["fields"]).get(key)
            if value:
                out[r["page"]] = value
    return out


def _form_tag(archive: Archive, pack: Pack) -> Callable[[str], str]:
    """Page -> the text that tells a person's forms apart in a header ("" for the person).

    Two forms of one person speak differently and say different things in the same slot, so
    a header that names only the person makes the two read as one slot said twice.
    """
    display = _display_names(archive, pack)
    person_of = archive.person_of()
    sep = pack.chunking.sep

    def tag(page: str) -> str:
        person = person_of.get(page)
        if person is None or page == person:
            return ""
        name, base = display.get(page, page), display.get(person, person)
        return sep("form").format(name) if name != base else ""
    return tag


def _voice(
    archive: Archive, pack: Pack, t: ChunkTemplate, header: str, env: Env
) -> Iterator[Chunk]:
    policy = pack.chunking
    person_of = archive.person_of()
    tag = _form_tag(archive, pack)
    # Voice records name their source page; the revision lives with the fetched page.
    # Attribution has to reach every unit, so it is looked up rather than left null.
    revids = {row["title"]: row["revid"]
              for row in archive.db.execute("SELECT title,revid FROM raw.pages")}
    # Person by person, forms in their declared order, lines in page order: when two forms
    # repeat a line, the first form's unit is the one the corpus keeps.
    ordinal = {r["page"]: r["ordinal"] for r in archive.db.execute("SELECT page,ordinal FROM forms")}
    rows = archive.db.execute(
        "SELECT page,subject,idx,variant,title,trigger,text FROM voices ORDER BY subject,page,seq"
    ).fetchall()
    rows.sort(key=lambda r: (person_of.get(r["subject"], r["subject"]),
                             ordinal.get(r["subject"], 0), r["subject"]))
    for r in rows:
        if r["trigger"] in policy.voice_skip:
            env.skipped["voice"] = env.skipped.get("voice", 0) + 1
            continue
        person = person_of.get(r["subject"], r["subject"])
        # A variant is the same slot recorded again with other words; naming it in the
        # header keeps the two lines from reading as one line said twice.
        name = policy.voice_name(r["trigger"] or "", r["title"] or "")
        head = header.format(
            person=f"{person}{tag(r['subject'])}", name=name,
            variant=f" · {r['variant']}" if r["variant"] else "",
        )
        yield Chunk(
            template=t.name, page=r["page"], text=r["text"], header=head,
            title=name, persons=(person,),
            revid=revids.get(r["page"]),
            span_of=f"{r['subject']}#voice", span_from=r["idx"], span_to=r["idx"],
        )


def _profile(
    archive: Archive, pack: Pack, t: ChunkTemplate, header: str, env: Env
) -> Iterator[Chunk]:
    """One identity card per person, then one unit per dossier section.

    Cards are assembled per *person*, not per page, so a second form does not produce
    a second half-empty contact: a form adds a card of its own only for the fields on
    which it differs from the person's first page. Attribute text is kept out of the prose
    units and given a unit of its own: mixing a table of measurements into a paragraph of
    biography degrades retrieval for both kinds of question.
    """
    policy = pack.chunking
    label, sep = policy.label, policy.sep
    memoir = policy.kind("memoir")
    omit = set(policy.card_omit)
    person_of = archive.person_of()
    forms = archive.persons()
    display = _display_names(archive, pack)
    tag = _form_tag(archive, pack)

    dossier: dict[str, dict] = {}
    for row in archive.db.execute("SELECT page,fields,sections,items,revid FROM dossiers"):
        dossier[row["page"]] = {
            "fields": json.loads(row["fields"]), "revid": row["revid"],
            "sections": json.loads(row["sections"]), "items": json.loads(row["items"]),
        }

    # Stock texts: the same item text on many persons is the work's, not theirs.
    stock: dict[tuple[str, str], set[str]] = {}
    if policy.shared_min:
        for person_id, form_rows in forms.items():
            for f in form_rows:
                for k, v in dossier.get(f["page"], {}).get("items", {}).items():
                    if isinstance(v, str) and v:
                        stock.setdefault((k, v), set()).add(person_id)
    is_stock = {kv for kv, owners in stock.items() if len(owners) >= policy.shared_min}
    stock_first: dict[tuple[str, str], str] = {}

    for person_id, form_rows in sorted(forms.items()):
        pages = [f["page"] for f in form_rows]
        held = [p for p in pages if p in dossier]
        sections: list[tuple[str, str, str]] = []
        items: list[tuple[str, str, str]] = []

        for page in held:
            for s in dossier[page]["sections"]:
                if s.get("text"):
                    sections.append((page, s.get("title", ""), s["text"]))
            for k, v in dossier[page]["items"].items():
                if k == memoir:
                    for rec in v:
                        if rec.get("intro"):
                            items.append((page, rec.get("set", memoir), rec["intro"]))
                elif isinstance(v, str) and v:
                    if (k, v) in is_stock:
                        stock_first.setdefault((k, v), page)
                        continue
                    items.append((page, k, v))

        base_page = next((p for p in held if dossier[p]["fields"]), None)
        if base_page is not None:
            base = {k: v for k, v in dossier[base_page]["fields"].items() if k not in omit}
            if base:
                body = "\n".join(f"{k}{sep('label')}{v}" for k, v in base.items())
                others = list(dict.fromkeys(
                    display.get(p, p) for p in pages
                    if p != base_page and display.get(p, p) != display.get(base_page, base_page)))
                if others:
                    body += f"\n{label('other_forms')}{sep('label')}" + sep("list").join(others)
                head = header.format(person=person_id, section=f" · {label('card')}")
                yield Chunk(
                    template=t.name, page=person_id, text=body, header=head,
                    title=f"{person_id} {label('card')}", persons=(person_id,),
                    revid=dossier[base_page]["revid"],
                    span_of=f"{person_id}{CARD_SPAN}", span_from=0, span_to=0,
                )
            for n, page in enumerate(held, start=1):
                if page == base_page:
                    continue
                diff = {k: v for k, v in dossier[page]["fields"].items()
                        if k not in omit and base.get(k) != v}
                if not diff:
                    continue
                head = header.format(
                    person=f"{person_id}{tag(page)}", section=f" · {label('card')}")
                yield Chunk(
                    template=t.name, page=page,
                    text="\n".join(f"{k}{sep('label')}{v}" for k, v in diff.items()),
                    header=head, title=f"{person_id} {label('card')}", persons=(person_id,),
                    revid=dossier[page]["revid"],
                    span_of=f"{person_id}{CARD_SPAN}", span_from=n, span_to=n,
                )

        for i, (page, title, text) in enumerate(sections + items):
            head = header.format(
                person=f"{person_id}{tag(page)}",
                section=f" · {policy.section_name(title)}" if title else "")
            for part in _split(text, t.max_chars, policy.breaks):
                yield Chunk(
                    template=t.name, page=page, text=part, header=head,
                    title=title or person_id, persons=(person_id,),
                    revid=dossier[page]["revid"],
                    span_of=f"{person_id}#dossier", span_from=i, span_to=i,
                )

    for n, ((key, text), page) in enumerate(sorted(stock_first.items())):
        head = header.format(person=label("shared"), section=f" · {policy.section_name(key)}")
        yield Chunk(
            template=t.name, page=page, text=text, header=head, title=key,
            revid=dossier[page]["revid"], span_of=f"{label('shared')}#stock",
            span_from=n, span_to=n,
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


def _letter(
    archive: Archive, pack: Pack, t: ChunkTemplate, header: str, env: Env
) -> Iterator[Chunk]:
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


def archive_triggers(archive: Archive) -> list[str]:
    """Every voice trigger key the archive holds."""
    return [r[0] for r in archive.db.execute(
        "SELECT DISTINCT trigger FROM voices WHERE trigger <> '' ORDER BY trigger")]


def engine_meta_test(policy: ChunkPolicy, triggers: list[str]) -> Callable[[str, str, str], bool]:
    """`(template, page, header) -> bool`: does a unit header still show engine metadata?

    Three kinds: a voice trigger key the archive holds, matched as a whole word anywhere
    in any header; a word of the work's interface the pack lists (`header_taboo`); and a
    dialogue header that names its scene by the full page title where the pack's page-part
    rule shortens it.
    """
    keys = sorted(set(triggers), key=len, reverse=True)
    # ASCII word edges: a key glued to text in another script is still the key.
    key_rx = (re.compile(r"(?<![A-Za-z0-9_-])(?:" + "|".join(map(re.escape, keys))
                         + r")(?![A-Za-z0-9_-])") if keys else None)
    dialogue = set(policy.shaped("dialogue"))
    taboo = re.compile("|".join(policy.header_taboo)) if policy.header_taboo else None

    def test(template: str, page: str, header: str) -> bool:
        if key_rx is not None and key_rx.search(header):
            return True
        if taboo is not None and taboo.search(header):
            return True
        return (template in dialogue and policy.scene_name(page) != page
                and page in header)
    return test


BUILDERS = {
    "lore": _lore,
    "dialogue": _dialogue,
    "voice": _voice,
    "profile": _profile,
    "letter": _letter,
}


def build(
    archive: Archive, pack: Pack, *, only: str | None = None, env: Env | None = None
) -> Iterator[Chunk]:
    """Every unit the pack's policy defines, template by template."""
    env = env or Env.for_build(archive, pack)
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
        yield from builder(archive, pack, template, header, env)
