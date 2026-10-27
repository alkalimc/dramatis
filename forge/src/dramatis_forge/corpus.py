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
import re
from dataclasses import dataclass, field
from pathlib import Path

from . import __version__, segment
from .archive import Archive
from .chunk import CARD_SPAN, Env, archive_triggers, engine_meta_test
from .chunk import build as build_chunks
from .config import RunInfo
from .normalize import (
    TERM_ALIAS, alias_chains, mark_exact_terms, settle_term_targets,
)
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
    #: Headers that still carry game-engine metadata: a voice trigger key, or a scene's
    #: page-part suffix. Both are noise to retrieval and a model may read them aloud.
    engine_meta_headers: int = 0
    #: Voice trigger keys the pack gives no words for.
    unnamed_triggers: list[str] = field(default_factory=list)
    #: Units whose text still carries wiki markup or a comment fragment, by example.
    markup_residue: dict[str, int] = field(default_factory=dict)
    #: Story records (lines, choices) that no unit's span holds, and places where two
    #: neighbouring units of a scene do not meet or do overlap.
    records_uncovered: int = 0
    span_gaps: int = 0
    #: Roster people no dialogue unit is attributed to; memoir scenes missing their owner.
    people_without_lines: list[str] = field(default_factory=list)
    owners_missing: list[str] = field(default_factory=list)
    scenes_with_owner: int = 0


#: Markup that must never survive into a unit: comment fragments, template and link braces,
#: break tags, character references. A comment closer on its own is how a hidden section
#: leaks, which is why this is checked on what shipped rather than trusted from the parsers.
RE_RESIDUE = re.compile(r"<!--|-->|\{\{|\}\}|\[\[|\]\]|<ref\b|<br\b|&[a-z]+;|&#\d+;", re.I)


def _repeats_a_segment(header: str) -> bool:
    """Does a header name the same thing twice in its breadcrumb?"""
    parts = [p.strip() for p in header.split("›") if p.strip()]
    return len(parts) != len(set(parts))


def _engine_meta(archive: Archive, pack: Pack, rep: CorpusReport):
    keys = archive_triggers(archive)
    rep.unnamed_triggers = [k for k in keys if k not in pack.chunking.triggers]
    return engine_meta_test(pack.chunking, keys)


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
    engine_meta = _engine_meta(archive, pack, rep)
    first_copy: dict[tuple, tuple[str, str]] = {}
    copies: dict[str, list[str]] = {}
    ord_ = 0
    env = Env.for_build(archive, pack, params)

    with Folio.create(folio_path) as folio:
        for chunk in build_chunks(archive, pack, env=env):
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
            for hit in RE_RESIDUE.findall(chunk.embed_text):
                rep.markup_residue[hit.lower()] = rep.markup_residue.get(hit.lower(), 0) + 1
            if _repeats_a_segment(chunk.header):
                rep.repeated_header_segments += 1
            if engine_meta(chunk.template, chunk.page, chunk.header):
                rep.engine_meta_headers += 1
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

        _write_roster(archive, folio, pack, params, env)
        _write_relations(archive, folio, pack, params)
        alias_rows = [
            *((r["alias"], r["target"], r["kind"])
              for r in archive.db.execute("SELECT alias,target,kind FROM aliases")),
            # A speaker label the portraits settle is a name the story gives that person.
            *((label, person, SPEAKER_ALIAS) for label, person in env.speakers.speaker_aliases()),
        ]
        alias_rows = mark_exact_terms(
            settle_term_targets(alias_rows),
            params.alias.whole_word_max)
        _check_alias_chains(rep, alias_rows, set(env.speakers.person_of.values()))
        folio.write_aliases(alias_rows)
        for template, n in sorted(env.skipped.items()):
            rep.ledger.add(
                "G5", f"{n} {template} record(s) made no unit: the pack's policy skips their slot")
        _check_consumers(rep, archive, pack)
        _check_story_coverage(rep, archive, pack, spans)
        if env.unresolved_branches:
            scenes = sorted(set(env.unresolved_branches))
            rep.ledger.add(
                "G5", f"{len(env.unresolved_branches)} branch marker(s) in {len(scenes)} "
                      "scene(s) cannot name the reply they follow and show a bare id "
                      f"(first: {', '.join(scenes[:5])})", high=True)
        _check_attribution(rep, archive, folio, pack, env, params)

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
        # What the checks above counted, for the figure report: a defect that is fixed stays
        # visible as a number that must stay at its floor.
        folio.set_meta("checks", {
            "records_uncovered": rep.records_uncovered, "span_gaps": rep.span_gaps,
            "people_without_lines": len(rep.people_without_lines),
            "owners_missing": len(rep.owners_missing), "owner_scenes": rep.scenes_with_owner,
            "markup_residue": sum(rep.markup_residue.values()),
            "skipped": sum(env.skipped.values()),
            "placeholders": env.placeholders,
            "unresolved_branches": len(env.unresolved_branches),
        })
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


