"""The forge's LLM layer, offline: every protocol through an `httpx.MockTransport`.

The Anthropic path goes through the official SDK, which accepts the same transport, so
these tests also pin what the SDK puts on the wire (URL, auth header, body keys).
"""

from __future__ import annotations

import json
import socket
from pathlib import Path

import httpx
import pytest

from dramatis_forge import llm
from dramatis_forge.llm.wire_messages import sdk_base_url
from llm_support import sse_of

FIXTURES = Path(__file__).resolve().parent / "fixtures"
SCHEMA = {"type": "object", "properties": {"answer": {"type": "string"}},
          "required": ["answer"], "additionalProperties": False}


@pytest.fixture(autouse=True)
def offline(monkeypatch: pytest.MonkeyPatch) -> None:
    def refuse(*_a, **_k):
        raise RuntimeError("tests must not open network connections")

    monkeypatch.setattr(socket, "socket", refuse)
    monkeypatch.setattr(socket, "create_connection", refuse)


def recorded(name: str) -> dict:
    return json.loads((FIXTURES / "llm" / name).read_text(encoding="utf-8"))


class Fake:
    """Serves replies in order (the last repeats) and records every request."""

    def __init__(self, *replies: dict | tuple[int, dict]) -> None:
        self.replies = list(replies)
        self.requests: list[httpx.Request] = []

    @property
    def bodies(self) -> list[dict]:
        return [json.loads(r.content) for r in self.requests if r.content]

    def handler(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        reply = self.replies[min(len(self.requests), len(self.replies)) - 1]
        status, body = reply if isinstance(reply, tuple) else (200, reply)
        if json.loads(request.content or b"{}").get("stream") and status == 200:
            return httpx.Response(200, text=sse_of(body),
                                  headers={"content-type": "text/event-stream"})
        return httpx.Response(status, json=body)

    @property
    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self.handler)


def endpoint(protocol: str, base: str = "https://llm.example.invalid/v1") -> llm.Endpoint:
    return llm.Endpoint(protocol, base, "model-a", "sk-test-not-a-real-key")


def request(**kw) -> llm.Request:
    return llm.Request(system="sys", messages=(llm.Message("user", "hi"),),
                       output_schema=SCHEMA, effort="high", **kw)


def no_wait(_attempt: int) -> float:
    return 0


# --------------------------------------------------------------------------- #
# messages (official SDK)
# --------------------------------------------------------------------------- #


def test_sdk_base_url_drops_the_version_segment():
    assert sdk_base_url("https://x.invalid/v1") == "https://x.invalid"
    assert sdk_base_url("https://x.invalid/v1/") == "https://x.invalid"
    assert sdk_base_url("https://x.invalid/proxy/v1") == "https://x.invalid/proxy"
    assert sdk_base_url("https://x.invalid") == "https://x.invalid"


def test_messages_url_auth_and_body_non_streaming():
    fake = Fake(recorded("messages_ok.json"))
    resp = llm.complete(endpoint("messages"), request(max_output=1000),
                        transport=fake.transport)
    req = fake.requests[0]
    # The SDK adds /v1/messages itself: the profile's /v1 must not be doubled.
    assert str(req.url) == "https://llm.example.invalid/v1/messages"
    assert req.headers["x-api-key"] == "sk-test-not-a-real-key"
    assert req.headers["anthropic-version"] == "2023-06-01"
    body = fake.bodies[0]
    assert body["thinking"] == {"type": "adaptive"}
    assert body["output_config"] == {"effort": "high",
                                     "format": {"type": "json_schema", "schema": SCHEMA}}
    assert body["system"] == "sys" and body["max_tokens"] == 1000
    assert body["messages"] == [{"role": "user", "content": "hi"}]
    assert "tool_choice" not in body and "stream" not in body
    assert resp.stop == llm.END_TURN and resp.parsed == {"answer": "harbour", "score": 3}
    assert resp.usage == llm.Usage(40, 5000, 200, 150, 0)
    assert resp.usage.total_in == 5240


