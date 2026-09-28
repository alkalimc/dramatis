"""A retrieval benchmark built from relations the wiki's editors already wrote.

The observation this rests on: a collaborative encyclopaedia is full of
human-authored equivalence and relatedness judgements that nobody made for our
benefit and that cost nothing to collect. Redirects assert "this name means that
entity". Disambiguation pages assert "this word could mean any of these, and people
confuse them". Name tables and identity declarations assert identity across naming
conventions. Heading paths assert containment. Turned around, each of those is a
labelled retrieval query with a known answer.

One family per relation, each recording which relation produced it so that results can
be read per family rather than as one aggregate that hides the differences. Naming
families are derived from whatever alias kinds the folio holds, so a pack's own alias
kinds become families without the framework knowing their names. Unit shapes are read
from the folio's `template_shapes`, never assumed from template names.

The methodological care that makes this a benchmark rather than a pile of pairs is
**lexical-overlap stratification**. If the query string appears verbatim in the gold
passage, BM25 alone answers it, and a benchmark dominated by such cases measures
string matching while appearing to measure retrieval. Every query is therefore
labelled `verbatim` or `paraphrase` by checking whether the query occurs in its gold
text, and the two strata are reported separately. Without that split, saturation on
this benchmark would be indistinguishable from competence.

Disambiguation siblings do double duty: as queries, and as **hard negatives chosen by
a human who noticed the confusion**. That is a strictly better negative than
random sampling or embedding-nearest-neighbour mining, and it is free.
"""

from __future__ import annotations

import json
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path

from .records import sig
from .folio import Folio

#: Relation families with a fixed meaning. Each is a different claim about what
#: retrieval should do, so metrics are reported per family; a single mean over all of
#: them is not meaningful. Any other alias kind in the folio becomes a naming family
#: described by `NAMING`.
FAMILIES = {
    "redirect": "an alternate name should reach the entity it redirects to",
    "form": "the name of one of a person's forms should reach the one person",
    "disambig": "an ambiguous word plus a qualifier should reach the right entity",
    "section": "a heading path should reach the section it names",
    "voice": "a speaker plus a circumstance should reach the line said then",
}
NAMING = "a name the source declares equivalent ({kind}) should reach its entity"


def describe(family: str) -> str:
    return FAMILIES.get(family) or NAMING.format(kind=family)

VERBATIM, PARAPHRASE = "verbatim", "paraphrase"


@dataclass
class Query:
    qid: str
    family: str
    text: str
    #: chunk_id -> graded relevance. 2 = the passage the relation points at,
    #: 1 = same entity, different passage.
    gold: dict[str, int] = field(default_factory=dict)
    #: Human-identified confusable entities. Not scored as relevant; used to report
    #: how often a retriever prefers a sibling to the answer.
    hard_negatives: list[str] = field(default_factory=list)
    stratum: str = PARAPHRASE
    note: str = ""

    def as_dict(self) -> dict:
        return {
            "qid": self.qid, "family": self.family, "text": self.text,
            "stratum": self.stratum, "note": self.note,
            "gold": self.gold, "hard_negatives": self.hard_negatives,
        }


