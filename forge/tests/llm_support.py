"""Shared by the LLM tests: the stream an Anthropic-style endpoint sends."""

import json


def sse_of(message: dict) -> str:
    """The event stream an Anthropic-style endpoint sends for a finished message."""
    def ev(name: str, data: dict) -> str:
        return f"event: {name}\ndata: {json.dumps(data)}\n\n"

    usage = message["usage"]
    out = [ev("message_start", {"type": "message_start", "message": {
        **message, "content": [], "stop_reason": None,
        "usage": {**usage, "output_tokens": 1}}})]
    for i, block in enumerate(message["content"]):
        if block["type"] == "thinking":
            start = {**block, "thinking": "", "signature": ""}
            deltas = [{"type": "thinking_delta", "thinking": block["thinking"]},
                      {"type": "signature_delta", "signature": block["signature"]}]
        else:
            start = {**block, "text": ""}
            half = len(block["text"]) // 2
            deltas = [{"type": "text_delta", "text": block["text"][:half]},
                      {"type": "text_delta", "text": block["text"][half:]}]
        out.append(ev("content_block_start", {"type": "content_block_start", "index": i,
                                              "content_block": start}))
        out += [ev("content_block_delta", {"type": "content_block_delta", "index": i,
                                           "delta": d}) for d in deltas]
        out.append(ev("content_block_stop", {"type": "content_block_stop", "index": i}))
    out.append(ev("message_delta", {"type": "message_delta",
                                    "delta": {"stop_reason": message["stop_reason"]},
                                    "usage": {"output_tokens": usage["output_tokens"]}}))
    out.append(ev("message_stop", {"type": "message_stop"}))
    return "".join(out)
