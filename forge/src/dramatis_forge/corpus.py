"""Assemble a folio: chunks, lexical index, roster, aliases, stats, manifest.

Vectors are deliberately *not* written here. Encoding needs a model, so folding it
into this stage would make the whole corpus build depend on one — and the chunk set is
the thing worth iterating on, dozens of times, with no model in sight.

Guard G5 runs here. Its job is to catch the failures that produce a *plausible* corpus
— one that builds, counts up, and is quietly worse than it looks: a template that
silently produced nothing, units outside their budget, positional redundancy from a
splitter emitting the same records twice, and headers that repeat themselves. The
header check exists because sampling, not any count, is what reveals a breadcrumb that
names the same section twice.
"""

from __future__ import annotations

import datetime as dt
import json
from dataclasses import dataclass, field
from pathlib import Path

from . import __version__, segment
from .archive import Archive
from .chunk import build as build_chunks
from .config import RunInfo
from .folio import FOLIO_FORMAT_VERSION, Folio
from .guards import Ledger
from .pack import Pack, PersonSource
from .params import Params
from .records import PARSER_VERSION


@dataclass
class CorpusReport:
    chunks: int = 0
    by_template: dict[str, int] = field(default_factory=dict)
    chars_by_template: dict[str, int] = field(default_factory=dict)
    duplicates: int = 0
    #: Units whose text repeats, verbatim, a unit of the same person and template read
    #: from another of that person's form pages: stored once (see `run`).
    form_copies: int = 0
    segmenter: str = ""
    redundancy: dict[str, float] = field(default_factory=dict)
    ledger: Ledger = field(default_factory=Ledger)
    size_stats: dict[str, dict[str, int]] = field(default_factory=dict)
    #: Obligations this corpus places on whatever reads it. Recorded in the folio so the
    #: engine can refuse a corpus whose requirements it does not implement, rather than
    #: quietly serving degraded results.
    requirements: list[str] = field(default_factory=list)
    repeated_header_segments: int = 0


def _repeats_a_segment(header: str) -> bool:
    """Does a header name the same thing twice in its breadcrumb?"""
    parts = [p.strip() for p in header.split("›") if p.strip()]
    return len(parts) != len(set(parts))


