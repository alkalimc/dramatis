"""Assemble a folio: chunks, lexical index, roster, aliases, stats, manifest.

Vectors are deliberately *not* written here. Encoding needs a model endpoint, so
folding it into this stage would make the whole corpus build depend on one being
reachable — and the chunk set is the thing worth iterating on, dozens of times, with
no model in sight. `encode` fills the vector table afterwards and is the only stage
that needs weights.

Guard G5 runs here. Its job is to catch the failures that produce a *plausible* corpus
— one that builds, counts up, and is quietly worse than it looks: a template that
silently produced nothing, units outside their budget, positional redundancy from a
splitter emitting the same records twice, and headers that repeat themselves. Every one
of those was a real finding rather than a hypothetical; the header check exists because
sampling caught 992 units reading `情报处理室 › 情报处理室 › …`, which no count would
ever have revealed.
"""

from __future__ import annotations

import datetime as dt
import json
from dataclasses import dataclass, field
from pathlib import Path

from ..config import RunInfo
from ..normalize.guards import Ledger
from ..normalize.records import PARSER_VERSION
from ..pack import Pack
from ..store.archive import Archive
from ..store.folio import FOLIO_FORMAT_VERSION, Folio
from . import tokenize
from .chunk import Chunk, build as build_chunks

#: What a citation card links to. The folio stores the page title and revision, and
#: the client composes the URL, so a wiki move does not invalidate stored data.


@dataclass
class CorpusReport:
    chunks: int = 0
    by_template: dict[str, int] = field(default_factory=dict)
    chars_by_template: dict[str, int] = field(default_factory=dict)
    duplicates: int = 0
    segmenter: str = ""
    redundancy: dict[str, float] = field(default_factory=dict)
    ledger: Ledger = field(default_factory=Ledger)
    vector_bytes: dict[int, int] = field(default_factory=dict)
    size_stats: dict[str, dict[str, int]] = field(default_factory=dict)
    #: Obligations this corpus places on whatever reads it. Recorded in the folio so the
    #: engine can refuse a corpus whose requirements it does not implement, rather than
    #: quietly serving degraded results.
    requirements: list[str] = field(default_factory=list)
    repeated_header_segments: int = 0

    @property
    def total_chars(self) -> int:
        return sum(self.chars_by_template.values())

    @property
    def clean(self) -> bool:
        return self.ledger.clean


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
    progress=None,
) -> CorpusReport:
    rep = CorpusReport()
    seg = tokenize.load(segmenter)
    rep.segmenter = f"{seg.name}/{seg.version}"
    run_info = RunInfo.create("corpus", pack.name, pack.version)

    sizes: dict[str, list[int]] = {}
    bodies: dict[str, list[int]] = {}
    spans: dict[str, dict[str, list[tuple[int, int]]]] = {}
    seen: set[str] = set()
    batch: list[tuple] = []
    fts: list[tuple[int, str]] = []
    ord_ = 0

    with Folio.create(folio_path) as folio:
        for chunk in build_chunks(archive, pack):
            if chunk.id in seen:
                # Identical text at an identical span. Real, and harmless once it is
                # counted rather than silently written twice into the vector store.
                rep.duplicates += 1
                continue
            seen.add(chunk.id)
            batch.append(chunk.row(ord_))
            fts.append((ord_, seg(chunk.embed_text)))
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
                batch, fts = [], []
                if progress is not None:
                    progress(f"chunks {ord_:,}")
        if batch:
            folio.add_chunks(batch)
            folio.add_fts(fts)
        rep.chunks = ord_

        _write_roster(archive, folio)
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
        rep.vector_bytes = {
            dim: rep.chunks * dim * 2 for dim in (1024, 512, 256, 128)
        }
        # Units are stored once, so a reader that wants surrounding context must fetch
        # it — the corpus does not carry lookbehind. Declaring the capability makes the
        # obligation explicit instead of leaving readers to discover truncated context.
        rep.requirements.append("neighbor_expand")
        if any(ratio > 0.005 for ratio in rep.redundancy.values()):
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
        folio.set_meta("source_url_pattern", pack.wiki.source_url)
        folio.set_meta("history_url_pattern", pack.wiki.history_url)
        folio.set_meta("source_revid_max", archive.scalar(
            "SELECT MAX(revid) FROM (SELECT revid FROM scenes UNION ALL "
            "SELECT revid FROM dossiers UNION ALL SELECT revid FROM lore)") or 0)
        folio.set_meta("built_at", dt.datetime.now(dt.UTC).isoformat(timespec="seconds"))
        folio.set_meta("build", run_info.as_dict())
        folio.set_meta("wording", dict(pack.wording))
        folio.set_meta("guard_tally", {g: list(v) for g, v in rep.ledger.tally().items()})
        folio.set_meta("guard_findings", [f.detail for f in rep.ledger.findings])
        # Written last: the fingerprint has to cover everything above it.
        folio.set_meta("build_fingerprint", _fingerprint(folio))
        folio.optimize()
    return rep


