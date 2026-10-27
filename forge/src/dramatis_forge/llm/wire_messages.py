"""Anthropic-style `messages` through the official SDK.

Thinking is adaptive and always on; the depth is `output_config.effort`. The cache policy
is the automatic breakpoint at the end of the request. Nothing is prefilled and no
`tool_choice` is sent. Retries are ours (`max_retries=0` on the client) so every protocol
retries by the same rules; the SDK's typed errors tell which failures are transient.
"""

from __future__ import annotations

from typing import Any

import anthropic
import httpx
import httpx2

from .types import END_TURN, MAX_TOKENS, REFUSAL, TOOL_USE, LlmError, Request, Response, Usage
from .wire_openai import READ_TIMEOUT, RETRY_STATUS, retry_after

#: Messages requires `max_tokens`; thinking counts against it.
DEFAULT_MAX_OUTPUT = 32_000
#: Above this the SDK refuses a non-streaming call (it could outlast the HTTP timeout).
STREAM_ABOVE = 16_000
_STOP = {"end_turn": END_TURN, "stop_sequence": END_TURN, "tool_use": TOOL_USE,
         "max_tokens": MAX_TOKENS, "model_context_window_exceeded": MAX_TOKENS,
         "refusal": REFUSAL}


class _Bridge(httpx2.BaseTransport):
    """Lets a caller inject one `httpx` transport for every protocol: the SDK is built on
    `httpx2` and rejects `httpx` objects, so requests and replies are converted here."""

    def __init__(self, inner: httpx.BaseTransport) -> None:
        self.inner = inner

    def handle_request(self, request: httpx2.Request) -> httpx2.Response:
        reply = self.inner.handle_request(httpx.Request(
            request.method, str(request.url), headers=list(request.headers.raw),
            content=request.read()))
        body = reply.read()  # decoded already: drop the headers that describe the wire form
        headers = [(k, v) for k, v in reply.headers.raw
                   if k.lower() not in (b"content-encoding", b"content-length",
                                        b"transfer-encoding")]
        return httpx2.Response(reply.status_code, headers=headers, content=body)


def sdk_base_url(base_url: str) -> str:
    """The SDK appends `/v1/messages` itself and our base URL already ends in `/v1`."""
    base = base_url.rstrip("/")
    return base[: -len("/v1")] if base.endswith("/v1") else base


def kwargs(model: str, req: Request) -> dict[str, Any]:
    """The arguments of `messages.create`, which are also the JSON body's keys."""
    out: dict[str, Any] = {
        "model": model,
        "max_tokens": req.max_output or DEFAULT_MAX_OUTPUT,
        "system": req.system,
        "messages": [{"role": m.role, "content": m.content} for m in req.messages],
        "thinking": {"type": "adaptive"},
    }
    config: dict[str, Any] = {}
    if req.effort:
        config["effort"] = req.effort
    if req.output_schema is not None:
        config["format"] = {"type": "json_schema", "schema": req.output_schema}
    if config:
        out["output_config"] = config
    return out


def usage_of(raw: Any) -> Usage:
    """`input_tokens` is only the uncached part; `output_tokens` includes thinking."""
    u = raw.get("usage") if isinstance(raw, dict) else None
    u = u if isinstance(u, dict) else {}

    def n(key: str) -> int:
        v = u.get(key)
        return v if isinstance(v, int) and not isinstance(v, bool) else 0

    return Usage(n("input_tokens"), n("cache_read_input_tokens"),
                 n("cache_creation_input_tokens"), n("output_tokens"), 0)


def decode(raw: dict[str, Any]) -> Response:
    usage = usage_of(raw)
    stop = _STOP.get(raw.get("stop_reason") or "end_turn", f"other:{raw.get('stop_reason')}")
    if stop == REFUSAL:  # checked before the content is read
        return Response("", stop, usage, raw)
    text = "".join(b.get("text", "") for b in raw.get("content") or []
                   if b.get("type") == "text")
    return Response(text, stop, usage, raw)


def once(model: str, base_url: str, key: str, req: Request,
         transport: httpx.BaseTransport | None) -> Response:
    args = kwargs(model, req)
    timeout = httpx2.Timeout(30.0, read=READ_TIMEOUT)
    http = None
    if transport is not None:
        inner = transport if isinstance(transport, httpx2.BaseTransport) else _Bridge(transport)
        http = httpx2.Client(transport=inner, timeout=timeout)
    client = anthropic.Anthropic(api_key=key, base_url=sdk_base_url(base_url), max_retries=0,
                                 http_client=http, timeout=timeout)
    try:
        with client:
            if args["max_tokens"] > STREAM_ABOVE:
                with client.messages.stream(**args) as stream:
                    message = stream.get_final_message()
            else:
                message = client.messages.create(**args)
    except anthropic.APIStatusError as exc:
        raise LlmError(f"HTTP {exc.status_code}: {_text(exc)}", status=exc.status_code,
                       raw=exc.body,
                       retryable=exc.status_code in RETRY_STATUS or exc.status_code >= 500,
                       retry_after=retry_after(exc.response.headers.get("retry-after"))) from exc
    except anthropic.APIConnectionError as exc:  # includes timeouts
        raise LlmError(f"transport: {exc}", retryable=True) from exc
    return decode(message.model_dump(mode="json"))


def _text(exc: anthropic.APIStatusError) -> str:
    body = exc.body
    if isinstance(body, dict):
        inner = body.get("error")
        if isinstance(inner, dict) and isinstance(inner.get("message"), str):
            return inner["message"]
    return exc.message
