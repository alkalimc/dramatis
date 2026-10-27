"""`forge persona --jobs`, and HTTP 429 handling in the persona client.

Offline: replies are recorded fixtures behind an `httpx.MockTransport`; the clock and the
sleep are fakes, so no test waits for real.
"""

from __future__ import annotations

import threading

import httpx
import pytest
from persona_support import (
    ENDPOINTS,
    FakeEndpoint,
    key_lookup,
    offline,  # noqa: F401  (autouse fixture)
    recorded,
)
from test_persona import (  # noqa: F401  (fixtures)
    GEN,
    endpoints_file,
    paths,
    rows,
    run,
    toy,
)

from dramatis_forge.persona import store
from dramatis_forge.persona.client import CallError, Client, Gate
from dramatis_forge.persona.command import UsageError
from dramatis_forge.persona.endpoints import Endpoints, persona_target

OK = recorded("responses_ok.json")
SLOW = (429, {"error": {"message": "all proxy slots full: rph limit reached"}})


class Clock:
    """A clock that only the fake sleep moves."""

    def __init__(self) -> None:
        self.now = 1000.0
        self.slept: list[float] = []
        self._lock = threading.Lock()

    def __call__(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        with self._lock:
            self.slept.append(seconds)
            self.now += seconds


def client(handler, clock: Clock, *, retries: int = 3, budget: float = 3600.0,
           gate: Gate | None = None) -> Client:
    target = persona_target(Endpoints.from_toml(ENDPOINTS.format(wire="responses")),
                            key_lookup())
    return Client(target, retries=retries, transport=httpx.MockTransport(handler),
                  wait=lambda _n: 0.0, rate_limit_wait_s=budget, clock=clock,
                  sleep=clock.sleep, gate=gate)


def sequence(*replies: tuple[int, dict, dict[str, str]]):
    """A handler answering with each reply in turn (the last one repeats)."""
    seen: list[httpx.Request] = []
    lock = threading.Lock()

    def handler(request: httpx.Request) -> httpx.Response:
        with lock:
            seen.append(request)
            status, body, headers = replies[min(len(seen), len(replies)) - 1]
        return httpx.Response(status, json=body, headers=headers)

    handler.seen = seen  # type: ignore[attr-defined]
    return handler


def limited(headers: dict[str, str] | None = None) -> tuple[int, dict, dict[str, str]]:
    return SLOW[0], SLOW[1], headers or {}


def ok() -> tuple[int, dict, dict[str, str]]:
    return 200, OK, {}


def call(c: Client):
    return c.call("s", "u", GEN.schema(False))


# --------------------------------------------------------------------------- #
# 429
# --------------------------------------------------------------------------- #


def test_429_then_success_does_not_use_an_attempt():
    clock = Clock()
    h = sequence(limited(), limited(), ok())
    reply = call(client(h, clock, retries=1))  # one attempt allowed, two 429s survived
    assert reply.attempts == 1 and len(h.seen) == 3
    assert clock.slept == [30.0, 60.0]  # no Retry-After: 30 s, doubling


def test_retry_after_is_honoured():
    clock = Clock()
    h = sequence(limited({"Retry-After": "7"}), ok())
    call(client(h, clock))
    assert clock.slept == [7.0]


def test_backoff_without_retry_after_is_capped():
    clock = Clock()
    h = sequence(*[limited()] * 7, ok())
    call(client(h, clock))
    assert clock.slept == [30.0, 60.0, 120.0, 240.0, 300.0, 300.0, 300.0]


def test_429_past_the_budget_is_an_error():
    clock = Clock()
    h = sequence(limited())
    with pytest.raises(CallError, match="rate limited.*rate_limit_wait_s 100"):
        call(client(h, clock, budget=100.0))
    # 30 + 60 fits in 100 s; the next wait of 120 s would not.
    assert clock.slept == [30.0, 60.0] and len(h.seen) == 3


def test_other_failures_still_count_as_attempts():
    clock = Clock()
    h = sequence((503, {"error": {"message": "busy"}}, {}), limited(), ok())
    reply = call(client(h, clock, retries=2))
    assert reply.attempts == 2  # the 503 and the success; the 429 is free


def test_a_gate_holds_every_thread():
    clock = Clock()
    gate = Gate(clock, clock.sleep)
    gate.hold(45.0)  # another worker met a 429
    h = sequence(ok())
    call(client(h, clock, gate=gate))
    assert clock.slept == [45.0] and len(h.seen) == 1


def test_a_429_in_one_thread_pauses_a_sibling():
    clock = Clock()
    gate = Gate(clock, clock.sleep)
    first_sent = threading.Event()
    lock = threading.Lock()
    sent: list[float] = []  # clock reading at each request

    def handler(request: httpx.Request) -> httpx.Response:
        with lock:
            sent.append(clock.now)
            n = len(sent)
        if n == 1:
            first_sent.set()
            return httpx.Response(429, json=SLOW[1], headers={"Retry-After": "20"})
        return httpx.Response(200, json=OK)

    c = client(handler, clock, gate=gate)
    out: list = []
    t1 = threading.Thread(target=lambda: out.append(call(c)))
    t1.start()
    first_sent.wait(5)
    t1.join(5)  # t1 retries after the 20 s hold
    t2 = threading.Thread(target=lambda: out.append(call(c)))
    t2.start()
    t2.join(5)
    assert len(out) == 2 and sent[0] == 1000.0
    assert all(t >= 1020.0 for t in sent[1:])


# --------------------------------------------------------------------------- #
# --jobs
# --------------------------------------------------------------------------- #


def snapshot(result, where):
    rep = result.report
    return ([(o.subject, o.status, o.reasons, o.attempts, o.usage) for o in rep.outcomes],
            rep.missing, rep.figures(), rows(where))


def test_jobs_report_and_rows_equal_a_serial_run(toy, paths, endpoints_file, tmp_path):  # noqa: F811
    serial_lines: list[str] = []
    serial = run(toy, paths, endpoints_file, FakeEndpoint(OK), progress=serial_lines.append)
    want = snapshot(serial, paths)
    assert len(serial.report.outcomes) >= 3

    from dramatis_forge.config import Paths

    from persona_support import build_folio
    other = Paths.for_pack("toy", home=tmp_path / "home2").ensure()
    build_folio(other.folio)
    lines: list[str] = []
    pooled = run(toy, other, endpoints_file, FakeEndpoint(OK), jobs=3, progress=lines.append)
    assert snapshot(pooled, other) == want
    # Same progress lines; `[i/N]` counts completions.
    shape = lambda ls: sorted(ln.split("] ", 1)[1] for ln in ls if ln.startswith("["))  # noqa: E731
    assert shape(lines) == shape(serial_lines)
    assert [ln.split("]")[0] for ln in lines if ln.startswith("[")] \
        == [f"[{i}/{len(serial.report.outcomes)}" for i in range(1, len(serial.report.outcomes) + 1)]


def test_calls_overlap_and_writes_stay_on_the_main_thread(toy, paths, endpoints_file,  # noqa: F811
                                                         monkeypatch):
    barrier = threading.Barrier(3, timeout=10)  # three requests must be in flight together
    threads: set[int] = set()
    ep = FakeEndpoint(OK)
    handler = ep.handler

    def meeting(request: httpx.Request) -> httpx.Response:
        threads.add(threading.get_ident())
        barrier.wait()
        return handler(request)

    ep.handler = meeting  # type: ignore[method-assign]
    writers: set[int] = set()
    real = store.write

    def spy(*a, **k):
        writers.add(threading.get_ident())
        return real(*a, **k)

    monkeypatch.setattr(store, "write", spy)
    result = run(toy, paths, endpoints_file, ep, jobs=3)
    assert len(ep.requests) >= 3 and len(threads) == 3
    assert main_only(writers)
    assert all(o.status != "error" for o in result.report.outcomes)


def main_only(idents: set[int]) -> bool:
    return idents == {threading.get_ident()}


def test_a_pooled_resume_makes_no_call(toy, paths, endpoints_file):  # noqa: F811
    run(toy, paths, endpoints_file, FakeEndpoint(OK), jobs=2)
    ep = FakeEndpoint(OK)
    again = run(toy, paths, endpoints_file, ep, jobs=2, lookup=key_lookup(None))
    assert not ep.requests
    assert {o.status for o in again.report.outcomes} <= {"resumed", "skipped", "no material",
                                                          "failed"}


def test_a_429_in_the_pool_is_retried(toy, paths, endpoints_file, monkeypatch):  # noqa: F811
    monkeypatch.setattr("dramatis_forge.persona.client.RATE_WAIT_FIRST", 0.05)
    ep = FakeEndpoint(SLOW, OK)  # the first request is refused, the rest succeed
    result = run(toy, paths, endpoints_file, ep, jobs=3)
    assert all(o.status != "error" for o in result.report.outcomes)
    called = [o for o in result.report.outcomes if o.prompt is not None]
    assert len(ep.requests) == len(called) + 1
    assert all(o.attempts == 1 for o in called)


def test_jobs_below_one_is_a_usage_error(toy, paths, endpoints_file):  # noqa: F811
    with pytest.raises(UsageError, match="--jobs"):
        run(toy, paths, endpoints_file, FakeEndpoint(OK), jobs=0)
