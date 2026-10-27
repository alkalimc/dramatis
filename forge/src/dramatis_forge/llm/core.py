"""`complete` and `models`: the two calls every caller makes, whatever the protocol."""

from __future__ import annotations

import time
from collections.abc import Callable
from typing import Any

import httpx

from . import wire_messages, wire_openai
from .types import PROTOCOLS, Endpoint, LlmError, Request, Response, Usage


def usage_of(protocol: str, raw: Any) -> Usage:
    """Normalised usage of a raw reply body."""
    if protocol == "messages":
        return wire_messages.usage_of(raw)
    return wire_openai.usage_of(raw, protocol)


def backoff(attempt: int) -> float:
    """Seconds to wait after the `attempt`-th failure: 2, 4, 8 ... capped at 60."""
    return float(min(60, 2 * 2 ** (attempt - 1)))


def complete(endpoint: Endpoint, request: Request, *,
             transport: httpx.BaseTransport | None = None, retries: int = 3,
             wait: Callable[[int], float] | None = None,
             sleep: Callable[[float], None] = time.sleep) -> Response:
    """One call, up to `retries` attempts. Rate limits, server errors and network failures
    are retried with backoff; any other failure is final. The attempts made are in
    `Response.attempts` (or `LlmError.attempts`)."""
    if endpoint.protocol not in PROTOCOLS:
        raise ValueError(f"unknown protocol {endpoint.protocol!r}")
    attempts = max(1, retries)
    last: LlmError | None = None
    for attempt in range(1, attempts + 1):
        try:
            response = _once(endpoint, request, transport)
        except LlmError as exc:
            exc.attempts = attempt
            if not exc.retryable:
                raise
            last = exc
            if attempt < attempts:
                sleep((wait or backoff)(attempt))
            continue
        response.attempts = attempt
        return response
    assert last is not None
    raise LlmError(f"gave up after {attempts} attempt(s): {last}", status=last.status,
                   raw=last.raw, retryable=True, attempts=attempts,
                   reason=str(last), retry_after=last.retry_after) from last


def _once(endpoint: Endpoint, request: Request,
          transport: httpx.BaseTransport | None) -> Response:
    if endpoint.protocol == "messages":
        return wire_messages.once(endpoint.model, endpoint.base_url, endpoint.key, request,
                                  transport)
    try:
        status, raw, wait = wire_openai.once(endpoint.protocol, endpoint.model, endpoint.base_url,
                                       endpoint.key, request, transport)
    except httpx.TransportError as exc:
        raise LlmError(f"transport: {exc}", retryable=True) from exc
    _check(status, raw, wait)
    if not isinstance(raw, dict):
        raise LlmError("response body is not a JSON object", raw=raw, retryable=True)
    return wire_openai.decode(raw, endpoint.protocol)


def _check(status: int, raw: Any, retry_after: float | None = None) -> None:
    if status < 400:
        return
    message = wire_openai._get(raw, "error", "message") or ""
    raise LlmError(f"HTTP {status}: {message}", status=status, raw=raw,
                   retryable=status in wire_openai.RETRY_STATUS, retry_after=retry_after)


def models(endpoint: Endpoint, *, transport: httpx.BaseTransport | None = None) -> list[str]:
    """Model ids from `GET {base_url}/models`. A failure carries the endpoint's own text."""
    headers: dict[str, str] = {}
    key = endpoint.key
    if endpoint.protocol == "messages":
        headers = {"anthropic-version": "2023-06-01", "x-api-key": key}
        key = ""
    try:
        status, raw = wire_openai.send("GET", endpoint.base_url, "models", key, transport,
                                       headers=headers)
    except httpx.TransportError as exc:
        raise LlmError(f"transport: {exc}", retryable=True) from exc
    _check(status, raw)
    return [m["id"] for m in (raw.get("data") or []) if isinstance(m, dict) and "id" in m]