def test_a_large_output_budget_streams():
    fake = Fake(recorded("messages_ok.json"))
    resp = llm.complete(endpoint("messages"), request(), transport=fake.transport)
    assert fake.bodies[0]["stream"] is True and fake.bodies[0]["max_tokens"] == 32_000
    assert resp.text == '{"answer": "harbour", "score": 3}'
    assert resp.usage.cached_in == 5000 and resp.usage.output == 150


def test_refusal_is_read_before_the_content():
    for max_output in (1000, None):
        fake = Fake(recorded("messages_refusal.json"))
        resp = llm.complete(endpoint("messages"), request(max_output=max_output),
                            transport=fake.transport)
        assert resp.stop == llm.REFUSAL and resp.text == "" and resp.parsed is None


def test_truncated_output_is_not_an_end_turn():
    fake = Fake(recorded("messages_truncated.json"))
    resp = llm.complete(endpoint("messages"), request(max_output=1000),
                        transport=fake.transport)
    assert resp.stop == llm.MAX_TOKENS and resp.parsed is None


@pytest.mark.parametrize("status", [429, 500, 529])
def test_messages_transient_errors_retry(status):
    fake = Fake((status, {"type": "error", "error": {"type": "x", "message": "busy"}}),
                recorded("messages_ok.json"))
    resp = llm.complete(endpoint("messages"), request(max_output=1000),
                        transport=fake.transport, retries=3, wait=no_wait)
    assert resp.attempts == 2 and len(fake.requests) == 2


def test_messages_retries_are_bounded_and_client_errors_final():
    busy = (429, {"type": "error", "error": {"type": "rate_limit_error", "message": "slow"}})
    fake = Fake(busy)
    with pytest.raises(llm.LlmError, match=r"gave up after 2 attempt\(s\).*slow") as e:
        llm.complete(endpoint("messages"), request(max_output=1000), transport=fake.transport,
                     retries=2, wait=no_wait)
    assert e.value.retryable and len(fake.requests) == 2
    fake = Fake((400, {"type": "error", "error": {"type": "invalid_request_error",
                                                   "message": "bad schema"}}))
    with pytest.raises(llm.LlmError, match="HTTP 400: bad schema") as e:
        llm.complete(endpoint("messages"), request(max_output=1000), transport=fake.transport,
                     retries=3, wait=no_wait)
    assert not e.value.retryable and len(fake.requests) == 1


def test_a_dropped_connection_retries():
    calls = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if len(calls) == 1:
            raise httpx.ConnectError("reset", request=request)
        return httpx.Response(200, json=recorded("messages_ok.json"))

    resp = llm.complete(endpoint("messages"), request(max_output=1000),
                        transport=httpx.MockTransport(handler), retries=2, wait=no_wait)
    assert resp.attempts == 2


# --------------------------------------------------------------------------- #
# OpenAI-style protocols and the shared surface
# --------------------------------------------------------------------------- #

RESPONSES = {"status": "completed", "output": [
    {"type": "reasoning", "summary": []},
    {"type": "message", "content": [{"type": "output_text", "text": '{"answer": "a"}'}]}],
    "usage": {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 60},
              "output_tokens": 30, "output_tokens_details": {"reasoning_tokens": 12}}}
CHAT = {"choices": [{"finish_reason": "stop", "message": {"content": '{"answer": "a"}'}}],
        "usage": {"prompt_tokens": 100, "completion_tokens": 30,
                  "prompt_tokens_details": {"cached_tokens": 60}}}


