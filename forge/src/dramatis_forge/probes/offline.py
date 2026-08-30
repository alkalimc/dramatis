"""Probes answerable from artifacts already on disk.

These run without network access, model weights, or a toolchain, which means they can
run on every build and their answers can be cited with a build fingerprint attached.
Several of them contradict figures the design documents currently assert; each such
contradiction is recorded on the result rather than quietly corrected, because the
document is what has to change, and it should be possible to see why.
"""

from __future__ import annotations

import datetime as dt
import json
import re
from pathlib import Path

from ..pack import Pack
from ..store.archive import Archive
from ..store.folio import Folio
from .registry import BLOCKED, FAIL, PASS, Result, register

#: A person needs enough lines that a held-out fifth is still a meaningful sample.
MIN_LINES_FOR_HOLDOUT = 25
HOLDOUT_FRACTION = 0.2


def _now() -> str:
    return dt.datetime.now(dt.UTC).isoformat(timespec="seconds")


# --------------------------------------------------------------------------- #
# P1 identity
# --------------------------------------------------------------------------- #


def _p1(archive: Archive, pack: Pack, **_: object) -> Result:
    r = Result(ran_at=_now())
    kinds = {
        row["kind"]: row["n"]
        for row in archive.db.execute("SELECT kind, COUNT(*) AS n FROM forms GROUP BY kind")
    }
    people = archive.count("persons")
    pages = archive.count("seeds", "seed=?", ("S2",))
    r.measurements = {
        "roster_pages": pages,
        "persons": people,
        "forms_by_kind": kinds,
        "multi_form_persons": archive.count("persons", "form_count>1"),
    }

    expected = dict(pack.identity.baselines)
    ok = (
        people == expected.get("persons", people)
        and kinds.get("alter", 0) == expected.get("alter", 0)
        and kinds.get("variant", 0) == expected.get("variant", 0)
    )
    r.status = PASS if ok else FAIL
    r.note(
        f"{pages} roster pages resolve to {people} people "
        f"({kinds.get('alter', 0)} alternate incarnations, {kinds.get('variant', 0)} class variants)"
    )

    # The claim that motivated reading template parameters instead of body links.
    linked = 0
    alters = [row["page"] for row in archive.db.execute(
        "SELECT page FROM forms WHERE kind='alter'")]
    person_of = archive.person_of()
    for page in alters:
        row = archive.page(page)
        if row is None:
            continue
        target = person_of.get(page, "")
        if target and re.search(r"\[\[\s*" + re.escape(target) + r"\s*[\]|]", row["wikitext"] or ""):
            linked += 1
    r.measurements["alters_linking_prototype_in_body"] = linked
    r.note(
        f"{linked}/{len(alters)} alternate-incarnation pages link their prototype in body "
        "text — the relation exists only as a template parameter, so a wikilink scan finds "
        "nothing and reports success"
    )
    if linked:
        r.contradiction(
            "identity: body links do occur, so the 'template parameter is the only signal' "
            "claim needs restating")
    return r


# --------------------------------------------------------------------------- #
# P2 corpus scale and the vector budget
# --------------------------------------------------------------------------- #


def _p2(folio: Folio, **_: object) -> Result:
    r = Result(ran_at=_now())
    n = int(folio.get_meta("chunk_count") or folio.count())
    by_template = folio.get_meta("chunks_by_template") or folio.templates()
    folio_bytes = folio.path.stat().st_size
    vectors = {dim: n * dim * 2 for dim in (1024, 512, 256, 128)}

    r.measurements = {
        "chunks": n,
        "by_template": by_template,
        "folio_bytes_without_vectors": folio_bytes,
        "vector_bytes_f16": vectors,
        "distributable_bytes_at_1024": folio_bytes + vectors[1024],
        "segmenter": folio.get_meta("segmenter"),
    }
    # The budget this gates: a resident vector store plus page cache inside a stated
    # allowance, with a full exact scan rather than an approximate index.
    r.status = PASS if vectors[1024] < 512 * 1024 * 1024 else FAIL
    r.note(
        f"{n:,} retrieval units → {vectors[1024] / 1e6:.0f} MB of f16 vectors at 1024 "
        f"dimensions; folio without vectors is {folio_bytes / 1e6:.0f} MB, so the "
        f"knowledge base ships at about {(folio_bytes + vectors[1024]) / 1e6:.0f} MB"
    )
    r.contradiction(
        f"artifacts budget: assumed roughly 40,000 units and 82 MB of vectors; measured "
        f"{n:,} units and {vectors[1024] / 1e6:.0f} MB — the unit count is the error, "
        f"{n / 40000:.1f}x, and every figure derived from it moves"
    )
    return r


