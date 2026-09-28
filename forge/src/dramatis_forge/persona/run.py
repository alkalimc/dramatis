"""One `forge persona` run: choose subjects, resume, call, gate, write, report.

Resume rule, per subject, in order:

1. Rows in the folio carry the current generator version and still pass the gates:
   skip. (Gates are re-checked because the lexicons may have been tightened.)
2. The raw cache holds a reply for the current version: gate it and write or drop the
   rows, with no call. `forge build` recreates the folio, so this restores a whole run
   for free.
3. Otherwise one call. A person with no material is not called at all: a generator
   given nothing will invent someone.

`--force` skips 1 and 2. A call that fails for good leaves the folio untouched and is
reported; a reply that fails a gate removes the subject's rows, so the roster shows the
person as missing rather than an older persona.

Figures (`persona.meta_leak`, `prescriptive.rate`) are measured over every subject's
current output, written or not: over written rows alone they would be zero by
construction. They are written to the run report beside the folio, not into the
folio's manifest, which the build fingerprint covers.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from . import gates, store
from .client import CallError, Client, Usage
from .generator import Generator, fill
from .material import HOST, Counter, Material, for_host, for_person
from .params import Persona

PROBE_THICK = 3
PROBE_THIN = 2

WRITTEN, RESUMED, RESTORED, FAILED, ERROR, EMPTY = (
    "written", "resumed", "restored", "failed", "error", "no material")


@dataclass
class Outcome:
    subject: str
    status: str
    reasons: list[str] = field(default_factory=list)
    output: dict[str, Any] | None = None
    verdict: gates.Verdict | None = None
    material: Material | None = None
    usage: Usage = field(default_factory=Usage)
    attempts: int = 0
    raw_path: Path | None = None
    prompt: tuple[str, str] | None = None


@dataclass
class Report:
    version: str
    counter: str
    placeholder: str = ""
    outcomes: list[Outcome] = field(default_factory=list)
    #: Roster persons with no rows after the run, whatever the reason.
    missing: list[str] = field(default_factory=list)

    def by_status(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for o in self.outcomes:
            out[o.status] = out.get(o.status, 0) + 1
        return out

    @property
    def judged(self) -> list[Outcome]:
        return [o for o in self.outcomes if o.verdict is not None]

    @property
    def meta_leak(self) -> int:
        return sum(o.verdict.count(gates.META) for o in self.judged if o.verdict)

    @property
    def prescriptive_rate(self) -> float:
        judged = self.judged
        if not judged:
            return 0.0
        hit = sum(1 for o in judged if o.verdict and o.verdict.count(gates.PRESCRIPTIVE))
        return round(hit / len(judged), 4)

    @property
    def usage(self) -> Usage:
        total = Usage()
        for o in self.outcomes:
            total = total + o.usage
        return total

    def figures(self) -> dict[str, Any]:
        """The run's measured keys, as written to the report JSON."""
        return {"generator_version": self.version, "meta_leak": self.meta_leak,
                "prescriptive_rate": self.prescriptive_rate, "judged": len(self.judged),
                "missing": len(self.missing), "counter": self.counter}


# --------------------------------------------------------------------------- #
# Choosing subjects
# --------------------------------------------------------------------------- #


def roster(db: sqlite3.Connection) -> list[str]:
    return [r[0] for r in db.execute("SELECT person_id FROM persons ORDER BY person_id")]


def resolve_subject(db: sqlite3.Connection, name: str) -> str | None:
    """A person id, a display name or an alias; `host` names the host."""
    if name == HOST:
        return HOST
    row = db.execute("SELECT person_id FROM persons WHERE person_id=? OR display=? "
                     "ORDER BY person_id=? DESC LIMIT 1", (name, name, name)).fetchone()
    if row:
        return row[0]
    row = db.execute("SELECT a.target FROM aliases a JOIN persons p ON p.person_id=a.target "
                     "WHERE a.alias=? AND a.kind<>'disambig' LIMIT 1", (name,)).fetchone()
    return row[0] if row else None


def probe_set(db: sqlite3.Connection) -> list[str]:
    """The thickest three by material, then the thinnest two by `persona_confidence`
    among persons who have any material. The thin end is where prescriptive phrasing
    shows up first: the less a generator has, the more it reaches for "you talk little".
    """
    thick = [r[0] for r in db.execute(
        "SELECT person_id FROM persons ORDER BY material DESC, person_id LIMIT ?",
        (PROBE_THICK,))]
    thin = [r[0] for r in db.execute(
        "SELECT person_id FROM persons WHERE material > 0 AND person_id NOT IN "
        f"({','.join('?' * len(thick))}) ORDER BY persona_confidence, material, person_id "
        "LIMIT ?", (*thick, PROBE_THIN))]
    return thick + thin


