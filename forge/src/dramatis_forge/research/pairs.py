"""Structural training pairs and their hard negatives.

One family per structure the wiki already encodes:

    alias           a redirect or declared equivalent name -> the entity's unit
    disambiguation  an ambiguous word plus a qualifier -> the right entity
    section         a heading path -> that section's unit
    speaker         a speaker plus a circumstance -> the line said then
    identity        the name of a person's other form -> that one person's card

Each construction yields `Raw` pairs with an ordered list of negative candidates; the
assembler then applies the split, so a training pair never names a test page or a test
person even as a negative. The negatives are the structure's own near-misses (siblings on
the same disambiguation page, sibling sections, the same speaker's other lines, people
sharing the person's attributes), not samples drawn across the corpus.
"""

from __future__ import annotations

import hashlib
import json
from collections import Counter
from collections.abc import Callable, Iterable
from itertools import zip_longest
from dataclasses import dataclass, field

from ..evals import _strip_punct
from . import split as split_mod
from .source import Corpus

FAMILIES = ("alias", "disambiguation", "section", "speaker", "identity")

#: Alias kinds that are not the plain "alias" family.
_DISAMBIG, _FORM = "disambig", "form"
#: A lookalike name must share at least this share of its character bigrams.
LOOKALIKE_MIN = 0.3
#: Candidates kept per construction before the split filter; far above `Params.negatives`.
_POOL = 64


@dataclass(frozen=True)
class Params:
    #: Negatives kept per pair.
    negatives: int = 8
    #: Persons' facet whose value defines "the same group" for identity negatives; when
    #: unset, the people sharing the most facet values with the person are used.
    group_facet: str | None = None
    #: Deterministic cap per family before the split (0 = no cap).
    max_per_family: int = 0


@dataclass
class Raw:
    query: str
    positive: str
    family: str
    variant: str
    #: Persons the query itself names (the speaker, the identified person).
    persons: tuple[str, ...] = ()
    #: Ordered negative candidates as (unit id, kind).
    candidates: list[tuple[str, str]] = field(default_factory=list)
    #: Other units that also answer the query (graded lower in the evaluation set).
    also: list[str] = field(default_factory=list)


@dataclass
class Pair:
    query: str
    positive: str
    negatives: list[str]
    family: str
    split: str
    page: str
    persons: list[str]
    variant: str = ""
    neg_kinds: list[str] = field(default_factory=list)
    also: list[str] = field(default_factory=list)

    def as_dict(self) -> dict:
        return {"query": self.query, "positive": self.positive, "negatives": self.negatives,
                "family": self.family, "split": self.split, "page": self.page,
                "persons": self.persons}


def _rank(salt: str, ids: Iterable[str]) -> list[str]:
    """Stable pseudo-random order: independent of enumeration order and of Python's hash seed."""
    return sorted(ids, key=lambda i: hashlib.sha256(f"{salt}\0{i}".encode()).digest())


def _bigrams(text: str) -> set[str]:
    t = "".join(text.lower().split())
    return {t[i:i + 2] for i in range(len(t) - 1)} or ({t} if t else set())


def _dedupe(cands: list[tuple[str, str]], exclude: set[str]) -> list[tuple[str, str]]:
    seen, out = set(exclude), []
    for uid, kind in cands:
        if uid not in seen:
            seen.add(uid)
            out.append((uid, kind))
    return out


# ---- constructions ---------------------------------------------------------------


def alias(c: Corpus) -> Iterable[Raw]:
    by_alias: dict[str, set[str]] = {}
    for rows in c.aliases.values():
        for a, t in rows:
            by_alias.setdefault(a, set()).add(t)
    for kind in sorted(c.aliases):
        if kind in (_DISAMBIG, _FORM):
            continue
        rows = c.aliases[kind]
        index: dict[str, set[int]] = {}
        grams = [_bigrams(a) for a, _ in rows]
        for i, g in enumerate(grams):
            for b in g:
                index.setdefault(b, set()).add(i)
        for i, (a, t) in enumerate(rows):
            pos = c.primary_unit(t)
            if pos is None:
                continue
            cands: list[tuple[str, str]] = []
            for other in sorted(by_alias[a] - {t}):
                u = c.primary_unit(other)
                if u:
                    cands.append((u, "homonym"))
            scored: list[tuple[float, str, str]] = []
            near = Counter(j for b in grams[i] for j in index.get(b, ()) if j != i)
            for j, shared in near.items():
                other_alias, other_t = rows[j]
                if other_t == t:
                    continue
                score = shared / len(grams[i] | grams[j])
                if score >= LOOKALIKE_MIN:
                    scored.append((-score, other_alias, other_t))
            for _, _, other_t in sorted(scored):
                u = c.primary_unit(other_t)
                if u:
                    cands.append((u, "lookalike"))
            also = [x for x in _entity_units(c, t) if x != pos]
            yield Raw(a, pos, "alias", kind, (t,) if t in c.persons else (),
                      _dedupe(cands, {pos}), also)