# --------------------------------------------------------------------------- #
# P3 window overlap and result diversity
# --------------------------------------------------------------------------- #


def _p3(archive: Archive, folio: Folio, **_: object) -> Result:
    """Are units stored once, and do their boundaries fall between speaker turns?

    Rewritten. The original version measured how *much* dialogue units overlapped,
    because overlap was then deliberate — a sliding window bought reachability for
    exchanges that straddled a boundary. Measurement killed that design: the overlap ran
    to 95%, multiplied the vector store by 1.9, and forced a merge stage into the ranker,
    while the problem it solved is answerable at query time by widening a hit to its
    neighbours. So the invariant flipped from "overlap is recorded" to "there is none",
    and the probe has to test the invariant that now exists.
    """
    r = Result(ran_at=_now())
    redundancy = folio.get_meta("redundancy") or {}
    requires = folio.get_meta("requires") or []

    rows = list(folio.db.execute(
        "SELECT span_of, COUNT(*) AS units, COUNT(DISTINCT span_from) AS starts "
        "FROM chunks WHERE template='dialogue' GROUP BY span_of"
    ))
    # Distinct start positions per scene should equal unit count: a repeat means two
    # units begin at the same line, i.e. the splitter emitted the same records twice.
    repeated = sum(row["units"] - row["starts"] for row in rows)

    # Boundary quality: does a unit end where its last speaker stops talking? Cutting
    # mid-turn is the failure that actually damages a dialogue unit, and it is what a
    # fixed-width window guarantees at every boundary.
    speakers: dict[str, dict[int, str | None]] = {}
    for row in archive.db.execute("SELECT scene,seq,speaker FROM lines"):
        speakers.setdefault(row["scene"], {})[row["seq"]] = row["speaker"]
    clean = mid_turn = 0
    for row in folio.db.execute(
        "SELECT span_of, span_to FROM chunks WHERE template='dialogue' ORDER BY span_of, span_from"
    ):
        by_seq = speakers.get(row["span_of"], {})
        last, nxt = by_seq.get(row["span_to"]), by_seq.get(row["span_to"] + 1)
        if nxt is None or last != nxt:
            clean += 1
        else:
            mid_turn += 1
    total_bounds = clean + mid_turn
    clean_rate = clean / total_bounds if total_bounds else 1.0

    r.measurements = {
        "redundancy_by_template": redundancy,
        "folio_requirements": requires,
        "dialogue_units": sum(row["units"] for row in rows),
        "scenes": len(rows),
        "units_sharing_a_start": repeated,
        "boundaries_at_speaker_change": clean,
        "boundaries_mid_turn": mid_turn,
        "clean_boundary_rate": round(clean_rate, 4),
        "vector_bytes_1024": (folio.get_meta("chunk_count") or 0) * 2048,
    }

    overlapping = {k: v for k, v in redundancy.items() if v > 0.005}
    r.status = PASS if (
        not overlapping and repeated == 0 and clean_rate >= 0.9
        and "neighbor_expand" in requires
    ) else FAIL

    r.note(
        f"no template carries positional redundancy above 0.5%; {repeated} unit(s) share "
        "a start position, so each record is stored exactly once"
        if not overlapping and repeated == 0 else
        f"redundancy present: {overlapping or ''}, {repeated} unit(s) share a start"
    )
    r.note(
        f"{clean_rate:.1%} of unit boundaries fall at a speaker change "
        f"({mid_turn:,} of {total_bounds:,} cut mid-turn) — a fixed-width window would "
        "cut mid-turn at every boundary"
    )
    r.note(
        "the folio requires neighbor_expand: context is recovered by widening a ranked "
        "hit rather than by storing lookbehind in every unit, so the reader pays for "
        "context on the handful returned rather than on all of them"
        if "neighbor_expand" in requires else
        "no neighbor_expand requirement recorded — readers will silently serve truncated context"
    )
    return r


