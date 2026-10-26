"""One structured-output call for `forge persona`, over any wire the endpoints file names.

The protocols live in `dramatis_forge.llm`; this module adds what the generator needs on
top: a fixed output format (one offline call per person, not a cached session), output
that must parse, and the bookkeeping of retried attempts.

Transport failures, rate limits, server errors, unfinished replies and unparseable output
are retried up to `persona.retries` attempts with exponential backoff; any other client
error is final. The transport is injectable so tests never touch the network.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from time import sleep
from typing import Any

import httpx

from .. import llm
from .endpoints import Role, Target


@dataclass(frozen=True)
class Usage:
    #: All input tokens, cached ones included.
    input: int = 0
    cached: int = 0
    output: int = 0
    reasoning: int = 0

    def __add__(self, other: Usage) -> Usage:
        return Usage(self.input + other.input, self.cached + other.cached,
                     self.output + other.output, self.reasoning + other.reasoning)

    def as_dict(self) -> dict[str, int]:
        return {"input": self.input, "cached": self.cached, "output": self.output,
                "reasoning": self.reasoning}


def _usage(u: llm.Usage) -> Usage:
    return Usage(u.total_in, u.cached_in, u.output, u.reasoning)


class CallError(Exception):
    """A call that failed for good. `raw` is the last response body, if any; `usage` is
    what the failed attempts were billed."""

    def __init__(self, message: str, raw: Any = None, usage: Usage | None = None) -> None:
        super().__init__(message)
        self.raw = raw
        self.usage = usage or Usage()


class _Transient(Exception):
    def __init__(self, message: str, raw: Any = None) -> None:
        super().__init__(message)
        self.raw = raw


@dataclass
class Reply:
    output: dict[str, Any]
    raw: dict[str, Any]
    usage: Usage
    attempts: int = 1
    #: Usage of attempts that were retried, which are billed all the same.
    wasted: Usage = field(default_factory=Usage)


def usage_of(raw: Any, wire: str) -> Usage:
    return _usage(llm.usage_of(wire, raw))


def parse_output(text: str) -> dict[str, Any]:
    return llm.parse_json(text)


def llm_request(system: str, user: str, schema: dict, name: str, role: Role) -> llm.Request:
    return llm.Request(system=system, messages=(llm.Message("user", user),),
                       output_schema=schema, effort=role.reasoning, schema_name=name)


def request(wire: str, role: Role, system: str, user: str, schema: dict,
            name: str = "persona") -> tuple[str, dict]:
    """(path, JSON body) of one call. Pure, so a probe can record exactly what is sent."""
    req = llm_request(system, user, schema, name, role)
    if wire == "messages":
        return "messages", llm.messages_kwargs(role.model, req)
    return llm.openai_body(wire, role.model, req)


class Client:
    def __init__(self, target: Target, *, retries: int,
                 transport: httpx.BaseTransport | None = None,
                 wait: Callable[[Any], float] | None = None) -> None:
        self.target = target
        self.retries = max(1, retries)
        self.wait = wait
        self.transport = transport
        self.endpoint = llm.Endpoint(target.profile.wire_api, target.profile.base_url,
                                     target.role.model, target.key)

    def close(self) -> None:
        """Nothing is held open between calls; kept so callers can `with` a client."""

    def __enter__(self) -> Client:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    @property
    def wire(self) -> str:
        return self.target.profile.wire_api

    def body(self, system: str, user: str, schema: dict, name: str) -> tuple[str, dict]:
        return request(self.wire, self.target.role, system, user, schema, name)

    def call(self, system: str, user: str, schema: dict, name: str = "persona") -> Reply:
        req = llm_request(system, user, schema, name, self.target.role)
        wasted = Usage()
        attempts = 0
        last: Exception | None = None
        while attempts < self.retries:
            if attempts:
                sleep((self.wait or llm.backoff)(attempts))
            try:
                resp = llm.complete(self.endpoint, req, transport=self.transport,
                                    retries=self.retries - attempts, wait=self.wait)
            except llm.LlmError as exc:
                attempts += exc.attempts
                if not exc.retryable:
                    raise CallError(str(exc), exc.raw, wasted) from exc
                last = _Transient(exc.reason, exc.raw)
                break  # the retries were used up inside `complete`
            attempts += resp.attempts
            usage = _usage(resp.usage)
            if resp.stop == llm.REFUSAL:  # final, billed all the same
                raise CallError(f"refused: {resp.text}", resp.raw, wasted + usage)
            try:
                if resp.stop != llm.END_TURN:
                    raise _Transient(f"response not completed: {resp.stop}", resp.raw)
                output = parse_output(resp.text)
            except _Transient as exc:
                wasted, last = wasted + usage, exc
                continue
            except ValueError as exc:
                wasted, last = wasted + usage, _Transient(f"unparseable output: {exc}", resp.raw)
                continue
            return Reply(output=output, raw=resp.raw, usage=usage, attempts=attempts,
                         wasted=wasted)
        raise CallError(f"gave up after {attempts} attempt(s): {last}",
                        getattr(last, "raw", None), wasted) from last