def _write_roster(archive: Archive, folio: Folio) -> None:
    """One row per person, with material volume and a confidence score.

    Confidence is the amount of source material, normalised — not a quality judgement.
    It is used downstream to *change behaviour rather than to apologise*: a person the
    archive barely covers should be written as terse and unwilling to speculate, which
    is a characterisation, whereas a generator told to produce a full personality from
    thin material will invent one.
    """
    person_pages = archive.persons()
    facet_source = {
        r["page"]: json.loads(r["fields"])
        for r in archive.db.execute("SELECT page,fields FROM dossiers")
    }
    material: dict[str, int] = {}
    for person_id, forms in person_pages.items():
        total = 0
        for f in forms:
            total += int(archive.scalar(
                "SELECT COALESCE(SUM(LENGTH(text)),0) FROM voices WHERE page=? OR subject=?",
                (f["page"], f["page"])) or 0)
            row = archive.db.execute(
                "SELECT sections,items FROM dossiers WHERE page=?", (f["page"],)).fetchone()
            if row is not None:
                total += len(row["sections"]) + len(row["items"])
        total += int(archive.scalar(
            "SELECT COALESCE(SUM(LENGTH(text)),0) FROM lines WHERE speaker=?",
            (person_id,)) or 0)
        material[person_id] = total

    ceiling = max(material.values()) or 1
    rows = []
    for person_id, forms in sorted(person_pages.items()):
        facets = facet_source.get(person_id, {})
        rows.append((
            person_id, person_id, facets.get("代号", person_id),
            json.dumps([{"page": f["page"], "kind": f["kind"]} for f in forms],
                       ensure_ascii=False),
            json.dumps(facets, ensure_ascii=False),
            material[person_id],
            round(min(material[person_id] / ceiling, 1.0) ** 0.5, 4),
        ))
    folio.write_persons(rows)


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
        if ratio <= 0.005:
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

    # A header that repeats a path segment wastes the reader's attention and the
    # encoder's budget on the same words twice. Found by sampling, not by a count:
    # 992 units read `情报处理室 › 情报处理室 › …`.
    if rep.repeated_header_segments:
        rep.ledger.add(
            "G5",
            f"{rep.repeated_header_segments} unit(s) have a header that repeats a path "
            "segment — the page title is already in the header, so the path should not "
            "carry it again",
            high=rep.repeated_header_segments > rep.chunks * 0.01,
        )


def _fingerprint(folio: Folio) -> str:
    """Hash of the chunk identifiers in order, plus the manifest that describes them.

    Content-derived rather than time-derived, so two builds of the same inputs
    fingerprint identically — which is what makes it usable for the client's
    lineage check and for reproducibility claims.
    """
    import hashlib

    h = hashlib.sha256()
    for (cid,) in folio.db.execute("SELECT id FROM chunks ORDER BY ord"):
        h.update(cid.encode())
    for key, value in sorted(folio.manifest().items()):
        if key == "build_fingerprint":
            continue
        h.update(f"{key}={json.dumps(value, sort_keys=True, ensure_ascii=False)}".encode())
    return f"sha256:{h.hexdigest()}"