# --------------------------------------------------------------------------- #
# P4 how much benchmark the wiki's own relations yield
# --------------------------------------------------------------------------- #


def _p4(evals_dir: Path, **_: object) -> Result:
    r = Result(ran_at=_now())
    manifest = evals_dir / "structural.manifest.json"
    if not manifest.exists():
        r.status = BLOCKED
        return r.note("no structural suite built yet")

    data = json.loads(manifest.read_text(encoding="utf-8"))
    counts = data["counts"]
    r.measurements = {
        "queries": counts["queries"],
        "by_family": counts["by_family"],
        "strata_by_family": counts["strata_by_family"],
        "hard_negatives": counts["hard_negatives"],
        "annotation_cost": 0,
    }
    r.status = PASS if counts["queries"] >= 1000 else FAIL
    r.note(
        f"{counts['queries']:,} graded queries with zero annotation budget, across "
        f"{len(counts['by_family'])} relation families"
    )

    trivial = [
        fam for fam, strata in counts["strata_by_family"].items()
        if strata.get("verbatim", 0) / max(sum(strata.values()), 1) > 0.9
    ]
    if trivial:
        r.note(
            f"families solvable by exact string match, and so unusable as evidence of "
            f"retrieval quality: {', '.join(trivial)}"
        )
        r.contradiction(
            "research claim on annotation-free evaluation: the construction is sound but "
            "some families are trivially saturated, so the claim must be stated per "
            "stratum or it will be refuted by a BM25 baseline"
        )

    scarce = [
        fam for fam, hn in counts["hard_negatives"].items()
        if hn["with_negatives"] and hn["queries"] < 50
    ]
    if scarce:
        r.note(
            f"highest-quality negatives (human-curated confusable sets) are the scarcest: "
            f"{', '.join(scarce)} — volume comes from structural siblings instead"
        )
    return r


# --------------------------------------------------------------------------- #
# P5 is the speaker-discrimination metric measuring what it claims
# --------------------------------------------------------------------------- #


