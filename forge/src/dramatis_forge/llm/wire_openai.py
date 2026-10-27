"""OpenAI-style protocols over `httpx`: `responses` and `chat`.

`responses`: `POST {base_url}/responses` with `store = false`, the effort as
`reasoning.effort`, the schema as `text.format`. `chat`: `POST {base_url}/chat/completions`
with the schema as `response_format` and the effort as `reasoning_effort`.
"""

from __future__ import annotations

from typing import Any

import httpx

from .types import END_TURN, MAX_TOKENS, REFUSAL, TOOL_USE, Request, Response, Usage

#: Read timeout for one call. High reasoning effort on a long input can think for minutes.
READ_TIMEOUT = 600.0
RETRY_STATUS = frozenset({408, 409, 429, 500, 502, 503, 504})


def body(protocol: str, model: str, req: Request) -> tuple[str, dict[str, Any]]:
    """(path, JSON body) of one call. Pure, so a probe can record exactly what is sent."""
    msgs = [{"role": "system", "content": req.system},
            *({"role": m.role, "content": m.content} for m in req.messages)]
    if protocol == "responses":
        out: dict[str, Any] = {"model": model, "input": msgs, "store": False}
        if req.output_schema is not None:
            out["text"] = {"format": {"type": "json_schema", "name": req.schema_name,
                                      "schema": req.output_schema, "strict": True}}
        if req.effort:
            out["reasoning"] = {"effort": req.effort}
        if req.max_output:
            out["max_output_tokens"] = req.max_output
        return "responses", out
    out = {"model": model, "messages": msgs}
    if req.output_schema is not None:
        out["response_format"] = {"type": "json_schema",
                                  "json_schema": {"name": req.schema_name,
                                                  "schema": req.output_schema, "strict": True}}
    if req.effort:
        out["reasoning_effort"] = req.effort
    if req.max_output:
        out["max_completion_tokens"] = req.max_output
    return "chat/completions", out


def _int(d: Any, *path: str) -> int:
    for key in path:
        if not isinstance(d, dict):
            return 0
        d = d.get(key)
    return d if isinstance(d, int) and not isinstance(d, bool) else 0


def _get(d: Any, *path: str) -> Any:
    for key in path:
        d = d.get(key) if isinstance(d, dict) else None
    return d


def usage_of(raw: Any, protocol: str) -> Usage:
    """Normalised usage: `uncached_in` excludes the cached part; `output` includes reasoning."""
    if protocol == "responses":
        cached = _int(raw, "usage", "input_tokens_details", "cached_tokens")
        return Usage(max(0, _int(raw, "usage", "input_tokens") - cached), cached, 0,
                     _int(raw, "usage", "output_tokens"),
                     _int(raw, "usage", "output_tokens_details", "reasoning_tokens"))
    usage = _get(raw, "usage")
    if _get(usage, "prompt_cache_hit_tokens") is not None:  # the hit/miss form some providers use
        cached, uncached = _int(usage, "prompt_cache_hit_tokens"), _int(
            usage, "prompt_cache_miss_tokens")
    else:
        cached = _int(usage, "prompt_tokens_details", "cached_tokens")
        uncached = max(0, _int(usage, "prompt_tokens") - cached)
    return Usage(uncached, cached, 0, _int(usage, "completion_tokens"),
                 _int(usage, "completion_tokens_details", "reasoning_tokens"))


def decode(raw: dict, protocol: str) -> Response:
    usage = usage_of(raw, protocol)
    if protocol == "responses":
        parts: list[str] = []
        refusal = ""
        calls = False
        for item in raw.get("output") or []:
            calls = calls or item.get("type") == "function_call"
            if item.get("type") != "message":
                continue  # reasoning items carry no answer
            for c in item.get("content") or []:
                if c.get("type") == "refusal":
                    refusal += c.get("refusal", "")
                elif c.get("type") == "output_text":
                    parts.append(c.get("text", ""))
        if refusal:
            return Response(refusal, REFUSAL, usage, raw)
        status = raw.get("status")
        if status not in (None, "completed"):
            reason = _get(raw, "incomplete_details", "reason") or status
            stop = MAX_TOKENS if reason == "max_output_tokens" else f"other:{reason}"
            return Response("".join(parts), stop, usage, raw)
        return Response("".join(parts), TOOL_USE if calls else END_TURN, usage, raw)
    choices = raw.get("choices") or [{}]
    message = choices[0].get("message") or {}
    if message.get("refusal"):
        return Response(message["refusal"], REFUSAL, usage, raw)
    reason = choices[0].get("finish_reason")
    stop = {None: END_TURN, "stop": END_TURN, "length": MAX_TOKENS, "tool_calls": TOOL_USE,
            "content_filter": REFUSAL}.get(reason, f"other:{reason}")
    return Response(message.get("content") or "", stop, usage, raw)


def once(protocol: str, model: str, base_url: str, key: str, req: Request,
         transport: httpx.BaseTransport | None) -> tuple[int, Any, float | None]:
    """One POST: (status, decoded JSON body or {"body": text}, seconds named by a
    `Retry-After` header, if any). Network failure raises `httpx.TransportError`."""
    path, payload = body(protocol, model, req)
    return send_full("POST", base_url, path, key, transport, json=payload)


def is_loopback(base_url: str) -> bool:
    """Whether the endpoint runs on this machine. Such a call must not go through a system
    proxy: the proxy cannot reach this machine's loopback and answers 404."""
    host = httpx.URL(base_url).host
    return host in ("localhost", "127.0.0.1", "::1") or host.endswith(".localhost")


def send(method: str, base_url: str, path: str, key: str,
         transport: httpx.BaseTransport | None, *, json: Any = None,
         headers: dict[str, str] | None = None) -> tuple[int, Any]:
    status, raw, _ = send_full(method, base_url, path, key, transport, json=json,
                               headers=headers)
    return status, raw


def retry_after(value: str | None) -> float | None:
    """Seconds named by a `Retry-After` header given as a number; an HTTP date is ignored."""
    try:
        seconds = float(value) if value is not None else None
    except ValueError:
        return None
    return seconds if seconds is not None and seconds >= 0 else None


def send_full(method: str, base_url: str, path: str, key: str,
              transport: httpx.BaseTransport | None, *, json: Any = None,
              headers: dict[str, str] | None = None) -> tuple[int, Any, float | None]:
    all_headers = dict(headers or {})
    if key:
        all_headers.setdefault("Authorization", f"Bearer {key}")
    with httpx.Client(base_url=base_url.rstrip("/") + "/", headers=all_headers,
                      timeout=httpx.Timeout(30.0, read=READ_TIMEOUT),
                      transport=transport, trust_env=not is_loopback(base_url)) as http:
        resp = http.request(method, path, json=json)
    wait = retry_after(resp.headers.get("retry-after"))
    try:
        return resp.status_code, resp.json(), wait
    except ValueError:
        return resp.status_code, {"body": resp.text[:2000]}, wait
