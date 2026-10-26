"""One interface over the external LLM protocols for the forge.

    from dramatis_forge import llm
    reply = llm.complete(llm.Endpoint("messages", base_url, model, key), llm.Request(...))

Each protocol is an adapter behind `complete` (`wire_openai`, `wire_messages`); callers
never see a wire format.
"""

from .core import backoff, complete, models, usage_of
from .types import (
    END_TURN,
    MAX_TOKENS,
    PROTOCOLS,
    REFUSAL,
    TOOL_USE,
    Endpoint,
    LlmError,
    Message,
    Request,
    Response,
    Usage,
    parse_json,
)
from .wire_messages import kwargs as messages_kwargs
from .wire_openai import body as openai_body

__all__ = [
    "END_TURN", "MAX_TOKENS", "PROTOCOLS", "REFUSAL", "TOOL_USE", "Endpoint", "LlmError",
    "Message", "Request", "Response", "Usage", "backoff", "complete", "messages_kwargs",
    "models", "openai_body", "parse_json", "usage_of",
]