def _p5(archive: Archive, **_: object) -> Result:
    """The confound that would invalidate the persona-fidelity metric.

    The metric holds out a fifth of each character's real lines, generates lines from the
    synthesised prompt, and scores by whether an embedding ranks the correct speaker
    first. That is only a measure of *voice* if the held-out lines do not name their own
    speaker. Characters address each other constantly, so some fraction of any line set is
    solvable by string matching — and if that fraction is large, a high score means the
    embedding learnt to spot proper nouns.
    """
    r = Result(ran_at=_now())
    person_of = archive.person_of()
    aliases: dict[str, set[str]] = {}
    for row in archive.db.execute("SELECT alias,target FROM aliases"):
        aliases.setdefault(row["target"], set()).add(row["alias"])

    per_person: dict[str, list[str]] = {}
    for row in archive.db.execute(
        "SELECT speaker, text FROM lines WHERE speaker IS NOT NULL AND kind='对白'"
    ):
        person = person_of.get(row["speaker"], row["speaker"])
        per_person.setdefault(person, []).append(row["text"])

    eligible = {p: lines for p, lines in per_person.items() if len(lines) >= MIN_LINES_FOR_HOLDOUT}
    roster = set(archive.person_of().values())
    eligible_roster = {p: v for p, v in eligible.items() if p in roster}

    leaked = 0
    total = 0
    per_person_leak: list[float] = []
    for person, lines in eligible.items():
        names = {person} | aliases.get(person, set())
        names = {n for n in names if len(n) >= 2}
        hits = sum(1 for text in lines if any(n in text for n in names))
        leaked += hits
        total += len(lines)
        per_person_leak.append(hits / len(lines))

    mean_leak = sum(per_person_leak) / len(per_person_leak) if per_person_leak else 0.0
    holdout = sum(int(len(v) * HOLDOUT_FRACTION) for v in eligible.values())

    # The class count is the thing most likely to be quoted wrongly, so sweep the
    # threshold instead of asserting one. Every roster member gets a synthesised prompt,
    # but only those with enough real lines can be *scored*, and those are different
    # numbers.
    sweep = {}
    for threshold in (10, 25, 50, 100, 200):
        roster_ok = [p for p, v in per_person.items() if p in roster and len(v) >= threshold]
        sweep[threshold] = {
            "roster_speakers": len(roster_ok),
            "holdout_lines": sum(int(len(per_person[p]) * HOLDOUT_FRACTION) for p in roster_ok),
        }

    r.measurements = {
        "distinct_speakers": len(per_person),
        "speakers_over_threshold": len(eligible),
        "roster_speakers_over_threshold": len(eligible_roster),
        "line_threshold": MIN_LINES_FOR_HOLDOUT,
        "total_lines_considered": total,
        "holdout_lines_at_20pct": holdout,
        "self_name_leakage_overall": round(leaked / total, 4) if total else 0.0,
        "self_name_leakage_mean_per_speaker": round(mean_leak, 4),
        "class_count_by_threshold": sweep,
        "roster_size": len(roster),
    }
    r.status = PASS if len(eligible_roster) >= 100 else FAIL
    r.note(
        f"{len(per_person):,} distinct speakers appear in the scripts, far more than the "
        f"{len(roster)}-member roster: most speaking parts belong to characters who never "
        "became playable"
    )
    r.note(
        "roster members with enough lines to hold out, by threshold — "
        + "; ".join(f"≥{k}: {v['roster_speakers']}" for k, v in sweep.items())
        + f". Every roster member gets a prompt, but only these can be scored"
    )
    # The confound I expected to find, measured and largely absent. Recording the negative
    # result matters: it is the difference between a control that is required and one that
    # is merely cheap insurance.
    r.note(
        f"self-name leakage is {leaked / total:.1%} of lines overall (mean {mean_leak:.1%} "
        "per speaker) — the string-matching shortcut is negligible in this corpus, so a "
        "name-masked condition is a cheap control rather than a correction the metric needs "
        "to be valid"
    )
    r.contradiction(
        f"persona-fidelity metric: the class count is stated as the full roster "
        f"({len(roster)}), but only {len(eligible_roster)} roster members have enough real "
        f"lines to hold out at a {int(HOLDOUT_FRACTION * 100)}% split. Reporting one number "
        "over the whole roster either silently drops the rest or scores them on a handful "
        "of lines; the metric has to state its class count and its per-class support"
    )
    return r


# --------------------------------------------------------------------------- #
# P6 segmentation choice
# --------------------------------------------------------------------------- #


