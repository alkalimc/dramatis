"""One structured-output call for `forge persona`, over any wire the endpoints file names.

The protocols live in `dramatis_forge.llm`; this module adds what the generator needs on
top: a fixed output format (one offline call per person, not a cached session), output
that must parse, and the bookkeeping of retried attempts.

Transport failures, server errors, unfinished replies and unparseable output are retried
up to `persona.retries` attempts with exponential backoff; any other client error is
final. HTTP 429 is different: it says "not now", not "this request is bad", so it is
waited out (the server's `Retry-After`, else 30 s doubling to 5 min) without using up an
attempt, until the call has waited `persona.rate_limit_wait_s` in all. The wait is held
by a gate shared by every thread using the client, so a pool backs off as a whole.
The transport, the clock and the sleep are injectable so tests never touch the network.
"""

from __future__ import annotations

import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
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


#: Wait after the first 429 without a `Retry-After`, doubling up to the cap, in seconds.
RATE_WAIT_FIRST = 30.0
RATE_WAIT_CAP = 300.0


class Gate:
    """A pause every thread of a run honours: a 429 for one call holds back all of them."""

    def __init__(self, clock: Callable[[], float] | None = None,
                 sleep: Callable[[float], None] | None = None) -> None:
        self.clock = clock or time.monotonic
        self.sleep = sleep or time.sleep
        self._lock = threading.Lock()
        self._until = 0.0

    def hold(self, seconds: float) -> None:
        with self._lock:
            self._until = max(self._until, self.clock() + seconds)

    def wait(self) -> None:
        while True:
            with self._lock:
                left = self._until - self.clock()
            if left <= 0:
                return
            self.sleep(left)


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
                 wait: Callable[[Any], float] | None = None,
                 rate_limit_wait_s: float = 0.0, gate: Gate | None = None,
                 clock: Callable[[], float] | None = None,
                 sleep: Callable[[float], None] | None = None) -> None:
        self.target = target
        self.retries = max(1, retries)
        self.rate_limit_wait_s = rate_limit_wait_s
        self.gate = gate or Gate(clock, sleep)
        self.clock, self.sleep = self.gate.clock, self.gate.sleep
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
        limited = 0  # 429s met by this call
        first_limit = 0.0
        pause = False  # whether a failed attempt is to be backed off from
        while attempts < self.retries:
            if pause:
                self.sleep((self.wait or llm.backoff)(attempts))
            self.gate.wait()
            try:
                # One attempt at a time: attempts and rate-limit waits are counted here.
                resp = llm.complete(self.endpoint, req, transport=self.transport, retries=1)
            except llm.LlmError as exc:
                if exc.status == 429:
                    now = self.clock()
                    if not limited:
                        first_limit = now
                    limited += 1
                    delay = exc.retry_after if exc.retry_after is not None else min(
                        RATE_WAIT_CAP, RATE_WAIT_FIRST * 2 ** (limited - 1))
                    if now - first_limit + delay > self.rate_limit_wait_s:
                        raise CallError(
                            f"rate limited (HTTP 429) for {now - first_limit:.0f} s, past "
                            f"persona.rate_limit_wait_s {self.rate_limit_wait_s:.0f}: "
                            f"{exc.reason}", exc.raw, wasted) from exc
                    self.gate.hold(delay)
                    pause = False
                    continue
                attempts += 1
                if not exc.retryable:
                    raise CallError(str(exc), exc.raw, wasted) from exc
                last, pause = _Transient(exc.reason, exc.raw), True
                continue
            attempts += 1
            pause = True
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