def test_responses_and_chat_requests():
    fake = Fake(RESPONSES)
    resp = llm.complete(endpoint("responses"), request(max_output=500), transport=fake.transport)
    assert str(fake.requests[0].url) == "https://llm.example.invalid/v1/responses"
    assert fake.requests[0].headers["authorization"] == "Bearer sk-test-not-a-real-key"
    body = fake.bodies[0]
    assert body["store"] is False and body["reasoning"] == {"effort": "high"}
    assert body["input"][0] == {"role": "system", "content": "sys"}
    assert body["text"]["format"]["schema"] == SCHEMA and body["max_output_tokens"] == 500
    assert resp.usage == llm.Usage(40, 60, 0, 30, 12) and resp.parsed == {"answer": "a"}

    fake = Fake(CHAT)
    resp = llm.complete(endpoint("chat"), request(), transport=fake.transport)
    assert str(fake.requests[0].url) == "https://llm.example.invalid/v1/chat/completions"
    body = fake.bodies[0]
    assert body["reasoning_effort"] == "high" and "store" not in body
    assert body["response_format"]["json_schema"]["schema"] == SCHEMA
    assert resp.usage == llm.Usage(40, 60, 0, 30, 0)


def test_usage_reads_every_shape():
    hit_miss = {"usage": {"prompt_cache_hit_tokens": 70, "prompt_cache_miss_tokens": 30,
                          "completion_tokens": 5}}
    assert llm.usage_of("chat", hit_miss) == llm.Usage(30, 70, 0, 5, 0)
    assert llm.usage_of("responses", {}) == llm.Usage()
    assert llm.usage_of("messages", {"usage": {"input_tokens": 3, "output_tokens": 4}}) \
        == llm.Usage(3, 0, 0, 4, 0)


def test_openai_incomplete_and_refusal():
    inc = {**RESPONSES, "status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}
    resp = llm.complete(endpoint("responses"), request(), transport=Fake(inc).transport)
    assert resp.stop == llm.MAX_TOKENS
    refused = {"status": "completed", "output": [{"type": "message", "content": [
        {"type": "refusal", "refusal": "no"}]}]}
    resp = llm.complete(endpoint("responses"), request(), transport=Fake(refused).transport)
    assert (resp.stop, resp.text) == (llm.REFUSAL, "no")
    refused = {"choices": [{"message": {"refusal": "no"}, "finish_reason": "stop"}]}
    resp = llm.complete(endpoint("chat"), request(), transport=Fake(refused).transport)
    assert resp.stop == llm.REFUSAL


def test_openai_errors_retry_by_status():
    fake = Fake((503, {"error": {"message": "busy"}}), CHAT)
    resp = llm.complete(endpoint("chat"), request(), transport=fake.transport, wait=no_wait)
    assert resp.attempts == 2
    fake = Fake((401, {"error": {"message": "bad key"}}))
    with pytest.raises(llm.LlmError, match="HTTP 401: bad key"):
        llm.complete(endpoint("chat"), request(), transport=fake.transport, wait=no_wait)
    assert len(fake.requests) == 1


def test_fenced_json_is_accepted():
    assert llm.parse_json('```json\n{"a": 1}\n```') == {"a": 1}
    with pytest.raises(ValueError):
        llm.parse_json("[1]")


@pytest.mark.parametrize("protocol", llm.PROTOCOLS)
def test_models_lists_ids_and_reports_the_endpoints_error(protocol):
    fake = Fake({"data": [{"id": "m1"}, {"id": "m2"}]})
    assert llm.models(endpoint(protocol), transport=fake.transport) == ["m1", "m2"]
    req = fake.requests[0]
    assert str(req.url) == "https://llm.example.invalid/v1/models"
    if protocol == "messages":
        assert req.headers["x-api-key"] == "sk-test-not-a-real-key"
    else:
        assert req.headers["authorization"] == "Bearer sk-test-not-a-real-key"
    bad = Fake((401, {"error": {"message": "invalid key"}}))
    with pytest.raises(llm.LlmError, match="invalid key"):
        llm.models(endpoint(protocol), transport=bad.transport)


def test_endpoint_repr_hides_the_key():
    assert "sk-test" not in repr(endpoint("chat"))