#: Alias kind for a speaker label that portraits, not a site page, tie to one person.
SPEAKER_ALIAS = "speaker"


def _material(
    archive: Archive, folio: Folio, dialogue: tuple[str, ...], env: Env
) -> dict[str, int]:
    """Characters of source material that are a person's own, over every template.

    Non-dialogue units attributed to a person are wholly theirs (their voice lines, their
    dossier sections, letters they wrote); identity cards are not counted, being tables. A dialogue unit is shared by everyone speaking
    in it, so it contributes only the person's own lines, resolved through identity.
    """
    material: dict[str, int] = {}
    marks = ",".join("?" * len(dialogue))
    for person_id, chars in folio.db.execute(
        "SELECT u.person_id, SUM(c.chars) FROM unit_persons u JOIN chunks c ON c.id = u.chunk_id "
        f"WHERE c.template NOT IN ({marks}) AND c.span_of NOT LIKE ? "
        "GROUP BY u.person_id", (*dialogue, f"%{CARD_SPAN}"),
    ):
        material[person_id] = int(chars or 0)
    for person_id, chars in env.speakers.own_chars().items():
        material[person_id] = material.get(person_id, 0) + chars
    return material


def _write_roster(
    archive: Archive, folio: Folio, pack: Pack, params: Params, env: Env
) -> None:
    """One row per person, their topical terms, and the host's first-run recommendations.

    `persona_confidence` is defined beside its column in `folio.SCHEMA`; `_material`
    measures the input.
    """
    sources = _person_sources(archive)
    forms = archive.persons()
    material = _material(archive, folio, pack.chunking.shaped("dialogue"), env)
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


def _write_relations(archive: Archive, folio: Folio, pack: Pack, params: Params) -> None:
    """Co-appearance and first-hand knowledge, both from who speaks in which scene.

    A scene is a dialogue unit's `span_of`. `cooccur` counts, per unordered pair, the
    scenes both speak in. `knowledge_scope` is `self` (every unit attributed to the
    person) plus `lived` (the dialogue units within `scope.lived_window` units before and
    after each unit they speak in, in the same scene).
    """
    window = params.scope.lived_window
    dialogue = pack.chunking.shaped("dialogue")
    marks = ",".join("?" * len(dialogue))
    units_of: dict[str, list[str]] = {}
    for scene, chunk_id in folio.db.execute(
        f"SELECT span_of, id FROM chunks WHERE template IN ({marks}) ORDER BY ord", dialogue
    ):
        units_of.setdefault(scene, []).append(chunk_id)
    in_scene: dict[str, set[str]] = {}
    speaks_in: dict[str, list[tuple[str, str]]] = {}
    for scene, person_id, chunk_id in folio.db.execute(
        "SELECT c.span_of, u.person_id, c.id FROM unit_persons u "
        f"JOIN chunks c ON c.id = u.chunk_id WHERE c.template IN ({marks})", dialogue,
    ):
        in_scene.setdefault(scene, set()).add(person_id)
        speaks_in.setdefault(scene, []).append((person_id, chunk_id))

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
        ((person_id, cid) for scene, spoken in speaks_in.items()
         for person_id, cid in _lived(units_of.get(scene, ()), spoken, window)))