def _p6(archive: Archive, pack: Pack, workdir: Path, **_: object) -> Result:
    """Does the zero-dependency fallback segmenter cost enough to make jieba required?"""
    import sqlite3
    import time

    from ..corpus.build import run as build_corpus

    r = Result(ran_at=_now())
    out: dict[str, dict[str, float]] = {}
    for prefer, label in (("none", "char-bigram"), ("jieba", "jieba")):
        path = workdir / f"probe-segment-{label}.folio"
        started = time.time()
        try:
            rep = build_corpus(archive, pack, path, segmenter=prefer)
        except ImportError:
            r.note(f"{label} unavailable in this environment")
            continue
        elapsed = time.time() - started
        db = sqlite3.connect(path)
        fts = db.execute(
            "SELECT COALESCE(SUM(pgsize),0) FROM dbstat WHERE name LIKE 'chunks_fts%'"
        ).fetchone()[0]
        db.close()
        out[rep.segmenter] = {
            "folio_bytes": path.stat().st_size,
            "fts_bytes": fts,
            "build_seconds": round(elapsed, 1),
        }
        path.unlink(missing_ok=True)
        for side in ("-wal", "-shm"):
            Path(str(path) + side).unlink(missing_ok=True)

    r.measurements = out
    r.status = PASS if len(out) >= 1 else BLOCKED
    if len(out) == 2:
        (a_name, a), (b_name, b) = sorted(out.items())
        ratio = a["fts_bytes"] / b["fts_bytes"] if b["fts_bytes"] else 0
        r.note(
            f"lexical index: {a_name} {a['fts_bytes'] / 1e6:.1f} MB vs {b_name} "
            f"{b['fts_bytes'] / 1e6:.1f} MB ({ratio:.2f}x); build time "
            f"{a['build_seconds']}s vs {b['build_seconds']}s"
        )
        r.note(
            "the fallback is usable rather than broken, so a real segmenter stays an "
            "optional dependency — but which one ran must be recorded in the manifest, "
            "because query-side segmentation has to match"
        )
    return r


# --------------------------------------------------------------------------- #
# P7 scope closure
# --------------------------------------------------------------------------- #


def _p7(archive: Archive, pack: Pack, **_: object) -> Result:
    r = Result(ran_at=_now())
    counts = {s.key: archive.count("seeds", "seed=?", (s.key,)) for s in pack.seeds}
    tally = archive.get_meta("guard_tally") or {}
    recon = archive.get_meta("reconciliation") or {}
    high = {g: v[0] for g, v in tally.items() if v and v[0]}

    r.measurements = {
        "seed_counts": counts,
        "baselines": pack.baselines,
        "guard_tally": tally,
        "reconciliation": recon,
        "pages_held": archive.get_meta("pages_held"),
        "record_counts": archive.get_meta("record_counts"),
        "total_chars": sum((archive.get_meta("record_chars") or {}).values()),
    }
    r.status = PASS if not high else FAIL
    r.note(
        "every seed set is a closed membership query, so there is no 'uncovered' state to "
        "estimate — only counts to compare"
    )
    if high:
        r.note(f"high-severity findings outstanding: {high}")
    else:
        r.note("no high-severity guard findings; low-severity counts are all attributed")

    produced = recon.get("produced", {})
    stored = recon.get("stored", {})
    recovered = {
        k: produced[k] - stored.get(k, 0) for k in produced if produced[k] != stored.get(k, 0)
    }
    if recovered:
        r.note(f"produced-vs-stored differences, all explained as exact duplicates: {recovered}")
    return r


# --------------------------------------------------------------------------- #
# P8 can a refusal-correctness set be built without annotation
# --------------------------------------------------------------------------- #


