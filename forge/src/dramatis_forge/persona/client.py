"""One structured-output call to an OpenAI-style endpoint, over either wire API.

`responses`: `POST {base_url}/responses` with `store = false`, the role's reasoning as
`reasoning.effort`, and the output schema as `text.format`. `chat`: `POST
{base_url}/chat/completions` with the schema as `response_format` and the reasoning as
`reasoning_effort`. A fixed output format is fine here: this is one offline call per
person, not a cached session whose request shape must stay constant.

Transport failures, rate limits, server errors and unparseable output are retried up to
`persona.retries` attempts with exponential backoff; any other client error is final.
The transport is injectable so tests never touch the network.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

import httpx
from tenacity import (
    RetryError,
    Retrying,
    retry_if_exception_type,
    stop_after_attempt,
    wait_exponential,
)

from .endpoints import Role, Target

#: Read timeout for one call. High reasoning effort on a long input can think for minutes.
READ_TIMEOUT = 600.0
RETRY_STATUS = frozenset({408, 409, 429, 500, 502, 503, 504})




class _Transient(Exception):
    def __init__(self, message: str, raw: Any = None) -> None:
        super().__init__(message)
        self.raw = raw


@dataclass(frozen=True)
class Usage:
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


class CallError(Exception):
    """A call that failed for good. `raw` is the last response body, if any; `usage` is
    what the failed attempts were billed."""

    def __init__(self, message: str, raw: Any = None, usage: Usage | None = None) -> None:
        super().__init__(message)
        self.raw = raw
        self.usage = usage or Usage()


@dataclass
class Reply:
    output: dict[str, Any]
    raw: dict[str, Any]
    usage: Usage
    attempts: int = 1
    #: Usage of attempts that were retried, which are billed all the same.
    wasted: Usage = field(default_factory=Usage)


def _int(d: Any, *path: str) -> int:
    for key in path:
        if not isinstance(d, dict):
            return 0
        d = d.get(key)
    return d if isinstance(d, int) else 0


def usage_of(raw: Any, wire: str) -> Usage:
    if wire == "responses":
        return Usage(_int(raw, "usage", "input_tokens"),
                     _int(raw, "usage", "input_tokens_details", "cached_tokens"),
                     _int(raw, "usage", "output_tokens"),
                     _int(raw, "usage", "output_tokens_details", "reasoning_tokens"))
    return Usage(_int(raw, "usage", "prompt_tokens"),
                 _int(raw, "usage", "prompt_tokens_details", "cached_tokens"),
                 _int(raw, "usage", "completion_tokens"),
                 _int(raw, "usage", "completion_tokens_details", "reasoning_tokens"))


def _text_of(raw: dict, wire: str) -> str:
    if wire == "responses":
        if raw.get("status") not in (None, "completed"):
            reason = _get(raw, "incomplete_details", "reason") or raw.get("status")
            raise _Transient(f"response not completed: {reason}", raw)
        parts = []
        for item in raw.get("output") or []:
            if item.get("type") != "message":
                continue  # reasoning items carry no answer
            for c in item.get("content") or []:
                if c.get("type") == "refusal":
                    raise CallError(f"refused: {c.get('refusal', '')}", raw)
                if c.get("type") == "output_text":
                    parts.append(c.get("text", ""))
        return "".join(parts)
    choices = raw.get("choices") or [{}]
    message = choices[0].get("message") or {}
    if message.get("refusal"):
        raise CallError(f"refused: {message['refusal']}", raw)
    if choices[0].get("finish_reason") not in (None, "stop"):
        raise _Transient(f"finish_reason {choices[0].get('finish_reason')}", raw)
    return message.get("content") or ""


def _get(d: Any, *path: str) -> Any:
    for key in path:
        d = d.get(key) if isinstance(d, dict) else None
    return d


def parse_output(text: str) -> dict[str, Any]:
    """The JSON object in a reply. Tolerates a fenced block: some endpoints wrap
    structured output in one even when asked for a schema."""
    body = text.strip()
    if body.startswith("```"):
        body = body.split("\n", 1)[1] if "\n" in body else ""
        body = body.rsplit("```", 1)[0]
    value = json.loads(body)
    if not isinstance(value, dict):
        raise ValueError("output is not a JSON object")
    return value


def request(wire: str, role: Role, system: str, user: str, schema: dict,
            name: str = "persona") -> tuple[str, dict]:
    """(path, JSON body) of one call. Pure, so a probe can record exactly what is sent."""
    if wire == "responses":
        body: dict[str, Any] = {
            "model": role.model,
            "input": [{"role": "system", "content": system},
                      {"role": "user", "content": user}],
            "store": False,
            "text": {"format": {"type": "json_schema", "name": name, "schema": schema,
                                "strict": True}},
        }
        if role.reasoning:
            body["reasoning"] = {"effort": role.reasoning}
        return "responses", body
    body = {
        "model": role.model,
        "messages": [{"role": "system", "content": system},
                     {"role": "user", "content": user}],
        "response_format": {"type": "json_schema",
                            "json_schema": {"name": name, "schema": schema, "strict": True}},
    }
    if role.reasoning:
        body["reasoning_effort"] = role.reasoning
    return "chat/completions", body


class Client:
    def __init__(self, target: Target, *, retries: int,
                 transport: httpx.BaseTransport | None = None,
                 wait: Callable[..., float] | None = None) -> None:
        self.target = target
        self.retries = max(1, retries)
        self.wait = wait or wait_exponential(multiplier=2, min=2, max=60)
        self.http = httpx.Client(
            base_url=target.profile.base_url.rstrip("/") + "/",
            headers={"Authorization": f"Bearer {target.key}"},
            timeout=httpx.Timeout(30.0, read=READ_TIMEOUT),
            transport=transport,
        )

    def close(self) -> None:
        self.http.close()

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
        path, body = self.body(system, user, schema, name)
        wasted = Usage()
        attempts = 0
        retrying = Retrying(stop=stop_after_attempt(self.retries), wait=self.wait,
                            retry=retry_if_exception_type(_Transient), reraise=False)
        try:
            for attempt in retrying:
                with attempt:
                    attempts += 1
                    raw = self._once(path, body)
                    usage = usage_of(raw, self.wire)
                    try:
                        text = _text_of(raw, self.wire)
                        output = parse_output(text)
                    except CallError as exc:  # a refusal: final, billed all the same
                        exc.usage = wasted + usage
                        raise
                    except _Transient:
                        wasted = wasted + usage
                        raise
                    except (ValueError, json.JSONDecodeError) as exc:
                        wasted = wasted + usage
                        raise _Transient(f"unparseable output: {exc}", raw) from exc
                    return Reply(output=output, raw=raw, usage=usage, attempts=attempts,
                                 wasted=wasted)
        except RetryError as exc:
            last = exc.last_attempt.exception()
            raise CallError(f"gave up after {attempts} attempt(s): {last}",
                            getattr(last, "raw", None), wasted) from last
        raise CallError("no attempt was made")  # pragma: no cover - stop allows one

    def _once(self, path: str, body: dict) -> dict:
        try:
            resp = self.http.post(path, json=body)
        except httpx.TransportError as exc:
            raise _Transient(f"transport: {exc}") from exc
        try:
            raw = resp.json()
        except ValueError:
            raw = {"body": resp.text[:2000]}
        if resp.status_code in RETRY_STATUS:
            raise _Transient(f"HTTP {resp.status_code}", raw)
        if resp.status_code >= 400:
            raise CallError(f"HTTP {resp.status_code}: {_get(raw, 'error', 'message') or ''}",
                            raw)
        if not isinstance(raw, dict):
            raise _Transient("response body is not a JSON object", raw)
        return raw