def disambiguation(c: Corpus) -> Iterable[Raw]:
    grouped: dict[str, list[str]] = {}
    for word, target in c.aliases.get(_DISAMBIG, []):
        grouped.setdefault(word, []).append(target)
    for word in sorted(grouped):
        cands = sorted(set(grouped[word]))
        if len(cands) < 2:
            continue  # nothing is ambiguous, so nothing is being discriminated
        for t in cands:
            pos = c.primary_unit(t)
            if pos is None:
                continue
            qualifier = _strip_punct(t.replace(word, "")) or t
            sib = [(u, "sibling") for s in cands if s != t
                   for u in [c.primary_unit(s)] if u]
            also = [x for x in _entity_units(c, t) if x != pos]
            yield Raw(f"{word} {qualifier}".strip(), pos, "disambiguation", "disambig",
                      (t,) if t in c.persons else (), _dedupe(sib, {pos} | set(also)), also)


def section(c: Corpus) -> Iterable[Raw]:
    seen: set[tuple[str, str]] = set()
    spans: dict[tuple[str, str], list[str]] = {}
    for uid in c.order:
        u = c.units[uid]
        if u.shape == "lore":
            spans.setdefault((u.page, u.span_of), []).append(uid)
    for uid in c.order:
        u = c.units[uid]
        key = (u.page, u.span_of)
        if u.shape != "lore" or not u.title or key in seen:
            continue
        seen.add(key)
        # Sibling sections of the same page, nearest first: the closest section in the
        # document is the most plausible confusion.
        sibs = sorted((s for k, ids in spans.items() if k[0] == u.page and k != key
                       for s in ids[:1]), key=lambda s: abs(c.units[s].ord - u.ord))
        yield Raw(f"{u.page} {u.title}".strip(), uid, "section", "heading_path", (),
                  [(s, "sibling_section") for s in sibs], spans[key][1:])


def speaker(c: Corpus) -> Iterable[Raw]:
    by_span: dict[str, list[str]] = {}
    for uid in c.order:
        if c.units[uid].shape == "dialogue":
            by_span.setdefault(c.units[uid].span_of, []).append(uid)
    for person_id in sorted(c.by_person):
        person = c.persons.get(person_id)
        if person is None:
            continue
        mine = c.by_person[person_id]
        voice = [u for u in mine if c.units[u].shape == "voice"]
        dialogue = [u for u in mine if c.units[u].shape == "dialogue"]
        voice_order = _rank(person_id, voice)
        for uid in voice:
            title = c.units[uid].title
            if not title:
                continue
            others = [u for u in voice_order if u != uid][:_POOL]
            yield Raw(f"{person.display} {title}", uid, "speaker", "voice", (person_id,),
                      [(u, "same_speaker_line") for u in others])
        scenes: dict[str, str] = {}
        for uid in dialogue:
            scenes.setdefault(c.units[uid].span_of, uid)
        dialogue_order = _rank(person_id, dialogue)
        for scene, uid in scenes.items():
            u = c.units[uid]
            if not u.title:
                continue
            in_scene = [x for x in by_span.get(scene, []) if person_id not in c.persons_of(x)]
            in_scene.sort(key=lambda x: abs(c.units[x].ord - u.ord))
            elsewhere = [x for x in dialogue_order if c.units[x].span_of != scene][:_POOL]
            # Interleave the two kinds so a cap keeps both: the same speaker elsewhere
            # tests the circumstance, other speakers here test the attribution.
            cands: list[tuple[str, str]] = []
            for a, b in zip_longest([(x, "same_speaker_other_scene") for x in elsewhere],
                                    [(x, "same_scene_other_speaker") for x in in_scene[:_POOL]]):
                cands += [x for x in (a, b) if x]
            yield Raw(f"{person.display} {u.title}", uid, "speaker", "dialogue", (person_id,),
                      cands)