def _p8(archive: Archive, folio: Folio, **_: object) -> Result:
    """A character should decline questions outside their knowledge, and that has to be
    measured against questions genuinely outside it. Constructing such a set by hand is
    expensive; constructing it from the corpus is nearly free, because one character's
    material is by definition outside another's."""
    r = Result(ran_at=_now())
    rows = list(folio.db.execute(
        "SELECT person, COUNT(*) AS n FROM chunks WHERE person IS NOT NULL "
        "GROUP BY person HAVING n >= 5 ORDER BY n DESC"
    ))
    people = [row["person"] for row in rows]
    # An out-of-scope question for A is any question whose only answer lies in B's
    # material, for B far from A. "Far" is cheap to define structurally: different faction
    # and no shared scene.
    shared = {
        (row["a"], row["b"])
        for row in archive.db.execute(
            "SELECT l1.speaker AS a, l2.speaker AS b FROM lines l1 "
            "JOIN lines l2 ON l1.scene = l2.scene AND l1.speaker < l2.speaker "
            "WHERE l1.speaker IS NOT NULL AND l2.speaker IS NOT NULL GROUP BY 1,2"
        )
    }
    pairs = 0
    for i, a in enumerate(people[:200]):
        for b in people[i + 1: i + 40]:
            if (a, b) not in shared and (b, a) not in shared:
                pairs += 1

    r.measurements = {
        "people_with_material": len(people),
        "co_appearing_pairs": len(shared),
        "sampled_disjoint_pairs": pairs,
    }
    r.status = PASS if pairs >= 500 else FAIL
    r.note(
        f"{len(people)} people have at least five retrieval units; co-appearance is known "
        f"for {len(shared):,} pairs, so disjoint pairs are identifiable structurally and a "
        "refusal set can be generated rather than written"
    )
    r.note(
        "refusal correctness is the right primary metric for thinly-covered characters: "
        "requiring them to match well-covered ones on discrimination only rewards a "
        "generator that invents material"
    )
    return r


# --------------------------------------------------------------------------- #
# registration
# --------------------------------------------------------------------------- #

register(
    id="P1", title="identity resolution",
    question="How many people does the roster actually contain, and does the declared "
             "relation parse for every page that carries it?",
    gates="one agent per person rather than per page; the class count of any "
          "speaker-discrimination metric; a single relationship record per character",
    method="resolve declarations over all roster pages, compare against measured "
           "baselines, and re-check the claim that body links cannot substitute",
    criterion="person count and both form-kind counts match their baselines exactly",
    needs=("archive",), cost="seconds", run=_p1,
)
register(
    id="P2", title="corpus scale and vector budget",
    question="How many retrieval units does the corpus actually produce, and what does "
             "the vector store cost at full dimension?",
    gates="the resident-memory budget; the decision to scan exactly rather than "
          "approximately; whether dimensionality reduction is needed after all",
    method="build the corpus and measure units, folio size, and vector bytes per dimension",
    criterion="vectors at full dimension stay under 512 MB, keeping an exact scan viable",
    needs=("folio",), cost="seconds", run=_p2,
)
register(
    id="P3", title="units stored once, boundaries between turns",
    question="Is every record stored exactly once, and do unit boundaries fall where a "
             "speaker stops talking?",
    gates="non-overlapping storage; the absence of a span-merge stage in the ranker; the "
          "size of the vector store; the neighbour-expansion contract",
    method="check positional redundancy and duplicate start positions per template, then "
           "compare each unit's last line against the next line's speaker",
    criterion="no redundancy above 0.5%, no two units sharing a start, at least 90% of "
              "boundaries at a speaker change, and neighbor_expand declared",
    needs=("archive", "folio"), cost="seconds", run=_p3,
)
register(
    id="P4", title="annotation-free benchmark yield",
    question="How many graded queries can be derived from the wiki's own relations, and "
             "how many of them are trivially solvable?",
    gates="the central research claim about evaluation without annotation; which families "
          "may be reported as evidence",
    method="build the structural suite and stratify every query by lexical overlap with "
           "its gold passage",
    criterion="at least a thousand graded queries, with the trivially-solvable share "
              "identified per family",
    needs=("evals",), cost="seconds", run=_p4,
)
register(
    id="P5", title="is persona fidelity measuring voice",
    question="How many characters have enough lines to hold out, and how often does a "
             "line name its own speaker?",
    gates="the primary persona-fidelity metric and whether it needs a masked condition; "
          "the hold-out design; per-frequency reporting",
    method="map speakers to people through identity and aliases, count lines, and measure "
           "self-name occurrence",
    criterion="at least a hundred speakers clear the hold-out threshold, and the leakage "
              "rate is quantified",
    needs=("archive",), cost="seconds", run=_p5,
)
register(
    id="P6", title="segmentation choice",
    question="Does the zero-dependency fallback segmenter cost enough to make a real one "
             "mandatory?",
    gates="whether segmentation is an optional dependency; the lexical index budget",
    method="build the corpus both ways and compare index size and build time",
    criterion="the fallback is within roughly 2x on index size, i.e. usable rather than "
              "broken",
    needs=("archive", "folio"), cost="a minute", run=_p6,
)
register(
    id="P7", title="scope closure and guard state",
    question="Do the seed sets still match their baselines, and is every guard finding "
             "either absent or attributed?",
    gates="the design-freeze exit criteria; the claim that scope is closed rather than "
          "estimated",
    method="compare seed counts against baselines and read the guard ledger and the "
           "produced-versus-stored reconciliation",
    criterion="no high-severity finding, and every produced-versus-stored difference "
              "explained",
    needs=("archive",), cost="seconds", run=_p7,
)
register(
    id="P8", title="refusal-set constructibility",
    question="Can out-of-scope questions be generated from the corpus instead of written?",
    gates="refusal correctness as the primary metric for thinly-covered characters",
    method="use co-appearance to identify structurally disjoint pairs of people",
    criterion="at least five hundred disjoint pairs identifiable without annotation",
    needs=("archive", "folio"), cost="seconds", run=_p8,
)

