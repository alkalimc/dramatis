"""Provider-neutral types of the forge's LLM layer. Nothing here names a wire format
except the `protocol` string of an `Endpoint`."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any

PROTOCOLS = ("chat", "responses", "messages")


@dataclass(frozen=True)
class Endpoint:
    #: One of PROTOCOLS. Named like the endpoints file's `wire_api`.
    protocol: str
    #: API root including the version segment.
    base_url: str
    model: str
    key: str = field(default="", repr=False)


@dataclass(frozen=True)
class Message:
    role: str  # "user" | "assistant"
    content: str


@dataclass(frozen=True)
class Request:
    system: str
    messages: tuple[Message, ...]
    #: Ask for one JSON document matching this schema.
    output_schema: dict[str, Any] | None = None
    #: Passed as the protocol's reasoning parameter; None sends none.
    effort: str | None = None
    max_output: int | None = None
    schema_name: str = "output"


@dataclass(frozen=True)
class Usage:
    uncached_in: int = 0
    cached_in: int = 0
    cache_write_in: int = 0
    #: Includes `reasoning`.
    output: int = 0
    reasoning: int = 0

    def __add__(self, other: Usage) -> Usage:
        return Usage(self.uncached_in + other.uncached_in, self.cached_in + other.cached_in,
                     self.cache_write_in + other.cache_write_in, self.output + other.output,
                     self.reasoning + other.reasoning)

    @property
    def total_in(self) -> int:
        return self.uncached_in + self.cached_in + self.cache_write_in


#: `Response.stop` values. An unfinished reply is anything but END_TURN.
END_TURN = "end_turn"
MAX_TOKENS = "max_tokens"
REFUSAL = "refusal"
TOOL_USE = "tool_use"


@dataclass
class Response:
    #: The answer text; for a refusal, the refusal text where the protocol gives one.
    text: str
    stop: str
    usage: Usage
    raw: dict[str, Any]
    attempts: int = 1

    @property
    def parsed(self) -> dict[str, Any] | None:
        """The JSON object in `text`, or None when it holds none."""
        try:
            return parse_json(self.text)
        except ValueError:
            return None


def parse_json(text: str) -> dict[str, Any]:
    """The JSON object in a reply. Tolerates a fenced block: some endpoints wrap
    structured output in one even when asked for a schema."""
    body = text.strip()
    if body.startswith("```"):
        body = body.split("\n", 1)[1] if "\n" in body else ""
        body = body.rsplit("```", 1)[0]
    value = json.loads(body)  # JSONDecodeError is a ValueError
    if not isinstance(value, dict):
        raise ValueError("output is not a JSON object")
    return value


class LlmError(Exception):
    """A call that failed. `retryable` is whether more attempts could have helped (the
    failure was transient); `attempts` is how many were made; `raw` is the last response
    body, if any."""

    def __init__(self, message: str, *, status: int | None = None, raw: Any = None,
                 retryable: bool = False, attempts: int = 1, reason: str = "",
                 retry_after: float | None = None) -> None:
        super().__init__(message)
        #: The last failure alone, without the attempt count `message` may carry.
        self.reason = reason or message
        self.status = status
        self.raw = raw
        self.retryable = retryable
        self.attempts = attempts
        #: Seconds the server asked to wait (`Retry-After`), when it said.
        self.retry_after = retry_after