def _lived(units: list[str], spoken: list[tuple[str, str]], window: int):
    """(person, unit) for each unit within `window` units of one the person speaks in."""
    at = {cid: i for i, cid in enumerate(units)}
    for person_id, cid in spoken:
        i = at.get(cid)
        if i is None:
            continue
        for j in range(max(i - window, 0), min(i + window + 1, len(units))):
            yield person_id, units[j]


def _check_alias_chains(rep: CorpusReport, rows: list[tuple[str, str, str]], persons: set[str]) -> None:
    """G4, after speaker aliases join the table: no alias may end on another alias."""
    speaker_names = {a for a, _, k in rows if k == SPEAKER_ALIAS}
    hops = [r for r in alias_chains(rows, persons) if r[1] in speaker_names]
    if hops:
        rep.ledger.add(
            "G4", f"{len(hops)} alias(es) point at a speaker alias, not at its person "
                  "(first: " + ", ".join(f"{a}->{t}" for a, t, _ in hops[:5]) + ")")


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
    if rep.engine_meta_headers:
        rep.ledger.add(
            "G5", f"{rep.engine_meta_headers} unit header(s) carry a voice trigger key or a "
                  "scene's page-part suffix", high=True)
    if rep.markup_residue:
        rep.ledger.add(
            "G5", "unit text carries wiki markup: "
                  + ", ".join(f"{k!r} x{n}" for k, n in sorted(rep.markup_residue.items())),
            high=True)
    if rep.unnamed_triggers:
        rep.ledger.add(
            "G5", "voice trigger key(s) the pack gives no words for: "
                  + ", ".join(rep.unnamed_triggers))


#: Record kinds that structure other records or become aliases rather than units.
_NOT_UNITS = frozenset({"scene", "alias", "term"})


def _check_consumers(rep: CorpusReport, archive: Archive, pack: Pack) -> None:
    """G5: a record kind the archive holds but nothing reads is a loss no count shows.

    The output of a parser nobody consumes looks like a working parser: it produces,
    stores and reconciles, and its text reaches no reader.
    """
    from .records import KINDS
    consumed = {src for t in pack.chunking.templates for src in t.sources} | _NOT_UNITS
    for cls in KINDS:
        n = archive.count(cls.TABLE)
        if n and cls.KIND not in consumed:
            rep.ledger.add(
                "G5", f"{n} {cls.KIND} record(s) are archived and no unit template or alias "
                      "rule reads them", high=True)
    terms = archive.count("terms")
    if terms and not archive.count("aliases", "kind=?", (TERM_ALIAS,)):
        rep.ledger.add(
            "G5", f"{terms} glossary term(s) are archived but gave no alias", high=True)