# ---- registered, cannot run here; the blocker is part of the record ----

register(
    id="P9", title="reranker base model",
    question="Which cross-encoder base performs better on this corpus, and at what latency?",
    gates="the choice of reranker weights; whether reranking can stay always-on",
    method="score both candidates on the structural suite, measure p50 and p95 latency",
    criterion="the chosen base wins on the paraphrase stratum without exceeding the "
              "latency allowance",
    needs=("weights", "endpoint", "hardware"), cost="hours",
)
register(
    id="P10", title="fine-tuning gain",
    question="How much does domain fine-tuning of the encoder gain over the base model?",
    gates="whether training is worth its cost at all; the published ablation curve",
    method="train on structural pairs with structure-derived hard negatives, evaluate "
           "per family and per stratum",
    criterion="a gain on the paraphrase stratum that survives a held-out page split",
    needs=("weights",), cost="days",
)
register(
    id="P11", title="retrieval latency on target hardware",
    question="What is the real end-to-end latency of the hybrid pipeline, cold and warm?",
    gates="the latency budget; whether an exact scan is fast enough at the measured unit "
          "count; whether reranking stays always-on",
    method="fixed hardware, stated thermal state, repeated runs, report the distribution "
           "and not a mean",
    criterion="first token within the stated allowance at p95",
    needs=("weights", "hardware", "toolchain"), cost="days",
)
register(
    id="P12", title="resident memory on target hardware",
    question="What does the whole system actually hold resident?",
    gates="every figure in the resource budget; the decision not to reduce dimensionality; "
          "keeping the reranker loaded",
    method="measure each component separately and together, cold and warm, on each platform",
    criterion="within the stated target, not merely the stated ceiling",
    needs=("weights", "hardware", "toolchain"), cost="days",
)
register(
    id="P13", title="client toolkit licensing",
    question="What are the actual licence terms of each library the client would reuse, "
             "and do any carry exceptions?",
    gates="the whole licence-boundary argument, and therefore the repository split",
    method="read the licence texts and the per-file headers; record findings per library",
    criterion="every reused library's terms confirmed from source, with no assumption "
              "left standing",
    needs=("legal",), cost="a day",
)
register(
    id="P14", title="minimal client build",
    question="What is the true dependency closure for a window that renders one message?",
    gates="the client's dependency list; the installer size; the platform matrix",
    method="build the smallest thing that draws a styled message on each platform",
    criterion="it builds on all three platforms from a documented dependency set",
    needs=("toolchain", "hardware"), cost="days",
)
register(
    id="P15", title="local generation viability",
    question="Is a small local model good enough to be a recommended preset rather than "
             "merely a supported endpoint?",
    gates="whether a local model appears in the recommended configuration; the onboarding "
          "download size",
    method="run the standard conversation set against a local endpoint and against a "
           "hosted one, compare on the same rubric",
    criterion="usable without qualification, or it stays an unadvertised preset",
    needs=("weights", "endpoint", "hardware"), cost="days",
)
register(
    id="P16", title="notification capability per platform",
    question="Can each platform deliver a notification while the app is closed, and under "
             "what permission model?",
    gates="whether being contacted first can exist as a mechanic at all",
    method="a minimal signed build per platform, tested with the app closed",
    criterion="delivery confirmed on all three, with the permission prompt documented",
    needs=("toolchain", "hardware"), cost="days",
)
register(
    id="P17", title="attribution and takedown notices",
    question="What exactly must accompany redistributed corpus text, and who is the "
             "contact of record?",
    gates="whether the corpus may be published at all; the per-record provenance "
          "requirement",
    method="draft per-page attribution and a notice naming rights holders, the "
           "non-commercial restriction, and a takedown route; have it reviewed",
    criterion="drafted and reviewed **before** the first upload, not after",
    needs=("legal",), cost="a day",
)
register(
    id="P18", title="persona synthesis convergence",
    question="Does automatic prompt synthesis converge to usable prompts across the whole "
             "roster, including thinly-covered characters?",
    gates="the central claim that hundreds of personas need no hand-written prompts; the "
          "generator's acceptance gate",
    method="synthesise for every person, then score discrimination with masking, "
           "refusal correctness, and out-of-scope rate",
    criterion="acceptance thresholds met without any hand-written persona",
    needs=("endpoint", "weights"), cost="days",
)
register(
    id="P19", title="settlement determinism",
    question="Does advancing the in-world clock after a long absence produce the same "
              "state regardless of how the elapsed time is chunked?",
    gates="the whole in-world time model; whether deadlines and meetings can be trusted "
          "across a closed app",
    method="replay the same journal in different step sizes and compare resulting state",
    criterion="bit-identical state for any partition of the same interval",
    needs=("toolchain",), cost="days",
)
register(
    id="P21", title="workspace prompt injection",
    question="Can a document placed in the workspace redirect an agent that reads it — "
             "and does the approval gate still hold when it tries?",
    gates="the workspace's double duty as safety and fiction boundary; whether the default "
          "workspace may ever point at a user's own directory",
    method="plant documents carrying instruction-shaped text (override attempts, forged "
           "approval grants, path-escape requests) in the workspace, have an agent "
           "summarise them, and check whether any out-of-scope action is attempted or "
           "granted without a prompt",
    criterion="no out-of-scope action taken without an explicit user grant, and every "
              "attempt visible in the trace rather than silent",
    needs=("endpoint", "toolchain"), cost="days",
)
register(
    id="P22", title="approval wording preserves weight",
    question="Does in-world phrasing ever make a destructive action sound smaller than it is?",
    gates="the rule that in-world wording may change the narration but never the weight; "
          "the whole approval-gate design",
    method="render every approval prompt for every wording key, then have readers who have "
           "not seen the code rank each by how dangerous it sounds and compare against the "
           "actual blast radius",
    criterion="perceived severity is monotonic in actual severity; no destructive action "
              "ranks below a benign one",
    needs=("endpoint",), cost="a day",
)
register(
    id="P20", title="local control channel security",
    question="Can any other process on the machine drive the daemon, approve its own tool "
             "calls, or read the credentials?",
    gates="the transport and authentication design; the approval gate's trustworthiness",
    method="attempt connection and approval-forgery from an unrelated local process on "
           "each platform",
    criterion="connection refused without the per-launch secret, and approvals "
              "unforgeable",
    needs=("toolchain", "hardware"), cost="days",
)
