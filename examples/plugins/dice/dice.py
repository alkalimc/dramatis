#!/usr/bin/env python3
"""Example plugin for dramatis: speaks the plugin protocol (docs/plugins.md).

Protocol in short: one JSON object per line on stdin and stdout. Requests carry
`id`, `method`, `params`; answer each with the same `id` and `result` or
`error: {"message": ...}`. Notifications have no `id` and get no answer. Stdout is for
the protocol only; write logs to stderr.
"""
import json
import random
import re
import sys

EMAIL = re.compile(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+")
seen = 0  # messages this process has been told about (a plugin may keep state)


def initialize(params):
    return {}


def prompt(params):
    # Lines for the turn's harness entry. Keep them short; the host cuts them anyway.
    if seen == 0:
        return {"lines": []}
    return {"lines": [f"Note from the dice plugin: {seen} messages have been posted so far."]}


def tool(params):
    # A tool the manifest declares. `tool` is the plugin-local name.
    if params["tool"] != "roll":
        raise ValueError("unknown tool")
    args = params["arguments"]
    sides = int(args["sides"])
    count = int(args.get("count", 1))
    if not 2 <= sides <= 1000 or not 1 <= count <= 20:
        raise ValueError("sides must be 2 to 1000 and count 1 to 20")
    dice = [random.randint(1, sides) for _ in range(count)]
    return {"output": f"{' '.join(map(str, dice))} (total {sum(dice)})"}


def tool_result(params):
    # Post-process any tool's result before the model reads it. Returning no `output`
    # leaves the result as it was.
    masked = EMAIL.sub("[address]", params["output"])
    if masked == params["output"]:
        return {}
    return {"output": masked}


def message(params):
    global seen
    seen += 1


METHODS = {"initialize": initialize, "prompt": prompt, "tool": tool, "tool_result": tool_result}


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stdin.reconfigure(encoding="utf-8")
    for line in sys.stdin:
        if not line.strip():
            continue
        req = json.loads(line)
        method, params = req.get("method"), req.get("params") or {}
        if "id" not in req:  # notification
            if method == "message":
                message(params)
            continue
        try:
            reply = {"id": req["id"], "result": METHODS[method](params)}
        except Exception as e:  # report, do not crash
            reply = {"id": req["id"], "error": {"message": str(e)}}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
