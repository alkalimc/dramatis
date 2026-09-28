"""The `forge persona` command body, with its dependencies injectable.

`execute` takes the key lookup and the HTTP transport as arguments, so a test can drive a
whole run (material, request, reply, gates, rows, reports) with no keychain and no
network. The CLI passes neither and gets the real ones.
"""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import httpx

from ..config import Paths
from ..pack import Pack, pack_dir
from ..report.tokens import load_counter
from . import generator as generator_mod
from . import report as report_mod
from .client import Client, request
from .endpoints import Endpoints, EndpointsError, KeyLookup, persona_target
from .material import HOST, estimate_tokens
from .params import Persona
from .run import Context, Report, ineligible, probe_set, resolve_subject, roster, run
from .store import RawCache


class UsageError(Exception):
    def __init__(self, message: str, hint: str = "") -> None:
        super().__init__(message)
        self.hint = hint


@dataclass
class Result:
    report: Report
    written: list[Path]


def counter_for(pack: Pack) -> tuple[Callable[[str], int], str]:
    """The pack's reference tokenizer, else the documented estimate."""
    count = load_counter(pack.tokenizer, pack_dir(pack.name)) if pack.tokenizer else None
    if count is not None:
        return count, pack.tokenizer
    return estimate_tokens, "estimate (1 per wide char, 1 per 4 others)"


def execute(
    pack: Pack,
    paths: Paths,
    *,
    probe: bool = False,
    only: list[str] | None = None,
    force: bool = False,
    endpoints_path: Path | None = None,
    params_path: Path | None = None,
    lookup: KeyLookup | None = None,
    transport: httpx.BaseTransport | None = None,
    counter: Callable[[str], int] | None = None,
    generator: generator_mod.Generator | None = None,
    progress: Callable[[str], None] | None = None,
) -> Result:
    if not paths.folio.exists():
        raise UsageError(f"no folio at {paths.folio}", "run `forge build` first")
    try:
        endpoints = Endpoints.load(endpoints_path)
    except EndpointsError as exc:
        raise UsageError(str(exc), exc.hint) from exc
    role = endpoints.roles.persona if endpoints.roles else None
    if role is None:
        raise UsageError("the endpoints file defines no `persona` role",
                         'add under [roles]: persona = { profile = "<name>", '
                         'model = "<id>", reasoning = "high" }')
    params = Persona.load(params_path if params_path is not None else paths.home / "params.toml")
    gen = generator or generator_mod.load(pack)
    placeholder = user_placeholder(paths.folio)
    version = gen.version(model=role.model, reasoning=role.reasoning, params=params,
                          placeholder=placeholder)
    if counter is not None:
        count, counter_name = counter, "injected"
    else:
        count, counter_name = counter_for(pack)

    def connect() -> Client:
        # The keychain is read only when a call is actually needed: a run that resumes
        # every subject needs no key.
        try:
            target = persona_target(endpoints, lookup)
        except EndpointsError as exc:
            raise UsageError(str(exc), exc.hint) from exc
        return Client(target, retries=params.retries, transport=transport)

    profile = endpoints.profile(role.profile)
    assert profile is not None  # checked when parsing
    root = paths.pack_dir / "persona"
    db = sqlite3.connect(paths.folio)
    try:
        subjects = _subjects(db, gen, params, probe=probe, only=only)
        ctx = Context(db=db, gen=gen, params=params, version=version, count=count,
                      sep=pack.chunking.sep("label"), cache=RawCache(root / "raw"),
                      connect=connect, placeholder=placeholder, force=force,
                      progress=progress)
        rep = run(ctx, subjects, counter_name)
        written = report_mod.write(rep, root, probe=probe)
        if probe:
            for o in rep.outcomes:
                schema = gen.schema(o.subject == HOST)
                written += report_mod.write_probe(
                    o, root / "probe", db=db,
                    render=lambda system, user, schema=schema: request(
                        profile.wire_api, role, system, user, schema))
    finally:
        db.close()
    return Result(report=rep, written=written)


def user_placeholder(folio: Path) -> str:
    """The corpus's user-name placeholder from the folio manifest; empty when it has none."""
    db = sqlite3.connect(f"file:{folio}?mode=ro", uri=True)
    try:
        row = db.execute("SELECT value FROM manifest WHERE key='user_placeholder'").fetchone()
    finally:
        db.close()
    value = json.loads(row[0]) if row else ""
    return value if isinstance(value, str) else ""


def _subjects(db: sqlite3.Connection, gen: generator_mod.Generator, params: Persona, *,
              probe: bool, only: list[str] | None) -> list[str]:
    if only:
        skip = ineligible(db, params)
        picked = []
        for name in only:
            subject = resolve_subject(db, name)
            if subject is None:
                raise UsageError(f"no person named {name!r}",
                                 "use a person id, a display name, an alias, or `host`")
            if subject == HOST and not gen.host_system:
                raise UsageError("the pack's generator does not generate the host")
            if subject in skip:
                raise UsageError(f"{subject} gets no persona: {skip[subject]}",
                                 "a persona is written only from a person's own words; "
                                 "set persona.min_own_units in params.toml to change the rule")
            if subject not in picked:
                picked.append(subject)
        return picked
    if probe:
        return probe_set(db, params)
    return roster(db) + ([HOST] if gen.host_system and gen.host is not None else [])