def _percentile(values: list[int], q: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    idx = min(int(q * (len(ordered) - 1)), len(ordered) - 1)
    return ordered[idx]


def run(
    archive: Archive,
    pack: Pack,
    folio_path: Path,
    *,
    segmenter: str = "auto",
    params: Params | None = None,
    progress=None,
) -> CorpusReport:
    params = params or Params()
    rep = CorpusReport()
    seg = segment.load(segmenter)
    rep.segmenter = f"{seg.name}/{seg.version}"
    run_info = RunInfo.create("build", pack.name, pack.version)

    sizes: dict[str, list[int]] = {}
    bodies: dict[str, list[int]] = {}
    spans: dict[str, dict[str, list[tuple[int, int]]]] = {}
    seen: set[str] = set()
    batch: list[tuple] = []
    fts: list[tuple[int, str]] = []
    owners: list[tuple[str, str]] = []
    # Attribution keeps roster members only: `person_id` is the key every reader joins on,
    # and a name with no roster row has nothing to join to.
    roster = set(archive.persons())
    # Material reachable through two form pages of one person — an alter's page that
    # repeats the base dossier, a line recorded again under the alter — is one piece of
    # material. The first form's unit is kept and names the other pages it also appears
    # on (manifest `form_copies`); form *differences* are kept as they are. Scenes are
    # left alone: two scenes that share lines are two tellings, not two copies.
    shared = set(pack.chunking.shaped("profile") + pack.chunking.shaped("voice"))
    first_copy: dict[tuple, tuple[str, str]] = {}
    copies: dict[str, list[str]] = {}
    ord_ = 0

    with Folio.create(folio_path) as folio:
        for chunk in build_chunks(archive, pack):
            if chunk.id in seen:
                # Identical text at an identical span. Real, and harmless once it is
                # counted rather than silently written twice into the vector store.
                rep.duplicates += 1
                continue
            seen.add(chunk.id)
            if chunk.template in shared and chunk.persons:
                key = (chunk.template, chunk.persons, chunk.text)
                kept = first_copy.get(key)
                if kept is not None and kept[1] != chunk.page:
                    copies.setdefault(kept[0], []).append(chunk.page)
                    rep.form_copies += 1
                    continue
                first_copy.setdefault(key, (chunk.id, chunk.page))
            batch.append(chunk.row(ord_))
            fts.append((ord_, seg(chunk.embed_text)))
            owners.extend((chunk.id, p) for p in chunk.persons if p in roster)
            rep.by_template[chunk.template] = rep.by_template.get(chunk.template, 0) + 1
            rep.chars_by_template[chunk.template] = (
                rep.chars_by_template.get(chunk.template, 0) + chunk.chars)
            # Budget checks measure what the encoder sees, not the bare body. A voice
            # line of four characters embeds fine once its header names the speaker and
            # the circumstance; judging it on the body alone flags half the voice corpus
            # as unusable when the header is exactly what makes it usable.
            sizes.setdefault(chunk.template, []).append(len(chunk.embed_text))
            bodies.setdefault(chunk.template, []).append(chunk.chars)
            if _repeats_a_segment(chunk.header):
                rep.repeated_header_segments += 1
            if chunk.span_of and chunk.span_from is not None and chunk.span_to is not None:
                spans.setdefault(chunk.template, {}).setdefault(chunk.span_of, []).append(
                    (chunk.span_from, chunk.span_to))
            ord_ += 1
            if len(batch) >= 5000:
                folio.add_chunks(batch)
                folio.add_fts(fts)
                folio.add_unit_persons(owners)
                batch, fts, owners = [], [], []
                if progress is not None:
                    progress(f"chunks {ord_:,}")
        if batch:
            folio.add_chunks(batch)
            folio.add_fts(fts)
            folio.add_unit_persons(owners)
        rep.chunks = ord_

        _write_roster(archive, folio, pack, params)
        _write_relations(archive, folio, pack)
        folio.write_aliases([
            (r["alias"], r["target"], r["kind"])
            for r in archive.db.execute("SELECT alias,target,kind FROM aliases")
        ])

        stats_rows = []
        for template, embed in sorted(sizes.items()):
            body = bodies.get(template, embed)
            stats = {
                "body_p50": _percentile(body, 0.5),
                "body_p95": _percentile(body, 0.95),
                "embed_p50": _percentile(embed, 0.5),
                "embed_p95": _percentile(embed, 0.95),
                "chars_total": sum(body),
            }
            rep.size_stats[template] = stats
            stats_rows.append((
                template, len(embed),
                _percentile(embed, 0.5), _percentile(embed, 0.95), max(embed),
                json.dumps(stats),
            ))
        folio.write_template_stats(stats_rows)

        rep.redundancy = _redundancy(spans, rep.by_template)
        # Units are stored once, so a reader that wants surrounding context must fetch
        # it — the corpus does not carry lookbehind. Declaring the capability makes the
        # obligation explicit instead of leaving readers to discover truncated context.
        rep.requirements.append("neighbor_expand")
        if any(ratio > REDUNDANCY_TOLERANCE for ratio in rep.redundancy.values()):
            rep.requirements.append("span_merge")

        _check(rep, pack, sizes, bodies)

        folio.set_meta("format_version", FOLIO_FORMAT_VERSION)
        folio.set_meta("pack", pack.name)
        folio.set_meta("pack_version", pack.version)
        folio.set_meta("parser_version", PARSER_VERSION)
        folio.set_meta("segmenter", rep.segmenter)
        folio.set_meta("chunk_count", rep.chunks)
        folio.set_meta("chunks_by_template", rep.by_template)
        folio.set_meta("chars_by_template", rep.chars_by_template)
        folio.set_meta("redundancy", rep.redundancy)
        folio.set_meta("requires", rep.requirements)
        folio.set_meta("size_stats", rep.size_stats)
        folio.set_meta("template_shapes", {t.name: t.builder for t in pack.chunking.templates})
        # The folio stores page titles and revisions; a reader composes URLs from these
        # patterns, so a site move does not invalidate stored data.
        folio.set_meta("source_url_pattern", pack.wiki.source_url)
        folio.set_meta("history_url_pattern", pack.wiki.history_url)
        folio.set_meta("revision_url_pattern", pack.wiki.revision_url)
        # Query-side stopwords for the engine: a property of the corpus language.
        folio.set_meta("stopwords", list(pack.stopwords))
        # Read from the units that shipped, not from a hand-listed subset of source
        # tables. The earlier form scanned three tables while units also draw revisions
        # from the fetched-page index, so the manifest under-reported its own watermark
        # and disagreed with the attribution file — the same shape of defect as any other
        # "two records of one fact": neither number was wrong, they answered different
        # questions while claiming to answer one.
        folio.set_meta("source_revid_max", folio.db.execute(
            "SELECT MAX(revid) FROM chunks").fetchone()[0] or 0)
        folio.set_meta("built_at", dt.datetime.now(dt.UTC).isoformat(timespec="seconds"))
        folio.set_meta("build", run_info.as_dict())
        folio.set_meta("wording", dict(pack.wording))
        folio.set_meta("scene_marker", list(pack.scene_marker))
        folio.set_meta("clock.year_offset", pack.year_offset)
        folio.set_meta("user_placeholder", pack.user_placeholder)
        # chunk id -> the other form pages its text also appears on, in form order.
        folio.set_meta("form_copies", {cid: list(dict.fromkeys(pages))
                                       for cid, pages in sorted(copies.items())})
        # Written last: the fingerprint has to cover everything above it.
        folio.set_meta("build_fingerprint", _fingerprint(folio))
        folio.optimize()
    # G5 findings join the others in the archive's findings table, the one place a tally
    # is read from.
    archive.write_findings(rep.ledger.rows(), stage="corpus")
    archive.commit()
    return rep


def _person_sources(archive: Archive) -> dict[str, PersonSource]:
    """Each person's dossier pages merged over their forms, canonical first."""
    out: dict[str, PersonSource] = {}
    for person_id, forms in archive.persons().items():
        facets: dict[str, str] = {}
        sections: list[dict[str, str]] = []
        items: dict[str, object] = {}
        for f in forms:
            row = archive.db.execute(
                "SELECT fields,sections,items FROM dossiers WHERE page=?", (f["page"],)
            ).fetchone()
            if row is None:
                continue
            for k, v in json.loads(row["fields"]).items():
                facets.setdefault(k, v)
            sections.extend(json.loads(row["sections"]))
            for k, v in json.loads(row["items"]).items():
                items.setdefault(k, v)
        out[person_id] = PersonSource(person_id, facets, tuple(sections), items)
    return out


def _material(archive: Archive, folio: Folio, dialogue: tuple[str, ...]) -> dict[str, int]:
    """Characters of source material that are a person's own, over every template.

    Non-dialogue units attributed to a person are wholly theirs (their voice lines, their
    dossier sections, letters they wrote). A dialogue unit is shared by everyone speaking
    in it, so it contributes only the person's own lines, resolved through identity.
    """
    material: dict[str, int] = {}
    marks = ",".join("?" * len(dialogue))
    for person_id, chars in folio.db.execute(
        "SELECT u.person_id, SUM(c.chars) FROM unit_persons u JOIN chunks c ON c.id = u.chunk_id "
        f"WHERE c.template NOT IN ({marks}) GROUP BY u.person_id", dialogue,
    ):
        material[person_id] = int(chars or 0)
    person_of = archive.person_of()
    for speaker, chars in archive.db.execute(
        "SELECT speaker, SUM(LENGTH(text)) FROM lines WHERE speaker IS NOT NULL GROUP BY speaker"
    ):
        person_id = person_of.get(speaker)
        if person_id is not None:
            material[person_id] = material.get(person_id, 0) + int(chars or 0)
    return material


def _write_roster(archive: Archive, folio: Folio, pack: Pack, params: Params) -> None:
    """One row per person, their topical terms, and the host's first-run recommendations.

    `persona_confidence` is defined beside its column in `folio.SCHEMA`; `_material`
    measures the input.
    """
    sources = _person_sources(archive)
    forms = archive.persons()
    material = _material(archive, folio, pack.chunking.shaped("dialogue"))
    present = sorted(m for p in sources if (m := material.get(p, 0)) > 0)
    median = present[len(present) // 2] if present else 0
    display_key = pack.identity.display_facet
    rules = pack.roster

    rows, terms, confidence = [], [], {}
    for person_id, src in sorted(sources.items()):
        m = material.get(person_id, 0)
        confidence[person_id] = round(m / (m + median), 4) if m else 0.0
        birthday = rules.birthday(src)
        if birthday is not None and not _is_birthday(birthday):
            raise ValueError(
                f"pack birthday hook returned {birthday!r} for {person_id!r}; expected MM-DD")
        display = src.facets.get(display_key) if display_key else None
        rows.append((
            person_id, person_id, display or person_id,
            json.dumps([{"page": f["page"], "kind": f["kind"]} for f in forms[person_id]],
                       ensure_ascii=False),
            json.dumps(dict(src.facets), ensure_ascii=False),
            m, confidence[person_id], birthday,
        ))
        terms.extend((person_id, t) for t in dict.fromkeys(rules.topic_terms(src)) if t)
    folio.write_persons(rows)
    folio.write_topic_terms(terms)

    picks = coldstart(sources, confidence, pack, params.coldstart.picks)
    folio.write_prompt(HOST, COLDSTART, json.dumps(picks, ensure_ascii=False),
                       f"forge/{__version__}")


#: The host's subject id and the builder-computed slot in the `prompts` table.
HOST = "host"
COLDSTART = "coldstart"


def _is_birthday(value: str) -> bool:
    if len(value) != 5 or value[2] != "-" or not (value[:2] + value[3:]).isdigit():
        return False
    try:
        dt.date(2000, int(value[:2]), int(value[3:]))  # a leap year admits 02-29
    except ValueError:
        return False
    return True


def coldstart(
    sources: dict[str, PersonSource], confidence: dict[str, float], pack: Pack, picks: int
) -> list[dict[str, str]]:
    """First-run recommendations: the thickest material, spread across affiliations.

    Greedy by `persona_confidence`, skipping a person whose affiliation is already on the
    card; if the roster has fewer affiliations than picks, the rest fill by confidence
    alone. A person with no quotable reason is passed over: the card says why, from the
    material, or it does not recommend.
    """
    ranked = sorted((p for p in sources if confidence.get(p, 0) > 0),
                    key=lambda p: (-confidence[p], p))
    reasons = {p: pack.roster.reason(sources[p]).strip() for p in ranked}
    ranked = [p for p in ranked if reasons[p]]
    chosen: list[str] = []
    seen: set[str] = set()
    for person_id in ranked:
        if len(chosen) == picks:
            break
        faction = pack.roster.faction(sources[person_id])
        if faction and faction in seen:
            continue
        chosen.append(person_id)
        if faction:
            seen.add(faction)
    for person_id in ranked:
        if len(chosen) >= picks:
            break
        if person_id not in chosen:
            chosen.append(person_id)
    return [{"person_id": p, "reason": reasons[p]} for p in chosen]


def _write_relations(archive: Archive, folio: Folio, pack: Pack) -> None:
    """Co-appearance and first-hand knowledge, both from who speaks in which scene.

    A scene is a dialogue unit's `span_of`. `cooccur` counts, per unordered pair, the
    scenes both speak in. `knowledge_scope` is `self` (every unit attributed to the
    person) plus `lived` (the other dialogue units of every scene they speak in).
    """
    dialogue = pack.chunking.shaped("dialogue")
    marks = ",".join("?" * len(dialogue))
    units_of: dict[str, list[str]] = {}
    for scene, chunk_id in folio.db.execute(
        f"SELECT span_of, id FROM chunks WHERE template IN ({marks}) ORDER BY ord", dialogue
    ):
        units_of.setdefault(scene, []).append(chunk_id)
    in_scene: dict[str, set[str]] = {}
    for scene, person_id in folio.db.execute(
        "SELECT DISTINCT c.span_of, u.person_id FROM unit_persons u "
        f"JOIN chunks c ON c.id = u.chunk_id WHERE c.template IN ({marks})", dialogue,
    ):
        in_scene.setdefault(scene, set()).add(person_id)

    pairs: dict[tuple[str, str], int] = {}
    for people in in_scene.values():
        ordered = sorted(people)
        for i, a in enumerate(ordered):
            for b in ordered[i + 1:]:
                pairs[(a, b)] = pairs.get((a, b), 0) + 1
    folio.write_cooccur((a, b, n) for (a, b), n in sorted(pairs.items()))

    folio.db.execute(
        "INSERT INTO knowledge_scope(person_id,chunk_id,kind) "
        "SELECT person_id, chunk_id, 'self' FROM unit_persons")
    # `self` wins where a unit is both: it is the stronger claim.
    folio.db.executemany(
        "INSERT OR IGNORE INTO knowledge_scope(person_id,chunk_id,kind) VALUES(?,?,'lived')",
        ((person_id, cid) for scene, people in in_scene.items()
         for person_id in people for cid in units_of.get(scene, ())))


def _redundancy(
    spans: dict[str, dict[str, list[tuple[int, int]]]], counts: dict[str, int]
) -> dict[str, float]:
    """Fraction of source positions covered by more than one unit, per template.

    Should be zero: units are stored once and widened at query time. A non-zero value
    means the splitter emitted the same records twice.

    Identical ranges are counted once before measuring. An over-long record splits into
    several units that all carry its position, and those parts are *disjoint text at one
    address* rather than duplicate coverage — counting them as overlap would report a
    defect that is not there, which is how a guard loses its meaning.
    """
    out: dict[str, float] = {}
    for template, groups in spans.items():
        covered = 0
        distinct = 0
        for ranges in groups.values():
            unique = set(ranges)
            covered += sum(hi - lo + 1 for lo, hi in unique)
            positions: set[int] = set()
            for lo, hi in unique:
                positions.update(range(lo, hi + 1))
            distinct += len(positions)
        if distinct:
            out[template] = round(max(covered / distinct - 1.0, 0.0), 3)
    return out


#: A unit whose embedded text is shorter than this carries almost no signal beyond its
#: header. Below it a unit is a lexical target only, which is a legitimate role but not
#: one the dense path can serve.
MIN_EMBED_CHARS = 24
#: Redundancy at or below this is rounding, not overlap.
REDUNDANCY_TOLERANCE = 0.005
#: Overlap above this is more than a recall hedge: it means the ranker cannot be
#: allowed to return raw top-k, because most of it will be the same passage.
REDUNDANCY_ALARM = 0.5


def _check(
    rep: CorpusReport,
    pack: Pack,
    sizes: dict[str, list[int]],
    bodies: dict[str, list[int]],
) -> None:
    for template in pack.chunking.templates:
        n = rep.by_template.get(template.name, 0)
        if n == 0:
            rep.ledger.add(
                "G5",
                f"template {template.name!r} produced no units although it is declared "
                f"over records {template.sources}",
                high=True,
            )
            continue
        embed = sizes.get(template.name, [])
        over = sum(1 for v in bodies.get(template.name, []) if v > template.max_chars * 1.25)
        if over:
            rep.ledger.add(
                "G5",
                f"{template.name}: {over} units exceed the {template.max_chars}-char "
                f"budget by more than 25% (max {max(bodies[template.name])}) — the "
                "splitter found no acceptable boundary",
                high=over > n * 0.01,
            )
        tiny = sum(1 for v in embed if v < MIN_EMBED_CHARS)
        if tiny:
            rep.ledger.add(
                "G5",
                f"{template.name}: {tiny} units embed to under {MIN_EMBED_CHARS} chars "
                "including the header — lexical targets only, no dense signal",
                high=tiny > n * 0.2,
            )

    # Units are meant to be stored once. Any positional redundancy is now a defect
    # rather than a deliberate trade, so it is reported on sight and escalates past the
    # point where a result page would start repeating itself.
    for template, ratio in sorted(rep.redundancy.items()):
        if ratio <= REDUNDANCY_TOLERANCE:
            continue
        rep.ledger.add(
            "G5",
            f"{template}: {ratio:.0%} positional redundancy — units should be stored "
            "once and widened at query time; overlap here means the splitter is "
            "emitting the same records twice",
            high=ratio > REDUNDANCY_ALARM,
        )
    if rep.duplicates:
        rep.ledger.add("G5", f"{rep.duplicates} identical units collapsed")
    if rep.form_copies:
        rep.ledger.add("G5", f"{rep.form_copies} unit(s) repeating another form page of the "
                             "same person verbatim, stored once (manifest form_copies)")

    # A header that repeats a path segment wastes the reader's attention and the
    # encoder's budget on the same words twice. Found by sampling, not by a count: the
    # counts all looked fine, and the breadcrumbs read `X › X › …`.
    if rep.repeated_header_segments:
        rep.ledger.add(
            "G5",
            f"{rep.repeated_header_segments} unit(s) have a header that repeats a path "
            "segment — the page title is already in the header, so the path should not "
            "carry it again",
            high=rep.repeated_header_segments > rep.chunks * 0.01,
        )


#: Manifest keys that describe when and where a build ran rather than what it contains.
_UNHASHED = frozenset({"build_fingerprint", "built_at", "build"})


def _fingerprint(folio: Folio) -> str:
    """Hash of the chunk identifiers in order, plus the manifest that describes them.

    Content-derived rather than time-derived, so two builds of the same inputs
    fingerprint identically — which is what makes it usable for the client's
    lineage check and for reproducibility claims. The build time and host stamp are
    therefore left out.
    """
    import hashlib

    h = hashlib.sha256()
    for (cid,) in folio.db.execute("SELECT id FROM chunks ORDER BY ord"):
        h.update(cid.encode())
    for key, value in sorted(folio.manifest().items()):
        if key in _UNHASHED:
            continue
        h.update(f"{key}={json.dumps(value, sort_keys=True, ensure_ascii=False)}".encode())
    return f"sha256:{h.hexdigest()}"
