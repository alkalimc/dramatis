"""Shared pieces of the persona tests: a synthetic folio, recorded replies, a fake endpoint.

Everything is invented and offline. The folio is written directly (not built from a toy
wiki) so each test can state exactly which units a person has; replies are the recorded
JSON under `fixtures/persona/`, served by an `httpx.MockTransport` that also records every
request it sees.
"""

from __future__ import annotations

import json
import socket
from collections.abc import Callable
from pathlib import Path

import httpx
import pytest

from dramatis_forge.folio import Folio

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "persona"
PLACEHOLDER = "<reader>"
KEY = "sk-test-not-a-real-key"

ENDPOINTS = """
[[profile]]
name = "primary"
base_url = "https://llm.example.invalid/v1"
wire_api = "{wire}"

[[profile.model]]
id = "model-a"
forced_tool = true
cache_breakpoints = 0

[roles]
chat = {{ profile = "primary", model = "model-a" }}
persona = {{ profile = "primary", model = "model-a", reasoning = "high" }}
"""


def recorded(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def output_of(name: str) -> dict:
    """The structured output inside a recorded reply."""
    raw = recorded(name)
    if "choices" in raw:
        return json.loads(raw["choices"][0]["message"]["content"])
    msg = next(i for i in raw["output"] if i["type"] == "message")
    return json.loads(msg["content"][0]["text"])


@pytest.fixture(autouse=True)
def offline(monkeypatch: pytest.MonkeyPatch) -> None:
    """No persona test may open a socket or read a real keychain."""
    def refuse(*_a, **_k):
        raise RuntimeError("tests must not open network connections")

    monkeypatch.setattr(socket, "socket", refuse)
    monkeypatch.setattr(socket, "create_connection", refuse)

    def no_keychain(*_a, **_k):
        raise RuntimeError("tests must not read the keychain")

    import keyring
    monkeypatch.setattr(keyring, "get_password", no_keychain)


class FakeEndpoint:
    """Serves replies in order (the last one repeats) and records every request."""

    def __init__(self, *replies: dict | tuple[int, dict] | Callable[[dict], dict]) -> None:
        self.replies = list(replies)
        self.requests: list[httpx.Request] = []

    @property
    def bodies(self) -> list[dict]:
        return [json.loads(r.content) for r in self.requests]

    def handler(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        reply = self.replies[min(len(self.requests), len(self.replies)) - 1]
        if callable(reply):
            reply = reply(json.loads(request.content))
        status, body = reply if isinstance(reply, tuple) else (200, reply)
        return httpx.Response(status, json=body)

    @property
    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self.handler)


def key_lookup(key: str | None = KEY) -> Callable[[str, str], str | None]:
    seen: list[tuple[str, str]] = []

    def lookup(service: str, account: str) -> str | None:
        seen.append((service, account))
        return key

    lookup.seen = seen  # type: ignore[attr-defined]
    return lookup


def unit(ord_: int, template: str, page: str, title: str, text: str,
         span_of: str | None = None) -> tuple:
    cid = f"{template}:{ord_:04d}"
    return (cid, ord_, template, page, 1, title, "", text, len(text), span_of, ord_, ord_)


def build_folio(path: Path, *, placeholder: str = PLACEHOLDER) -> Path:
    """A small folio in the shape the builder writes.

    - Alice: two forms (`Alice`, `Alice (Winter)`), card, dossier, an archive quote
      (a dossier unit whose title the generator reclassifies), an item description (to
      be dropped), voice lines on both forms, a character note, and story lines under her
      name and under her second form's name.
    - Bob, Carol, Frank, Gil: decreasing material, each with words of their own (voice or
      dialogue). Erin: an identity card only, so no persona. Dan: roster person with none.
    - The host speaks in one scene under the name `Signal`, and one lore unit describes it.
    """
    units = [
        unit(0, "profile", "Alice", "Alice identity card",
             "home: North Cape\nrole: Keeper of the northern light", "Alice#card"),
        unit(1, "profile", "Alice", "Early life",
             "She grew up in the harbour town, the youngest of five.", "Alice#dossier"),
        unit(2, "profile", "Alice", "Quote: patience",
             "The sea keeps its own hours; so do I.", "Alice#dossier"),
        unit(3, "profile", "Alice", "Token description",
             "A brass button. Collect five to exchange for a lamp.", "Alice#dossier"),
        unit(4, "voice", "Alice/voice", "Greeting",
             f"Good evening, {placeholder}. The lamp is lit.", "Alice#voice"),
        unit(5, "voice", "Alice (Winter)/voice", "Greeting",
             "Cold tonight. Stay near the stove.", "Alice (Winter)#voice"),
        unit(6, "profile", "Register", "Alice",
             "A keeper often seen on the cliff path.", "Alice#ref"),
        unit(7, "dialogue", "Chapter 1", "Chapter 1",
             f"The harbour is quiet.\nAlice: Good morning, Dr.{placeholder}.\n"
             "Bob: The boats are late again.\nAlice: Then we wait.", "Chapter 1"),
        unit(8, "dialogue", "Chapter 9", "Chapter 9",
             "Alice (Winter): Snow on the lens again.\nBob: I'll fetch the ladder.",
             "Chapter 9"),
        unit(9, "profile", "Bob", "Bob identity card", "role: Fisherman", "Bob#card"),
        unit(10, "voice", "Bob/voice", "Greeting", "Morning. Nets are dry.", "Bob#voice"),
        unit(11, "profile", "Carol", "Carol identity card", "role: Ferry pilot", "Carol#card"),
        unit(12, "profile", "Erin", "Erin identity card", "role: Clerk", "Erin#card"),
        unit(13, "profile", "Frank", "Frank identity card", "role: Cook", "Frank#card"),
        unit(14, "dialogue", "Chapter 3", "Chapter 3",
             f"Signal: Welcome back, {placeholder}. Two messages are waiting.\n"
             "Carol: Anything for me?\nSignal: One, from the ferry office.", "Chapter 3"),
        unit(15, "lore", "Codex", "Places › Signal",
             "The signal office relays every message in the harbour.", "Codex#Places"),
        unit(16, "voice", "Frank/voice", "Greeting", "Soup's on.", "Frank#voice"),
        unit(17, "profile", "Gil", "Gil identity card", "role: Porter", "Gil#card"),
        unit(18, "voice", "Gil/voice", "Greeting", "Mind the crates.", "Gil#voice"),
    ]
    owners = [("profile:0000", "Alice"), ("profile:0001", "Alice"), ("profile:0002", "Alice"),
              ("profile:0003", "Alice"), ("voice:0004", "Alice"), ("voice:0005", "Alice"),
              ("profile:0006", "Alice"), ("dialogue:0007", "Alice"), ("dialogue:0007", "Bob"),
              ("dialogue:0008", "Alice"), ("dialogue:0008", "Bob"), ("profile:0009", "Bob"),
              ("voice:0010", "Bob"), ("profile:0011", "Carol"), ("dialogue:0014", "Carol"),
              ("profile:0012", "Erin"), ("profile:0013", "Frank"), ("voice:0016", "Frank"),
              ("profile:0017", "Gil"), ("voice:0018", "Gil")]
    persons = [
        ("Alice", "Alice", "Alice", json.dumps([{"page": "Alice", "kind": "canonical"},
                                                {"page": "Alice (Winter)", "kind": "alt"}]),
         "{}", 900, 0.8, None),
        ("Bob", "Bob", "Bob", json.dumps([{"page": "Bob", "kind": "canonical"}]), "{}", 500,
         0.6, None),
        ("Carol", "Carol", "Carol", json.dumps([{"page": "Carol", "kind": "canonical"}]), "{}",
         300, 0.4, None),
        ("Dan", "Dan", "Dan", json.dumps([{"page": "Dan", "kind": "canonical"}]), "{}", 0, 0.0,
         None),
        ("Erin", "Erin", "Erin", json.dumps([{"page": "Erin", "kind": "canonical"}]), "{}", 40,
         0.1, None),
        ("Frank", "Frank", "Frank", json.dumps([{"page": "Frank", "kind": "canonical"}]), "{}",
         60, 0.15, None),
        ("Gil", "Gil", "Gil", json.dumps([{"page": "Gil", "kind": "canonical"}]), "{}", 70,
         0.2, None),
    ]
    with Folio.create(path) as f:
        f.add_chunks(units)
        f.add_unit_persons(owners)
        f.write_persons(persons)
        f.write_aliases([("Ally", "Alice", "redirect"), ("Alice (Winter)", "Alice", "form")])
        f.set_meta("template_shapes", {"profile": "profile", "voice": "voice",
                                       "dialogue": "dialogue", "lore": "lore"})
        if placeholder:
            f.set_meta("user_placeholder", placeholder)
    return path