@dataclass
class Suite:
    queries: list[Query] = field(default_factory=list)
    skipped: dict[str, int] = field(default_factory=dict)

    def add(self, q: Query) -> None:
        if q.gold:
            self.queries.append(q)
        else:
            self.skipped[q.family] = self.skipped.get(q.family, 0) + 1

    def by_family(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for q in self.queries:
            out[q.family] = out.get(q.family, 0) + 1
        return out

    def by_stratum(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for q in self.queries:
            out[q.stratum] = out.get(q.stratum, 0) + 1
        return out

    def strata_by_family(self) -> dict[str, dict[str, int]]:
        out: dict[str, dict[str, int]] = {}
        for q in self.queries:
            out.setdefault(q.family, {}).setdefault(q.stratum, 0)
            out[q.family][q.stratum] += 1
        return out

    def negatives_by_family(self) -> dict[str, dict[str, float]]:
        """How many queries carry structure-chosen hard negatives, and how many each.

        Worth publishing rather than assuming: the highest-quality source
        (disambiguation siblings) turns out to be by far the scarcest, because most
        candidates on those pages point at material deliberately left out of scope.
        Sibling sections and same-speaker lines are what actually supply negatives at
        volume.
        """
        out: dict[str, dict[str, float]] = {}
        for q in self.queries:
            slot = out.setdefault(q.family, {"queries": 0, "with_negatives": 0, "negatives": 0})
            slot["queries"] += 1
            if q.hard_negatives:
                slot["with_negatives"] += 1
                slot["negatives"] += len(q.hard_negatives)
        for slot in out.values():
            slot["mean_negatives"] = (
                round(slot["negatives"] / slot["with_negatives"], 1)
                if slot["with_negatives"] else 0.0
            )
        return out


class _Index:
    """Read-side view of a folio: which chunks belong to which page or person."""

    def __init__(self, folio: Folio) -> None:
        declared = folio.get_meta("template_shapes") or {}
        self.shapes: dict[str, str] = {}
        for (name,) in folio.db.execute("SELECT DISTINCT template FROM chunks"):
            self.shapes[name] = declared.get(name, name)
        self.by_page: dict[str, list[str]] = {}
        self.by_person: dict[str, list[str]] = {}
        self.text: dict[str, str] = {}
        self.title: dict[str, str] = {}
        self.template: dict[str, str] = {}
        for r in folio.db.execute(
            "SELECT id,page,template,title,header,text FROM chunks ORDER BY ord"
        ):
            self.by_page.setdefault(r["page"], []).append(r["id"])
            self.text[r["id"]] = f"{r['header']}\n{r['text']}"
            self.title[r["id"]] = r["title"] or ""
            self.template[r["id"]] = r["template"]
        for r in folio.db.execute(
            "SELECT u.person_id, u.chunk_id FROM unit_persons u "
            "JOIN chunks c ON c.id = u.chunk_id ORDER BY c.ord, u.person_id"
        ):
            self.by_person.setdefault(r["person_id"], []).append(r["chunk_id"])

    def templates_of(self, shape: str) -> list[str]:
        return sorted(t for t, s in self.shapes.items() if s == shape)

    def chunks_for(self, target: str) -> list[str]:
        """Every chunk that speaks for a target, person-first.

        Person before page matters: a target naming an alternate form should collect the
        whole person's material, which is the entire point of resolving identity before
        building a benchmark on top of it.
        """
        return self.by_person.get(target) or self.by_page.get(target) or []


def _stratum(query: str, gold_ids: list[str], index: _Index) -> str:
    """Does the query string appear verbatim in any gold passage?

    This is the guard against a benchmark that looks hard and is not. Queries whose
    text is present in the answer are solvable by exact match; keeping them is useful
    (they are realistic) but pooling them with the rest inflates every score.
    """
    for cid in gold_ids:
        if query and query in index.text.get(cid, ""):
            return VERBATIM
    return PARAPHRASE


def _grade(index: _Index, target: str, *, primary: int = 2) -> dict[str, int]:
    ids = index.chunks_for(target)
    if not ids:
        return {}
    graded = {cid: 1 for cid in ids}
    # The identity card and the profile units are what a "who is this" query wants
    # first; everything else about the person is relevant but secondary.
    for cid in ids:
        if index.shapes.get(index.template.get(cid, "")) == "profile":
            graded[cid] = primary
    if not any(v == primary for v in graded.values()):
        for cid in ids[:3]:
            graded[cid] = primary
    return graded


def build(folio: Folio, *, max_per_family: int = 0) -> Suite:
    index = _Index(folio)
    suite = Suite()

    aliases: dict[str, list[tuple[str, str]]] = {}
    for r in folio.db.execute("SELECT alias,target,kind FROM aliases ORDER BY alias,target"):
        aliases.setdefault(r["kind"], []).append((r["alias"], r["target"]))

    # ---- naming equivalences: every alias kind except disambiguation ----
    for family in sorted(k for k in aliases if k != "disambig"):
        for alias, target in aliases[family]:
            gold = _grade(index, target)
            q = Query(
                qid=f"{family}:{sig(alias, target)}",
                family=family, text=alias, gold=gold,
                note=f"{alias} → {target}",
            )
            q.stratum = _stratum(alias, list(gold), index)
            suite.add(q)

    # ---- 4. disambiguation: query with a qualifier, siblings as hard negatives ----
    grouped: dict[str, list[str]] = {}
    for word, target in aliases.get("disambig", []):
        grouped.setdefault(word, []).append(target)
    for word, candidates in grouped.items():
        if len(candidates) < 2:
            continue  # no ambiguity, so no discrimination is being tested
        for target in candidates:
            gold = _grade(index, target)
            if not gold:
                continue
            siblings = [c for c in candidates if c != target]
            negatives = [cid for s in siblings for cid in index.chunks_for(s)]
            # The qualifier is the distinguishing part of the target's own name, which is
            # how a person would actually disambiguate out loud.
            qualifier = _strip_punct(target.replace(word, "")) or target
            text = f"{word} {qualifier}".strip()
            q = Query(
                qid=f"disambig:{sig(word, target)}",
                family="disambig", text=text, gold=gold,
                hard_negatives=negatives[:64],
                note=f"{word} → {target}; {len(siblings)} sibling(s) as hard negatives",
            )
            q.stratum = _stratum(text, list(gold), index)
            suite.add(q)

    # ---- 5. heading paths: does structure-aware retrieval find the named section ----
    # Siblings under the same page are the hard negatives: near-miss passages selected by
    # the document's own structure rather than by a sampler. Cheaper than mining and
    # better targeted, because an encyclopaedia page's sections are about one subject and
    # differ only in facet.
    lore = _in(index.templates_of("lore"))
    lore_by_page: dict[str, list[tuple[str, str]]] = {}
    for r in folio.db.execute(
        f"SELECT id,page,title,span_of FROM chunks WHERE template IN ({lore[0]}) ORDER BY ord",
        lore[1],
    ):
        lore_by_page.setdefault(r["page"], []).append((r["id"], r["span_of"] or r["id"]))

    for r in folio.db.execute(
        f"SELECT id,page,title,span_of FROM chunks WHERE template IN ({lore[0]}) "
        "AND title <> '' ORDER BY ord", lore[1],
    ):
        text = f"{r['page']} {r['title']}".strip()
        span = r["span_of"] or r["id"]
        # A long section splits into several units; all of them answer the query, the
        # first one best.
        gold = {cid: (2 if cid == r["id"] else 1)
                for cid, s in lore_by_page.get(r["page"], []) if s == span}
        negatives = [cid for cid, s in lore_by_page.get(r["page"], []) if s != span]
        q = Query(
            qid=f"section:{sig(r['id'])}",
            family="section", text=text, gold=gold,
            hard_negatives=negatives[:64],
            note=f"heading path on {r['page']}; {len(negatives)} sibling section(s)",
        )
        q.stratum = _stratum(text, list(gold), index)
        suite.add(q)

    # ---- 6. speaker plus circumstance ----
    # Hard negatives: the same speaker's other lines. Same voice, same register, wrong
    # circumstance — which is exactly the confusion this family is meant to detect, and it
    # cannot be produced by sampling across the corpus.
    voice = _in(index.templates_of("voice"))
    voice_by_person: dict[str, list[str]] = {}
    for r in folio.db.execute(
        f"SELECT c.id, u.person_id AS person FROM chunks c "
        f"JOIN unit_persons u ON u.chunk_id = c.id WHERE c.template IN ({voice[0]}) "
        "ORDER BY c.ord, u.person_id", voice[1],
    ):
        voice_by_person.setdefault(r["person"], []).append(r["id"])

    for r in folio.db.execute(
        f"SELECT c.id, u.person_id AS person, c.title FROM chunks c "
        f"JOIN unit_persons u ON u.chunk_id = c.id WHERE c.template IN ({voice[0]}) "
        "AND c.title <> '' ORDER BY c.ord, u.person_id", voice[1],
    ):
        text = f"{r['person']} {r['title']}"
        siblings = [c for c in voice_by_person.get(r["person"], []) if c != r["id"]]
        q = Query(
            qid=f"voice:{sig(r['id'])}",
            family="voice", text=text, gold={r["id"]: 2},
            hard_negatives=siblings[:64],
            note=f"speaker + trigger label; {len(siblings)} other line(s) by the same speaker",
        )
        q.stratum = _stratum(text, [r["id"]], index)
        suite.add(q)

    if max_per_family:
        # Deterministic subsample: stable hash order, not RNG, so a smaller suite is a
        # subset of the larger one and results stay comparable across sizes.
        kept: dict[str, int] = {}
        chosen: list[Query] = []
        for q in sorted(suite.queries, key=lambda x: x.qid):
            if kept.get(q.family, 0) >= max_per_family:
                continue
            kept[q.family] = kept.get(q.family, 0) + 1
            chosen.append(q)
        suite.queries = chosen
    return suite


def _strip_punct(text: str) -> str:
    """Trim punctuation and spacing of any script from both ends."""
    def keep(ch: str) -> bool:
        return unicodedata.category(ch)[0] not in "PZS"
    start, end = 0, len(text)
    while start < end and not keep(text[start]):
        start += 1
    while end > start and not keep(text[end - 1]):
        end -= 1
    return text[start:end]


def _in(names: list[str]) -> tuple[str, list[str]]:
    """Placeholders and values for `IN (…)`; an empty list matches nothing."""
    return (",".join("?" * len(names)) or "NULL", names)


def write(suite: Suite, outdir: Path, *, folio: Folio) -> dict:
    """Emit the suite as JSONL plus a manifest.

    JSONL rather than a database because this is meant to be published, cited and read
    by other people's tooling. The manifest ties it to the exact corpus build it was
    derived from — a benchmark whose corpus version is unknown cannot be reproduced.
    """
    outdir.mkdir(parents=True, exist_ok=True)
    queries = outdir / "structural.queries.jsonl"
    with queries.open("w", encoding="utf-8") as fh:
        for q in sorted(suite.queries, key=lambda x: x.qid):
            fh.write(json.dumps(q.as_dict(), ensure_ascii=False) + "\n")

    manifest = {
        "suite": "structural",
        "built_from": {
            "folio": folio.path.name,
            "build_fingerprint": folio.get_meta("build_fingerprint"),
            "pack": folio.get_meta("pack"),
            "chunk_count": folio.get_meta("chunk_count"),
        },
        "families": {f: describe(f) for f in sorted(suite.by_family())},
        "counts": {
            "queries": len(suite.queries),
            "by_family": suite.by_family(),
            "by_stratum": suite.by_stratum(),
            "strata_by_family": suite.strata_by_family(),
            "skipped_no_gold": suite.skipped,
            "hard_negatives": suite.negatives_by_family(),
        },
        "reporting_rules": [
            "report per family; a single mean across families is not interpretable",
            "report verbatim and paraphrase strata separately — the verbatim stratum is "
            "solvable by exact match and will saturate",
            "report how often a hard negative outranks the gold passage, per family",
            "the form family is almost entirely verbatim by construction, because a "
            "person's identity card lists their other forms; use it as a sanity floor, "
            "not as evidence of retrieval quality",
            "one family dominates by count; never weight queries equally across families "
            "when computing an overall figure",
        ],
    }
    (outdir / "structural.manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    return manifest