def identity(c: Corpus, params: Params) -> Iterable[Raw]:
    forms: list[tuple[str, str]] = []
    for person in c.persons.values():
        forms += [(f["page"], person.id) for f in person.forms
                  if f.get("kind") != "canonical" and f["page"] != person.primary_page]
    forms += [(a, t) for a, t in c.aliases.get(_FORM, []) if t in c.persons]
    seen: set[tuple[str, str]] = set()
    cards = {p: c.profile_unit(p) for p in c.persons}
    for name, pid in sorted(forms):
        pos = cards.get(pid)
        if pos is None or (name, pid) in seen:
            continue
        seen.add((name, pid))
        me = c.persons[pid].facets
        scored = []
        for other, card in cards.items():
            if other == pid or card is None:
                continue
            theirs = c.persons[other].facets
            if params.group_facet and (
                    params.group_facet not in me
                    or theirs.get(params.group_facet) != me[params.group_facet]):
                continue
            shared = sum(1 for k, v in me.items() if theirs.get(k) == v)
            scored.append((-shared, hashlib.sha256(f"{pid}\0{other}".encode()).digest(), card))
        cands = [(card, "same_group_person") for _, _, card in sorted(scored)]
        yield Raw(name, pos, "identity", "form", (pid,), _dedupe(cands, {pos}),
                  [x for x in _entity_units(c, pid) if x != pos])


def _entity_units(c: Corpus, target: str) -> list[str]:
    """Every unit that speaks for an entity: a person's own material, else its page's."""
    return c.by_person.get(target) or c.by_page.get(target) or c.by_title.get(target) or []


def construct(c: Corpus, params: Params) -> list[Raw]:
    out: list[Raw] = []
    for build in (alias, disambiguation, section, speaker):
        out += list(build(c))
    out += list(identity(c, params))
    if params.max_per_family:
        kept: list[Raw] = []
        for fam in FAMILIES:
            fam_raw = [r for r in out if r.family == fam]
            fam_raw.sort(key=lambda r: hashlib.sha256(
                f"{fam}\0{r.variant}\0{r.query}\0{r.positive}".encode()).digest())
            kept += fam_raw[:params.max_per_family]
        out = kept
    return out


# ---- assembly --------------------------------------------------------------------


@dataclass
class Assembled:
    pairs: list[Pair] = field(default_factory=list)
    #: Candidate training pairs refused because they involve a test page or person.
    dropped: Counter = field(default_factory=Counter)


def assemble(c: Corpus, raws: Iterable[Raw], sp: split_mod.Split, params: Params) -> Assembled:
    out = Assembled()
    for r in raws:
        u = c.units[r.positive]
        side = sp.split_of_page(u.page)
        persons = set(r.persons) | set(c.persons_of(r.positive))
        if side == "train" and not split_mod.clean_for_train(sp, u.page, persons):
            out.dropped[r.family] += 1
            continue
        negs: list[tuple[str, str]] = []
        for uid, kind in r.candidates:
            nu = c.units[uid]
            if side == "train" and not split_mod.clean_for_train(sp, nu.page, c.persons_of(uid)):
                continue
            negs.append((uid, kind))
            if len(negs) == params.negatives:
                break
        for uid, _ in negs:
            persons.update(c.persons_of(uid))
        out.pairs.append(Pair(
            r.query, r.positive, [x for x, _ in negs], r.family, side, u.page,
            sorted(persons), r.variant, [k for _, k in negs], list(r.also)))
    return out


def unit_row(c: Corpus, uid: str) -> dict:
    u = c.units[uid]
    return {"id": u.id, "header": u.header, "text": u.text, "template": u.template,
            "page": u.page, "persons": list(c.persons_of(uid))}


def dumps(row: dict) -> str:
    return json.dumps(row, ensure_ascii=False, separators=(",", ":"))


Constructor = Callable[[Corpus], Iterable[Raw]]