# --------------------------------------------------------------------------- #
# Running
# --------------------------------------------------------------------------- #


@dataclass
class Context:
    db: sqlite3.Connection
    gen: Generator
    params: Persona
    version: str
    count: Counter
    sep: str
    cache: store.RawCache
    connect: Callable[[], Client]
    #: The corpus's user-name placeholder (manifest `user_placeholder`), or empty.
    placeholder: str = ""
    force: bool = False
    progress: Callable[[str], None] | None = None
    _client: Client | None = None

    def client(self) -> Client:
        if self._client is None:
            self._client = self.connect()
        return self._client

    def close(self) -> None:
        if self._client is not None:
            self._client.close()

    def say(self, message: str) -> None:
        if self.progress is not None:
            self.progress(message)


def prompts_for(ctx: Context, mat: Material, host: bool) -> tuple[str, str]:
    values: dict[str, object] = {
        "name": mat.display, "forms": ctx.sep.join(mat.forms) if host else " / ".join(mat.forms),
        "material": mat.render(ctx.gen, ctx.sep), **ctx.params.prompt_values(),
        "user_rule": ctx.gen.user_line(ctx.placeholder), "user_placeholder": ctx.placeholder,
    }
    system = ctx.gen.host_system if host else ctx.gen.system
    user = (ctx.gen.host_user or ctx.gen.user) if host else ctx.gen.user
    return fill(system, values), fill(user, values)


def _material(ctx: Context, subject: str) -> Material:
    if subject == HOST:
        assert ctx.gen.host is not None
        return for_host(ctx.db, ctx.gen.host, ctx.gen, ctx.params, ctx.count, ctx.sep)
    return for_person(ctx.db, subject, ctx.gen, ctx.params, ctx.count, ctx.sep)


def one(ctx: Context, subject: str, lex: gates.Lexicons) -> Outcome:
    host = subject == HOST
    fields = ctx.gen.fields(host)

    def judge(output: dict[str, Any]) -> gates.Verdict:
        return gates.check(output, host=host, gen=ctx.gen, lex=lex, p=ctx.params,
                           count=ctx.count)

    if not ctx.force:
        stored = store.read(ctx.db, subject, fields)
        if stored is not None and stored[0] == ctx.version:
            verdict = judge(stored[1])
            if verdict.passed:
                return Outcome(subject, RESUMED, output=stored[1], verdict=verdict,
                               raw_path=_existing(ctx.cache.path(subject)))
        cached = ctx.cache.load(subject, ctx.version)
        if cached is not None:
            return _settle(ctx, Outcome(subject, RESTORED, output=cached,
                                        raw_path=ctx.cache.path(subject)), judge, fields)

    mat = _material(ctx, subject)
    out = Outcome(subject, WRITTEN, material=mat)
    if mat.empty:
        out.status, out.reasons = EMPTY, ["no material to generate from"]
        return out
    system, user = out.prompt = prompts_for(ctx, mat, host)
    ctx.say(f"{subject}: calling ({mat.tokens:,} material tokens)")
    try:
        reply = ctx.client().call(system, user, ctx.gen.schema(host))
    except CallError as exc:
        out.status, out.reasons, out.usage = ERROR, [f"call: {exc}"], exc.usage
        return out
    out.output, out.attempts = reply.output, reply.attempts
    out.usage = reply.usage + reply.wasted
    out.raw_path = ctx.cache.save(subject, ctx.version, reply.output, reply.raw,
                                  out.usage.as_dict())
    return _settle(ctx, out, judge, fields)


def _existing(path: Path) -> Path | None:
    return path if path.exists() else None


def _settle(ctx: Context, out: Outcome, judge: Callable[[dict], gates.Verdict],
            fields: tuple[str, ...]) -> Outcome:
    assert out.output is not None
    out.verdict = judge(out.output)
    if out.verdict.passed:
        store.write(ctx.db, out.subject, out.output, fields, ctx.version)
    else:
        store.remove(ctx.db, out.subject, fields)
        out.status, out.reasons = FAILED, out.verdict.reasons()
    return out


def run(ctx: Context, subjects: list[str], counter_name: str) -> Report:
    rep = Report(version=ctx.version, counter=counter_name, placeholder=ctx.placeholder)
    lex = gates.Lexicons(ctx.gen, ctx.placeholder)
    try:
        for i, subject in enumerate(subjects, 1):
            out = one(ctx, subject, lex)
            rep.outcomes.append(out)
            ctx.say(f"[{i}/{len(subjects)}] {subject}: {out.status}"
                    + (f" — {out.reasons[0]}" if out.reasons else ""))
    finally:
        ctx.close()
    have = {r[0] for r in ctx.db.execute("SELECT DISTINCT subject FROM prompts WHERE slot='system'")}
    rep.missing = [p for p in roster(ctx.db) if p not in have]
    return rep