def _check_story_coverage(
    rep: CorpusReport, archive: Archive, pack: Pack,
    spans: dict[str, dict[str, list[tuple[int, int]]]],
) -> None:
    """G5: every line and choice sits in exactly one unit, and a scene's units meet.

    Positional redundancy only sees records counted twice; a record that never reached a
    unit is invisible to it, which is how a whole class of branch options vanished while
    every count looked healthy.
    """
    held: dict[str, set[int]] = {}
    for template in pack.chunking.shaped("dialogue"):
        for scene, ranges in spans.get(template, {}).items():
            held.setdefault(scene, set())
            for lo, hi in set(ranges):
                held[scene].update(range(lo, hi + 1))
    wanted: dict[str, list[int]] = {}
    for table in ("lines", "choices"):
        for r in archive.db.execute(f"SELECT scene,seq FROM {table}"):
            wanted.setdefault(r["scene"], []).append(r["seq"])
    lost = 0
    where: list[str] = []
    for scene, seqs in wanted.items():
        missing = [q for q in seqs if q not in held.get(scene, ())]
        if missing:
            lost += len(missing)
            where.append(scene)
    gaps = 0
    for template in pack.chunking.shaped("dialogue"):
        for ranges in spans.get(template, {}).values():
            ordered = sorted(set(ranges))
            for (_, hi), (lo, _) in zip(ordered, ordered[1:], strict=False):
                if lo != hi + 1:
                    gaps += 1
    rep.records_uncovered, rep.span_gaps = lost, gaps
    if lost:
        rep.ledger.add(
            "G5", f"{lost} story record(s) in {len(where)} scene(s) are in no unit "
                  f"(first: {', '.join(sorted(where)[:5])})", high=True)
    if gaps:
        rep.ledger.add(
            "G5", f"{gaps} place(s) where neighbouring units of a scene do not meet or "
                  "overlap — widening a hit there skips or repeats text", high=True)


def _check_attribution(
    rep: CorpusReport, archive: Archive, folio: Folio, pack: Pack, env: Env, params: Params
) -> None:
    """G4: the attribution has failed quietly when a person has no lines at all or is
    missing from a scene that is about them."""
    dialogue = pack.chunking.shaped("dialogue")
    marks = ",".join("?" * len(dialogue))
    rep.people_without_lines = [r[0] for r in folio.db.execute(
        "SELECT person_id FROM persons p WHERE NOT EXISTS (SELECT 1 FROM unit_persons u "
        f"JOIN chunks c ON c.id = u.chunk_id WHERE u.person_id = p.person_id "
        f"AND c.template IN ({marks})) ORDER BY person_id", dialogue)]
    if rep.people_without_lines:
        shown = ", ".join(rep.people_without_lines[:30])
        more = len(rep.people_without_lines) - 30
        rep.ledger.add(
            "G4", f"{len(rep.people_without_lines)} roster person(s) appear in no dialogue "
                  f"unit: {shown}" + (f" and {more} more" if more > 0 else ""))

    if pack.chunking.owner_scene:
        owner_rx = re.compile(pack.chunking.owner_scene)
        person_of = archive.person_of()
        attributed = {(r[0], r[1]) for r in folio.db.execute(
            "SELECT c.span_of, u.person_id FROM unit_persons u JOIN chunks c "
            f"ON c.id = u.chunk_id WHERE c.template IN ({marks})", dialogue)}
        for r in archive.db.execute("SELECT id FROM scenes ORDER BY id"):
            m = owner_rx.match(r["id"])
            owner = person_of.get(m.group("owner")) if m else None
            if owner is None:
                continue
            rep.scenes_with_owner += 1
            if (r["id"], owner) not in attributed:
                rep.owners_missing.append(r["id"])
        if rep.owners_missing:
            ratio = len(rep.owners_missing) / max(rep.scenes_with_owner, 1)
            rep.ledger.add(
                "G4", f"{len(rep.owners_missing)} of {rep.scenes_with_owner} scene(s) about "
                      "one person have no unit attributed to that person: "
                      + ", ".join(rep.owners_missing[:8]),
                high=ratio > params.speaker.owner_missing_max)

    for scene, label, person in env.speakers.stale_declared:
        rep.ledger.add(
            "G4", f"declared speaker exception ({label!r} as {person!r}) names a person on no "
                  "roster page; it has no effect", page=scene)
    for label in env.speakers.placeholders_resolved():
        rep.ledger.add(
            "G4", f"label {label!r} is a placeholder but a page, redirect or portrait count "
                  "names a person for it; it should belong to nobody")

    for c in sorted(env.speakers.conflicts.values(), key=lambda c: (c.scene, c.label)):
        rep.ledger.add(
            "G4", f"label {c.label!r} is a person's page name but all {c.lines} lines show "
                  f"{c.shown}'s portrait; the label stands ({c.person})", page=c.scene)


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
