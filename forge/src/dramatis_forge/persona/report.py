"""What a run leaves for a person to read.

`<pack dir>/persona/report.json` and `REPORT.md`: every subject's status and, for a
subject that failed, the reasons; the measured keys `persona.meta_leak` and
`prescriptive.rate`; the roster persons still missing.

`--probe` also writes `<pack dir>/persona/probe/<subject>.md` (material summary, the
generated slots, gate results) and `<subject>.json` (the request as sent, minus the key,
and the reply as received), so the generator can be read sentence by sentence before a
full run.
"""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Callable
from pathlib import Path
from typing import Any

from .gates import Verdict
from .material import KINDS, Material
from .run import Outcome, Report
from .store import safe_name


def keys(rep: Report) -> dict[str, Any]:
    return {"persona.meta_leak": rep.meta_leak, "prescriptive.rate": rep.prescriptive_rate}


def write(rep: Report, root: Path, *, probe: bool) -> list[Path]:
    root.mkdir(parents=True, exist_ok=True)
    data = {
        "generator_version": rep.version,
        "counter": rep.counter,
        "user_placeholder": rep.placeholder,
        "probe": probe,
        "figures": keys(rep),
        "status": rep.by_status(),
        "usage": rep.usage.as_dict(),
        "missing": rep.missing,
        "subjects": [{"subject": o.subject, "status": o.status, "reasons": o.reasons,
                      "attempts": o.attempts, "usage": o.usage.as_dict()}
                     for o in rep.outcomes],
    }
    stem = "probe-report" if probe else "report"
    json_path = root / f"{stem}.json"
    json_path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    md_path = root / f"{stem.upper().replace('-', '_')}.md"
    md_path.write_text(_markdown(rep, probe), encoding="utf-8")
    return [md_path, json_path]


def _markdown(rep: Report, probe: bool) -> str:
    u = rep.usage
    out = [
        f"# forge persona{' --probe' if probe else ''}",
        "",
        f"- generator version: `{rep.version}`",
        f"- token counter: {rep.counter}",
        f"- user-name placeholder: {('`' + rep.placeholder + '`') if rep.placeholder else 'none'}",
        f"- `persona.meta_leak`: {rep.meta_leak}",
        f"- `prescriptive.rate`: {rep.prescriptive_rate}",
        "- subjects: " + " · ".join(f"{k} {v}" for k, v in sorted(rep.by_status().items())),
        f"- usage: input {u.input:,} (cached {u.cached:,}) · output {u.output:,} "
        f"(reasoning {u.reasoning:,})",
        f"- roster persons without a persona: {len(rep.missing)}",
        "",
    ]
    failed = [o for o in rep.outcomes if o.reasons]
    if failed:
        out += ["## Not written", "", "| subject | status | reasons |", "| --- | --- | --- |"]
        for o in failed:
            reasons = "<br>".join(r.replace("|", "\\|") for r in o.reasons)
            out.append(f"| {o.subject} | {o.status} | {reasons} |")
        out.append("")
    return "\n".join(out)


def write_probe(outcome: Outcome, root: Path, *, db: sqlite3.Connection,
                render: Callable[[str, str], tuple[str, dict]]) -> list[Path]:
    root.mkdir(parents=True, exist_ok=True)
    stem = safe_name(outcome.subject)
    md = root / f"{stem}.md"
    md.write_text(_probe_markdown(outcome, db), encoding="utf-8")
    raw: dict[str, Any] = {"subject": outcome.subject, "status": outcome.status,
                           "reasons": outcome.reasons}
    if outcome.prompt is not None:
        path, body = render(*outcome.prompt)
        raw["request"] = {"path": path, "body": body}
    if outcome.raw_path is not None and outcome.raw_path.exists():
        cached = json.loads(outcome.raw_path.read_text(encoding="utf-8"))
        raw["response"] = cached.get("response")
        raw["usage"] = cached.get("usage")
    raw["output"] = outcome.output
    js = root / f"{stem}.json"
    js.write_text(json.dumps(raw, ensure_ascii=False, indent=2), encoding="utf-8")
    return [md, js]


def _summary(mat: Material) -> list[str]:
    counts = mat.counts()
    shown = " · ".join(f"{k} {counts[k]}" for k in (*KINDS, "story") if k in counts)
    lines = [f"- forms: {' / '.join(mat.forms)}",
             f"- material sent: {mat.tokens:,} tokens — {shown or 'nothing'}",
             f"- story lines: {len(mat.story):,} sampled of {mat.story_found:,}"]
    if mat.dropped:
        lines.append("- left out for budget: "
                     + " · ".join(f"{k} {v}" for k, v in sorted(mat.dropped.items())))
    return lines


def _gates(v: Verdict | None) -> list[str]:
    if v is None:
        return ["not judged"]
    lines = [f"- {'pass' if v.passed else 'FAIL'}"]
    lines += [f"- {name} tokens: {n}" for name, n in v.tokens.items()]
    lines += [f"- {r}" for r in v.reasons()]
    return lines


def _probe_markdown(o: Outcome, db: sqlite3.Connection) -> str:
    out = [f"# {o.subject}", "", f"status: **{o.status}**", ""]
    row = db.execute("SELECT material, persona_confidence FROM persons WHERE person_id=?",
                     (o.subject,)).fetchone()
    out += ["## Inputs", ""]
    if row is not None:
        out.append(f"- roster material: {row[0]:,} chars · persona_confidence {row[1]}")
    if o.material is not None:
        out += _summary(o.material)
    else:
        out.append("- (not recomputed: output resumed from an earlier run)")
    out += ["", "## Outputs", ""]
    if o.output:
        for name, value in o.output.items():
            out.append(f"### {name}")
            out.append("")
            if isinstance(value, list):
                out += [f"{i}. {v}" for i, v in enumerate(value, 1)]
            else:
                out.append(str(value))
            out.append("")
    else:
        out += ["(none)", ""]
    out += ["## Gates", "", *_gates(o.verdict), ""]
    if o.reasons and o.verdict is None:
        out += [f"- {r}" for r in o.reasons] + [""]
    if o.usage.input or o.usage.output:
        u = o.usage
        out += ["## Usage", "", f"- attempts {o.attempts} · input {u.input:,} (cached "
                f"{u.cached:,}) · output {u.output:,} (reasoning {u.reasoning:,})", ""]
    return "\n".join(out)
