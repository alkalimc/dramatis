"""`forge persona`: endpoints file, keychain, wire APIs, material, gates, resume, probe.

Offline throughout: replies are recorded fixtures served by an `httpx.MockTransport`,
the key lookup is injected, and the real `keyring` and sockets are patched to raise.
"""

from __future__ import annotations

import json
import re
from dataclasses import replace
from pathlib import Path

import httpx
import pytest
from persona_support import (
    ENDPOINTS,
    KEY,
    PLACEHOLDER,
    FakeEndpoint,
    build_folio,
    key_lookup,
    offline,  # noqa: F401  (autouse fixture)
    output_of,
    recorded,
)

from dramatis_forge.config import Paths
from dramatis_forge.persona import generator as generator_mod
from dramatis_forge.persona.client import CallError, Client, parse_output, usage_of
from dramatis_forge.persona.command import UsageError, execute
from dramatis_forge.persona.endpoints import (
    Endpoints,
    EndpointsError,
    default_path,
    persona_target,
)
from dramatis_forge.persona.gates import META, PRESCRIPTIVE, STRUCTURE, Lexicons, check
from dramatis_forge.persona.generator import DEFAULT, Generator, HostSource
from dramatis_forge.persona import stale as stale_mod
from dramatis_forge.persona.material import (
    Example,
    estimate_tokens,
    for_host,
    for_person,
    select_examples,
    speech_units,
)
from dramatis_forge.persona.params import Persona
from dramatis_forge.persona.store import RawCache
from dramatis_forge.persona.run import (
    FAILED,
    RESTORED,
    RESUMED,
    SKIPPED,
    WRITTEN,
    ineligible,
    probe_set,
)

GEN = replace(
    DEFAULT,
    label="test",
    host_system=generator_mod._DEFAULT_HOST_SYSTEM,
    host=HostSource(names=("Signal",), titles=("Signal",)),
    title_kinds=(("dossier", r"Quote: .*", "quote"), ("dossier", r"Token description", "drop")),
    user_name_patterns=(r"\bDr\.\s?(?!<reader>)[A-Z][a-z]+",),
)


@pytest.fixture
def paths(tmp_path: Path) -> Paths:
    p = Paths.for_pack("toy", home=tmp_path / "home").ensure()
    build_folio(p.folio)
    return p


@pytest.fixture
def endpoints_file(tmp_path: Path):
    def write(wire: str = "responses") -> Path:
        path = tmp_path / f"endpoints-{wire}.toml"
        path.write_text(ENDPOINTS.format(wire=wire), encoding="utf-8")
        return path
    return write


@pytest.fixture
def toy():
    from dramatis_forge.pack import load_pack
    return load_pack("toy")


def run(toy, paths, endpoints_file, endpoint: FakeEndpoint, *, wire: str = "responses",
        gen: Generator = GEN, **kw):
    kw.setdefault("lookup", key_lookup())
    return execute(toy, paths, endpoints_path=endpoints_file(wire), transport=endpoint.transport,
                   counter=estimate_tokens, generator=gen,
                   params_path=paths.home / "none.toml", **kw)


def rows(paths: Paths) -> dict[tuple[str, str], tuple[str, str]]:
    import sqlite3
    db = sqlite3.connect(paths.folio)
    try:
        return {(s, sl): (b, v) for s, sl, b, v in db.execute(
            "SELECT subject, slot, body, generator_version FROM prompts")}
    finally:
        db.close()


# --------------------------------------------------------------------------- #
# endpoints file and keychain
# --------------------------------------------------------------------------- #


def test_endpoints_mirror_the_engine_shape():
    e = Endpoints.from_toml(ENDPOINTS.format(wire="responses"))
    assert e.profiles[0].wire_api == "responses"
    assert e.profiles[0].models[0].forced_tool is True
    assert e.roles.persona.reasoning == "high" and e.roles.chat.reasoning is None
    assert Endpoints.from_toml("") == Endpoints()


def test_endpoints_accept_an_optional_host_role():
    text = ("[[profile]]\nname='a'\nbase_url='x'\nwire_api='chat'\n[roles]\n"
            "chat={profile='a', model='m'}\nhost={profile='a', model='h'}\n")
    assert Endpoints.from_toml(text).roles.host.model == "h"
    with pytest.raises(EndpointsError, match="not defined"):
        Endpoints.from_toml(text.replace("host={profile='a'", "host={profile='b'"))


@pytest.mark.parametrize("text, message", [
    ("[[profile]]\nname='a'\nbase_url='x'\nwire_api='chat'\napi_key='k'\n", "unknown key"),
    ("[[profile]]\nname='a'\nbase_url='x'\nwire_api='grpc'\n", "wire_api"),
    ("[[profile]]\nname='a'\nbase_url='x'\nwire_api='chat'\n" * 2, "twice"),
    ("[roles]\nchat = { profile = 'nope', model = 'm' }\n", "not defined"),
    ("[[profile]]\nname='a'\nbase_url='x'\nwire_api='chat'\n[[profile.model]]\nid='m'\n"
     "context_window='big'\n", "wrong type"),
])
def test_endpoints_reject_what_the_engine_rejects(text, message):
    with pytest.raises(EndpointsError, match=message):
        Endpoints.from_toml(text)


def test_endpoints_path_follows_dramatis_home(monkeypatch, tmp_path):
    monkeypatch.setenv("DRAMATIS_HOME", str(tmp_path))
    assert default_path() == tmp_path / "endpoints.toml"
    with pytest.raises(EndpointsError) as exc:
        Endpoints.load()
    assert "--endpoints" in exc.value.hint


def test_key_comes_from_the_keychain_account_only(monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "from-env")  # must be ignored
    e = Endpoints.from_toml(ENDPOINTS.format(wire="responses"))
    lookup = key_lookup()
    target = persona_target(e, lookup)
    assert target.key == KEY and lookup.seen == [("dramatis", "profile.primary")]
    assert KEY not in repr(target)


def test_missing_key_names_the_fix():
    e = Endpoints.from_toml(ENDPOINTS.format(wire="responses"))
    with pytest.raises(EndpointsError) as exc:
        persona_target(e, key_lookup(None))
    assert exc.value.hint.endswith(
        "security add-generic-password -s dramatis -a profile.primary -w")


def test_missing_persona_role_is_a_usage_error(toy, paths, tmp_path):
    path = tmp_path / "e.toml"
    path.write_text(ENDPOINTS.format(wire="chat").replace('persona = {', '# persona = {'))
    with pytest.raises(UsageError, match="no `persona` role"):
        execute(toy, paths, endpoints_path=path, lookup=key_lookup())


# --------------------------------------------------------------------------- #
# client: both wire APIs
# --------------------------------------------------------------------------- #


def client(endpoint: FakeEndpoint, wire: str, retries: int = 3) -> Client:
    target = persona_target(Endpoints.from_toml(ENDPOINTS.format(wire=wire)), key_lookup())
    return Client(target, retries=retries, transport=endpoint.transport, wait=lambda _s: 0)


def test_responses_request_shape_and_usage():
    ep = FakeEndpoint(recorded("responses_ok.json"))
    with client(ep, "responses") as c:
        reply = c.call("sys", "user", GEN.schema(False))
    req = ep.requests[0]
    assert req.url == "https://llm.example.invalid/v1/responses"
    assert req.headers["authorization"] == f"Bearer {KEY}"
    body = ep.bodies[0]
    assert body["store"] is False and body["reasoning"] == {"effort": "high"}
    assert body["input"][0] == {"role": "system", "content": "sys"}
    fmt = body["text"]["format"]
    assert fmt["type"] == "json_schema" and fmt["strict"] is True
    assert fmt["schema"]["required"] == list(generator_mod.PERSON_FIELDS)
    assert reply.output == output_of("responses_ok.json")
    assert reply.usage.as_dict() == {"input": 1200, "cached": 0, "output": 310, "reasoning": 180}


def test_chat_request_shape_and_usage():
    ep = FakeEndpoint(recorded("chat_ok.json"))
    with client(ep, "chat") as c:
        reply = c.call("sys", "user", GEN.schema(True))
    assert ep.requests[0].url == "https://llm.example.invalid/v1/chat/completions"
    body = ep.bodies[0]
    assert body["reasoning_effort"] == "high" and "store" not in body
    assert body["response_format"]["json_schema"]["schema"]["required"][-2:] == ["in_world", "meta"]
    assert reply.usage.cached == 128 and reply.usage.output == 250


def test_fenced_output_is_accepted():
    assert parse_output(recorded("chat_fenced.json")["choices"][0]["message"]["content"]) \
        == output_of("chat_ok.json")


def test_transient_failures_retry_and_are_billed():
    ep = FakeEndpoint((503, {"error": {"message": "busy"}}), recorded("responses_incomplete.json"),
                      recorded("responses_ok.json"))
    with client(ep, "responses") as c:
        reply = c.call("s", "u", GEN.schema(False))
    assert reply.attempts == 3 and len(ep.requests) == 3
    assert reply.wasted.input == 1200  # the incomplete reply was billed


def test_retries_are_bounded():
    ep = FakeEndpoint((503, {"error": {"message": "slow down"}}))
    with client(ep, "chat", retries=2) as c, pytest.raises(CallError, match="2 attempt"):
        c.call("s", "u", GEN.schema(False))
    assert len(ep.requests) == 2


def test_client_errors_are_final():
    ep = FakeEndpoint((401, {"error": {"message": "bad key"}}))
    with client(ep, "responses") as c, pytest.raises(CallError, match="401: bad key"):
        c.call("s", "u", GEN.schema(False))
    assert len(ep.requests) == 1


def test_usage_reads_both_shapes():
    assert usage_of(recorded("chat_ok.json"), "chat").input == 900
    assert usage_of({}, "responses").as_dict() == {"input": 0, "cached": 0, "output": 0,
                                                   "reasoning": 0}


# --------------------------------------------------------------------------- #
# material
# --------------------------------------------------------------------------- #


def open_db(paths: Paths):
    import sqlite3
    return sqlite3.connect(paths.folio)


def test_material_is_the_union_over_forms_tagged_by_form(paths):
    db = open_db(paths)
    mat = for_person(db, "Alice", GEN, Persona(), estimate_tokens, ": ")
    kinds = {(i.kind, i.form_page) for i in mat.items}
    assert ("voice", "Alice (Winter)") in kinds and ("voice", "Alice") in kinds
    assert ("quote", "Alice") in kinds and ("char_ref", "Alice") in kinds
    assert all("brass button" not in i.text for i in mat.items)  # dropped by title
    story = [(s.form_page, s.text) for s in mat.story]
    assert ("Alice (Winter)", "Snow on the lens again.") in story
    assert ("Alice", f"Good morning, Dr.{PLACEHOLDER}.") in story
    assert all("boats" not in t for _f, t in story)  # other speakers are not her lines
    text = mat.render(GEN, ": ")
    assert "[Alice (Winter)] Greeting: Cold tonight." in text
    assert PLACEHOLDER in text  # kept verbatim for runtime substitution
    assert text.index("## Identity card") < text.index("## Voice lines")


def test_story_lines_carry_what_they_answer(paths):
    db = open_db(paths)
    mat = for_person(db, "Alice", GEN, Persona(), estimate_tokens, ": ")
    before = {s.text: s.before for s in mat.story}
    assert before["Good morning, Dr.<reader>."] == "The harbour is quiet."
    assert before["Then we wait."] == "Bob: The boats are late again."
    assert before["Snow on the lens again."] == ""  # first line of its unit
    text = mat.render(GEN, ": ")
    assert "  Bob: The boats are late again.\n[Alice] Then we wait." in text
    line = next(s for s in mat.story if s.text == "Then we wait.")
    assert line.tokens == estimate_tokens(line.before + line.text)  # context is billed


def test_line_drops_remove_matching_lines_only(paths):
    db = open_db(paths)
    gen = replace(GEN, line_drops=(r"home: .*",))
    card = next(i for i in for_person(db, "Alice", gen, Persona(), estimate_tokens, ": ").items
                if i.kind == "card")
    assert card.text == "role: Keeper of the northern light"
    assert gen.version(model="m", reasoning=None, params=Persona()) != GEN.version(
        model="m", reasoning=None, params=Persona())


def test_material_budget_drops_and_counts(paths):
    db = open_db(paths)
    tight = Persona(material_tokens=30, story_tokens=8)
    mat = for_person(db, "Alice", GEN, tight, estimate_tokens, ": ")
    assert mat.tokens <= 30
    assert mat.dropped and mat.story_found == 3 and len(mat.story) < 3


def test_budget_fills_identity_and_voice_before_dossier_and_keeps_what_fits(paths):
    db = open_db(paths)
    full = for_person(db, "Alice", GEN, Persona(material_tokens=10**6, story_tokens=0),
                      estimate_tokens, ": ")
    own = sum(i.tokens for i in full.items if i.kind in ("card", "voice"))
    assert own < full.tokens
    mat = for_person(db, "Alice", GEN, Persona(material_tokens=own, story_tokens=0),
                     estimate_tokens, ": ")
    assert {i.kind for i in mat.items} == {"card", "voice"}
    assert mat.dropped.get("quote") and mat.dropped.get("char_ref") and not mat.story


def test_story_budget_is_reserved_before_the_items(paths):
    db = open_db(paths)
    p = Persona(material_tokens=60, story_tokens=15)
    mat = for_person(db, "Alice", GEN, p, estimate_tokens, ": ")
    assert mat.story and sum(s.tokens for s in mat.story) <= 15 and mat.tokens <= 60


def test_thin_people_send_everything_under_the_default_budget(paths):
    db = open_db(paths)
    for person in ("Bob", "Carol", "Frank", "Gil", "Alice"):
        mat = for_person(db, person, GEN, Persona(), estimate_tokens, ": ")
        assert not mat.dropped, person
    assert Persona().material_tokens == 5000 and Persona().story_tokens == 1500


def test_material_names_the_units_it_was_made_from(paths):
    db = open_db(paths)
    p = Persona(material_tokens=40, story_tokens=0)
    mat = for_person(db, "Alice", GEN, p, estimate_tokens, ": ")
    assert mat.sent and mat.sent < mat.attributed  # something was left out for budget
    assert "voice:0004" in mat.attributed and "profile:0003" not in mat.attributed  # dropped
    everything = for_person(db, "Alice", GEN, Persona(), estimate_tokens, ": ")
    assert {"dialogue:0007", "dialogue:0008"} <= everything.sent
    assert "dialogue:0007" in everything.attributed


def test_host_material_is_found_by_name(paths):
    db = open_db(paths)
    mat = for_host(db, GEN.host, GEN, Persona(), estimate_tokens, ": ")
    assert [s.text for s in mat.story] == [
        f"Welcome back, {PLACEHOLDER}. Two messages are waiting.", "One, from the ferry office."]
    assert [i.kind for i in mat.items] == ["lore"]
    assert {s.form_page for s in mat.story} == {"Signal"}
    assert mat.story[1].before == "Carol: Anything for me?"


def test_estimate_counts_wide_characters_one_each():
    assert estimate_tokens("abcdefgh") == 2
    assert estimate_tokens("\u4e00\u4e8c\u4e09") == 3  # three wide characters
    assert estimate_tokens("") == 0


def test_probe_set_is_thickest_three_then_thinnest_two_eligible(paths):
    # Erin has the lowest confidence but only a card: the thin end skips her.
    assert probe_set(open_db(paths), Persona()) == ["Alice", "Bob", "Carol", "Frank", "Gil"]
    # With the rule off she is back, and Dan (no material at all) still is not.
    assert probe_set(open_db(paths), Persona(min_own_units=0)) == [
        "Alice", "Bob", "Carol", "Erin", "Frank"]


def test_eligibility_counts_voice_and_dialogue_units_by_shape(paths):
    db = open_db(paths)
    assert speech_units(db) == {"Alice": 4, "Bob": 3, "Carol": 1, "Frank": 1, "Gil": 1}
    skip = ineligible(db, Persona())
    assert sorted(skip) == ["Dan", "Erin"]
    assert "0 voice or dialogue unit(s)" in skip["Erin"] and "min_own_units 1" in skip["Erin"]
    # A stricter floor, and templates found by declared shape, not by name.
    assert sorted(ineligible(db, Persona(min_own_units=2))) == [
        "Carol", "Dan", "Erin", "Frank", "Gil"]
    db.execute("UPDATE manifest SET value=? WHERE key='template_shapes'",
               (json.dumps({"profile": "profile", "voice": "lore", "dialogue": "dialogue"}),))
    assert speech_units(db) == {"Alice": 2, "Bob": 2, "Carol": 1}


# --------------------------------------------------------------------------- #
# gates
# --------------------------------------------------------------------------- #


def judge(output: dict, host: bool = False):
    return check(output, host=host, gen=GEN, lex=Lexicons(GEN, PLACEHOLDER), p=Persona(),
                 count=estimate_tokens)


def test_good_output_passes():
    v = judge(output_of("responses_ok.json"))
    assert v.passed, v.reasons()
    assert v.tokens["system_prompt"] > 0


def test_prescriptive_phrasing_fails_gate_six():
    v = judge(output_of("responses_prescriptive.json"))
    assert v.count(PRESCRIPTIVE) == 3 and v.count(META) == 0
    assert all(f.slot == "system_prompt" for f in v.findings)


def test_quoted_examples_are_not_prescriptive_but_can_still_leak():
    base = output_of("responses_ok.json")
    quoted = {**base, "tone_rules": [*base["tone_rules"],
                                     'Cuts a topic off: "Never bring that up again."']}
    assert judge(quoted).passed
    bare = {**base, "tone_rules": [*base["tone_rules"], "Never bring up the war."]}
    assert judge(bare).count(PRESCRIPTIVE) == 1
    leak = {**base, "tone_rules": [*base["tone_rules"], 'Says "this game is rigged".']}
    assert judge(leak).count(META) == 1


def test_meta_leak_fails_gate_two():
    v = judge(output_of("responses_meta_leak.json"))
    assert v.count(META) == 1 and v.findings[0].slot == "tone_rules"


def test_host_meta_layer_may_use_meta_words_but_nothing_else_may():
    out = output_of("responses_host.json")
    assert judge(out, host=True).passed
    leaky = dict(out, in_world=out["in_world"] + " It runs on an endpoint.")
    v = judge(leaky, host=True)
    assert [(f.gate, f.slot) for f in v.findings] == [(META, "in_world")]


def test_ascii_meta_terms_match_whole_words_only():
    out = dict(output_of("responses_ok.json"), capability_line="Rapid tides; gamekeeper")
    assert judge(out).passed


@pytest.mark.parametrize("change, detail", [
    ({"tone_rules": ["one", "two"]}, "outside persona.tone_rules"),
    ({"capability_line": "x" * 41}, "persona.capability_chars"),
    ({"capability_line": "  "}, "missing, empty"),
    ({"system_prompt": "word " * 3000}, "persona.prompt_tokens"),
    ({"extra": "field"}, "unexpected field"),
])
def test_structure_and_budget_gate_five(change, detail):
    v = judge({**output_of("responses_ok.json"), **change})
    assert v.count(STRUCTURE) >= 1 and any(detail in r for r in v.reasons())


def test_missing_host_layers_fail_gate_five():
    v = judge(output_of("responses_ok.json"), host=True)
    assert {f.slot for f in v.findings} == {"in_world", "meta"}


@pytest.mark.parametrize("text", [
    "Good morning, Dr.Harbour.",       # an invented name
    "Good morning, <Reader>.",         # an altered placeholder
    "Good morning, reader.",           # the placeholder's word without its brackets
])
def test_user_is_named_only_by_the_placeholder(text):
    v = judge({**output_of("responses_ok.json"), "capability_line": text})
    assert v.count(STRUCTURE) == 1 and "names the user" in v.reasons()[0]


def test_placeholder_itself_passes():
    out = {**output_of("responses_ok.json"), "capability_line": f"Ask Dr.{PLACEHOLDER}."}
    assert judge(out).passed


# --------------------------------------------------------------------------- #
# whole runs
# --------------------------------------------------------------------------- #


def test_run_writes_passing_people_and_reports_the_rest(toy, paths, endpoints_file):
    replies = {"Alice": "responses_ok.json", "Bob": "responses_prescriptive.json",
               "Carol": "responses_meta_leak.json", "Frank": "responses_ok.json",
               "Gil": "responses_ok.json"}

    def reply(body: dict) -> dict:
        user = body["input"][1]["content"]
        subject = user.split("\n", 1)[0].removeprefix("Person: ")
        return recorded(replies.get(subject, "responses_host.json"))

    ep = FakeEndpoint(reply)
    result = run(toy, paths, endpoints_file, ep)
    rep = result.report
    status = {o.subject: o.status for o in rep.outcomes}
    assert status == {"Alice": WRITTEN, "Bob": FAILED, "Carol": FAILED, "Dan": SKIPPED,
                      "Erin": SKIPPED, "Frank": WRITTEN, "Gil": WRITTEN, "host": WRITTEN}
    assert len(ep.requests) == 6  # one call per eligible subject; none for Dan or Erin
    assert rep.meta_leak == 1 and rep.prescriptive_rate == round(1 / 6, 4)
    assert rep.missing == ["Bob", "Carol", "Dan", "Erin"]
    assert [o.subject for o in rep.skipped] == ["Dan", "Erin"]

    r = rows(paths)
    assert r[("Alice", "system")][0].startswith("You are Alice")
    assert r[("Alice", "tone")][0].count("\n") == 2
    assert r[("Alice", "system")][1] == rep.version
    assert ("Bob", "system") not in r
    assert {sl for (s, sl) in r if s == "host"} >= {"system", "tone", "capability",
                                                     "in_world", "meta"}

    # The prompt the model saw keeps the placeholder and states the budgets.
    sent = ep.bodies[0]["input"]
    assert PLACEHOLDER in sent[1]["content"] and PLACEHOLDER in sent[0]["content"]
    assert "3 to 4" in sent[0]["content"] and "{" not in sent[0]["content"].split("Fields:")[1]

    report = json.loads((paths.pack_dir / "persona" / "report.json").read_text())
    assert report["figures"] == {"persona.meta_leak": 1, "prescriptive.rate": round(1 / 6, 4)}
    assert any("prescriptive" in r for s in report["subjects"] if s["subject"] == "Bob"
               for r in s["reasons"])
    erin = next(s for s in report["subjects"] if s["subject"] == "Erin")
    assert erin["status"] == SKIPPED and "min_own_units" in erin["reasons"][0]
    md = (paths.pack_dir / "persona" / "REPORT.md").read_text()
    assert "| Erin | skipped |" in md and "skipped, no words of their own: 2" in md
    assert KEY not in json.dumps(report)


def test_resume_skips_current_rows_and_restores_from_the_cache(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice", "Ally"])  # alias resolves to Alice
    assert len(ep.requests) == 1

    again = run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    assert [o.status for o in again.report.outcomes] == [RESUMED]
    assert len(ep.requests) == 1  # no call, and no key needed

    # `forge build` recreates the folio: the raw cache restores the rows without a call.
    build_folio(paths.folio)
    restored = run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    assert [o.status for o in restored.report.outcomes] == [RESTORED]
    assert ("Alice", "system") in rows(paths) and len(ep.requests) == 1

    forced = run(toy, paths, endpoints_file, ep, only=["Alice"], force=True)
    assert [o.status for o in forced.report.outcomes] == [WRITTEN] and len(ep.requests) == 2


def test_a_new_generator_version_regenerates(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    first = run(toy, paths, endpoints_file, ep, only=["Alice"]).report.version
    edited = replace(GEN, system=GEN.system + "\n")
    second = run(toy, paths, endpoints_file, ep, only=["Alice"], gen=edited).report.version
    assert first != second and len(ep.requests) == 2
    assert {v for (s, _sl), (_b, v) in rows(paths).items() if s == "Alice"} == {second}


def test_a_failing_regeneration_removes_the_old_rows(toy, paths, endpoints_file):
    run(toy, paths, endpoints_file, FakeEndpoint(recorded("responses_ok.json")), only=["Alice"])
    run(toy, paths, endpoints_file, FakeEndpoint(recorded("responses_prescriptive.json")),
        only=["Alice"], force=True)
    assert not any(s == "Alice" for s, _sl in rows(paths))


def test_call_errors_leave_rows_alone_and_exit_nonzero(toy, paths, endpoints_file):
    run(toy, paths, endpoints_file, FakeEndpoint(recorded("responses_ok.json")), only=["Alice"])
    ep = FakeEndpoint((400, {"error": {"message": "bad request"}}))
    rep = run(toy, paths, endpoints_file, ep, only=["Alice"], force=True).report
    assert rep.outcomes[0].status == "error" and "400" in rep.outcomes[0].reasons[0]
    assert ("Alice", "system") in rows(paths)


def message_of(output: dict) -> dict:
    """An Anthropic-style reply carrying `output` as the structured answer."""
    return {"id": "msg_1", "type": "message", "role": "assistant", "model": "model-a",
            "stop_reason": "end_turn", "stop_sequence": None,
            "content": [{"type": "thinking", "thinking": "Considering.", "signature": "sig=="},
                        {"type": "text", "text": json.dumps(output)}],
            "usage": {"input_tokens": 100, "cache_read_input_tokens": 50,
                      "cache_creation_input_tokens": 10, "output_tokens": 300}}


def test_messages_wire_run_and_request_shape(toy, paths, endpoints_file):
    ep = FakeEndpoint(message_of(output_of("responses_ok.json")))
    result = run(toy, paths, endpoints_file, ep, wire="messages", only=["Alice"])
    outcome = result.report.outcomes[0]
    assert outcome.status == WRITTEN
    assert outcome.usage.as_dict() == {"input": 160, "cached": 50, "output": 300, "reasoning": 0}
    req = ep.requests[0]
    assert str(req.url) == "https://llm.example.invalid/v1/messages"
    assert req.headers["x-api-key"] == KEY and "authorization" not in req.headers
    body = ep.bodies[0]
    assert body["thinking"] == {"type": "adaptive"}
    assert body["output_config"]["effort"] == "high"
    assert body["output_config"]["format"]["type"] == "json_schema"
    assert body["messages"][0]["role"] == "user" and PLACEHOLDER in body["system"]
    assert ("Alice", "system") in rows(paths)


def test_messages_wire_refusal_is_final(toy, paths, endpoints_file):
    refusal = {**message_of({}), "stop_reason": "refusal"}
    ep = FakeEndpoint(refusal)
    rep = run(toy, paths, endpoints_file, ep, wire="messages", only=["Alice"]).report
    assert rep.outcomes[0].status == "error" and "refused" in rep.outcomes[0].reasons[0]
    assert len(ep.requests) == 1


def test_messages_probe_records_the_sdk_arguments(toy, paths, endpoints_file):
    ep = FakeEndpoint(message_of(output_of("responses_ok.json")))
    run(toy, paths, endpoints_file, ep, wire="messages", probe=True)
    raw = json.loads(next((paths.pack_dir / "persona" / "probe").glob("Alice-*.json")).read_text())
    assert raw["request"]["path"] == "messages"
    assert raw["request"]["body"]["thinking"] == {"type": "adaptive"}
    assert KEY not in json.dumps(raw)


def test_endpoints_accept_messages_and_cache_write_price():
    e = Endpoints.from_toml(ENDPOINTS.format(wire="messages").replace(
        "forced_tool = true", "forced_tool = false\nprice_in_cache_write = 1.25"))
    assert e.profiles[0].wire_api == "messages"
    assert e.profiles[0].models[0].price_in_cache_write == 1.25


def test_request_billing_selects_the_larger_budgets():
    e = Endpoints.from_toml(ENDPOINTS.format(wire="chat").replace(
        "forced_tool = true", "forced_tool = true\nbilling = \"request\""))
    role = e.roles.persona
    assert e.profiles[0].models[0].billing == "request" and e.per_request(role)
    p = Persona().billed(e.per_request(role))
    assert (p.material_tokens, p.story_tokens) == (10000, 5000)
    assert Persona().billed(False) == Persona()
    token = Endpoints.from_toml(ENDPOINTS.format(wire="chat"))
    assert not token.per_request(token.roles.persona)
    with pytest.raises(EndpointsError):
        Endpoints.from_toml(ENDPOINTS.format(wire="chat").replace(
            "forced_tool = true", "billing = \"free\""))


def test_endpoints_accept_emulated_tools():
    e = Endpoints.from_toml(ENDPOINTS.format(wire="chat").replace(
        "forced_tool = true", "forced_tool = true\nnative_tools = false"))
    assert e.profiles[0].models[0].native_tools is False
    with pytest.raises(EndpointsError):
        Endpoints.from_toml(ENDPOINTS.format(wire="chat").replace(
            "forced_tool = true", "native_tools = \"no\""))


def test_probe_writes_readable_files(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("chat_ok.json"))
    result = run(toy, paths, endpoints_file, ep, wire="chat", probe=True)
    assert [o.subject for o in result.report.outcomes] == ["Alice", "Bob", "Carol", "Frank",
                                                            "Gil"]
    probe = paths.pack_dir / "persona" / "probe"
    md = sorted(probe.glob("*.md"))
    assert len(md) == 5 and len(list(probe.glob("*.json"))) == 5
    alice = next(p for p in md if p.name.startswith("Alice-")).read_text()
    for part in ("## Inputs", "## Outputs", "## Gates", "### system_prompt", "- pass",
                 "Alice (Winter)", "sampled of"):
        assert part in alice
    raw = json.loads(next(probe.glob("Alice-*.json")).read_text())
    assert raw["request"]["path"] == "chat/completions"
    assert raw["response"]["usage"]["prompt_tokens"] == 900
    assert KEY not in json.dumps(raw)
    assert (paths.pack_dir / "persona" / "PROBE_REPORT.md").exists()


def test_only_rejects_unknown_names(toy, paths, endpoints_file):
    with pytest.raises(UsageError, match="no person named"):
        run(toy, paths, endpoints_file, FakeEndpoint(), only=["Nobody"])
    with pytest.raises(UsageError, match="does not generate the host"):
        run(toy, paths, endpoints_file, FakeEndpoint(), only=["host"], gen=DEFAULT)


def test_only_refuses_a_person_without_words_of_their_own(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    with pytest.raises(UsageError, match="Erin gets no persona: 0 voice or dialogue") as exc:
        run(toy, paths, endpoints_file, ep, only=["Alice", "Erin"])
    assert "min_own_units" in exc.value.hint and not ep.requests


def test_a_skipped_person_loses_rows_an_older_rule_wrote(toy, paths, endpoints_file, tmp_path):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    off = tmp_path / "params.toml"
    off.write_text("[persona]\nmin_own_units = 0\n")
    execute(toy, paths, endpoints_path=endpoints_file("responses"), transport=ep.transport,
            counter=estimate_tokens, generator=GEN, params_path=off, lookup=key_lookup(),
            only=["Erin"])
    assert ("Erin", "system") in rows(paths)
    rep = run(toy, paths, endpoints_file, ep, gen=DEFAULT).report
    assert {o.subject: o.status for o in rep.outcomes}["Erin"] == SKIPPED
    assert not any(s == "Erin" for s, _sl in rows(paths)) and len(ep.requests) == 6  # Erin once, then 5 eligible


def test_the_default_generator_without_host_skips_it(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    rep = run(toy, paths, endpoints_file, ep, gen=DEFAULT).report
    assert "host" not in {o.subject for o in rep.outcomes}


def test_params_override_from_toml(tmp_path):
    path = tmp_path / "params.toml"
    path.write_text("[persona]\nprompt_tokens = 500\ntone_rules = [2, 6]\nunknown = 1\n")
    p = Persona.load(path)
    assert p.prompt_tokens == 500 and p.tone_rules == (2, 6) and p.capability_chars == 40
    assert p.min_own_units == 1
    assert Persona.load(tmp_path / "missing.toml") == Persona()


def test_no_network_even_without_the_mock():
    with pytest.raises(RuntimeError, match="network"):
        httpx.get("https://llm.example.invalid/v1/models")


# --------------------------------------------------------------------------- #
# staleness
# --------------------------------------------------------------------------- #


def edit_unit(paths: Paths, old_id: str, new_id: str, text: str) -> None:
    """What a rebuilt folio looks like when a unit's text changed: its id is a content
    hash, so the old id is gone and a new one stands in its place."""
    import sqlite3
    db = sqlite3.connect(paths.folio)
    try:
        with db:
            db.execute("UPDATE chunks SET id=?, text=? WHERE id=?", (new_id, text, old_id))
            db.execute("UPDATE unit_persons SET chunk_id=? WHERE chunk_id=?", (new_id, old_id))
    finally:
        db.close()


def add_alice_line(paths: Paths) -> None:
    import sqlite3
    db = sqlite3.connect(paths.folio)
    try:
        with db:
            db.execute(
                "INSERT INTO chunks(id, ord, template, page, revid, title, header, text, chars, "
                "span_of, span_from, span_to) VALUES('dialogue:0099', 99, 'dialogue', 'Chapter 7', "
                "1, 'Chapter 7', '', 'Alice: A new line.', 18, 'Chapter 7', 99, 99)")
            db.execute("INSERT INTO unit_persons(chunk_id, person_id) VALUES('dialogue:0099', "
                       "'Alice')")
    finally:
        db.close()


def stale_run(toy, paths, endpoints_file, ep, **kw):
    return run(toy, paths, endpoints_file, ep, stale=True, **kw)


def test_a_written_persona_records_what_it_was_made_from(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice"])
    entry = RawCache(paths.pack_dir / "persona" / "raw").entry("Alice")
    assert entry is not None
    source = entry["source"]
    assert "voice:0004" in source["sent"] and set(source["sent"]) <= set(source["attributed"])


def test_unchanged_material_is_not_stale(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice"])
    build_folio(paths.folio)  # a rebuild from an unchanged corpus
    res = stale_run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    assert res.stale is not None
    assert (res.stale.stale, res.stale.new_only, res.stale.current) == ([], [], 1)
    assert len(ep.requests) == 1  # listing calls nothing
    md = (paths.pack_dir / "persona" / "STALE.md").read_text()
    assert "stale: 0" in md and res.written == [paths.pack_dir / "persona" / "STALE.md"]


def test_a_changed_sent_unit_makes_the_persona_stale(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice"])
    build_folio(paths.folio)
    edit_unit(paths, "voice:0004", "voice:0004x", "Good evening. The lamp is out.")
    res = stale_run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    row = res.stale.stale[0]
    assert (row.subject, row.changed, res.stale.new_only) == ("Alice", 1, [])
    assert "| Alice |" in (paths.pack_dir / "persona" / "STALE.md").read_text()


def test_only_new_units_is_the_weak_signal_listed_apart(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice"])
    build_folio(paths.folio)
    add_alice_line(paths)
    res = stale_run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    assert res.stale.stale == [] and res.stale.to_regenerate == []
    assert [(r.subject, r.changed, r.new) for r in res.stale.new_only] == [("Alice", 0, 1)]


def test_a_changed_unit_that_was_never_sent_does_not_count(paths, tmp_path):
    db = open_db(paths)
    cache = RawCache(tmp_path / "raw")
    mat = for_person(db, "Alice", GEN, Persona(), estimate_tokens, ": ")
    sent = sorted(mat.sent)
    cache.save("Alice", "v1", {}, {}, {}, {"sent": sent, "attributed": sent + ["profile:0abc"]})
    material_of = stale_mod.current_material(db, GEN, ": ")
    res = stale_mod.check(db, cache, "v1", ["Alice"], material_of)
    assert res.current == 1 and not res.stale  # "profile:0abc" is not in the folio, unsent


def test_never_generated_no_record_and_older_versions_are_told_apart(paths, tmp_path):
    db = open_db(paths)
    cache = RawCache(tmp_path / "raw")
    cache.save("Alice", "v1", {}, {}, {}, None)  # written before sources were recorded
    cache.save("Bob", "v0", {}, {}, {}, {"sent": [], "attributed": []})
    material_of = stale_mod.current_material(db, GEN, ": ")
    res = stale_mod.check(db, cache, "v1", ["Alice", "Bob", "Carol", "Dan"], material_of)
    assert res.no_record == ["Alice"] and res.older_version == 1
    assert res.never == ["Carol"]  # Dan has no material at all, so there is nothing to make


def test_regenerate_calls_only_the_stale_ones(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice", "Frank"])
    assert len(ep.requests) == 2
    build_folio(paths.folio)
    edit_unit(paths, "voice:0004", "voice:0004x", "Good evening. The lamp is out.")
    res = run(toy, paths, endpoints_file, ep, stale=True, regenerate=True)
    assert [(o.subject, o.status) for o in res.report.outcomes] == [("Alice", WRITTEN)]
    assert len(ep.requests) == 3
    assert (paths.pack_dir / "persona" / "STALE.md").exists()
    # The new reply records the new sources, so it is current again.
    again = stale_run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    assert again.stale.stale == [] and again.stale.current == 1


def test_regenerate_needs_stale(toy, paths, endpoints_file):
    with pytest.raises(UsageError, match="--stale"):
        run(toy, paths, endpoints_file, FakeEndpoint(recorded("responses_ok.json")),
            regenerate=True)


def test_the_cli_lists_stale_personas_without_a_key(toy, paths, endpoints_file, monkeypatch):
    import typer
    from rich.console import Console
    from typer.testing import CliRunner

    from dramatis_forge.persona import cli
    from dramatis_forge.persona import generator as gen_mod

    monkeypatch.setattr(gen_mod, "load", lambda _pack: GEN)
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice"])
    build_folio(paths.folio)
    edit_unit(paths, "voice:0004", "voice:0004x", "Good evening. The lamp is out.")

    app = typer.Typer()
    cli.register(app, resolve=lambda _p, _h: (toy, paths),
                 die=lambda m, h: pytest.fail(f"{m} {h}"), console=Console(width=200))
    out = CliRunner().invoke(app, ["--stale", "--endpoints", str(endpoints_file())])
    assert out.exit_code == 0, out.output
    assert "stale 1" in out.output and "STALE.md" in out.output
    assert len(ep.requests) == 1


def test_a_budget_cut_keeps_voice_from_every_form(paths):
    db = open_db(paths)
    voices = [i for i in for_person(db, "Alice", GEN, Persona(material_tokens=10**6, story_tokens=0),
                                    estimate_tokens, ": ").items if i.kind == "voice"]
    room = sum(i.tokens for i in voices[:2]) + sum(
        i.tokens for i in for_person(db, "Alice", GEN, Persona(), estimate_tokens, ": ").items
        if i.kind == "card")
    mat = for_person(db, "Alice", GEN, Persona(material_tokens=room, story_tokens=0),
                     estimate_tokens, ": ")
    forms = {i.form_page for i in mat.items if i.kind == "voice"}
    assert forms == {"Alice", "Alice (Winter)"}


# --------------------------------------------------------------------------- #
# example lines: the person's own words, picked without the model
# --------------------------------------------------------------------------- #


def lines_of(forms: dict[str, tuple[int, int]], words: int = 12) -> tuple[list[Example], list[str]]:
    """Synthetic candidates: per form, (voice count, story count), each line its own scene."""
    cands, scenes = [], []
    for form, (voices, stories) in forms.items():
        for kind, n in (("voice", voices), ("story", stories)):
            for i in range(n):
                cands.append(Example(form, kind, f"{form} {kind} line {i} " + "word " * words,
                                     words + 6, f"{kind}:{form}:{i}"))
                scenes.append(f"{kind} {i}")
    return cands, scenes


def pick(forms, p=None, drops=()):
    p = p or Persona()
    cands, scenes = lines_of(forms)
    rank = {f: i for i, f in enumerate(forms)}
    return select_examples(cands, scenes, rank, p, [re.compile(d) for d in drops])


def test_examples_are_deterministic_and_spread_over_forms_and_kinds():
    forms = {"A": (30, 30), "B": (30, 30), "C": (3, 0)}
    got = pick(forms)
    assert got == pick(forms) and len(got) == Persona().example_lines
    assert {e.form_page for e in got} == {"A", "B", "C"}
    assert {e.kind for e in got if e.form_page == "A"} == {"voice", "story"}
    # Spread inside a group, not the first lines of it.
    a_voice = sorted(int(e.text.split()[3]) for e in got if e.form_page == "A" and e.kind == "voice")
    assert a_voice[-1] > 10


def test_examples_respect_count_and_token_budget():
    forms = {"A": (30, 30)}
    assert len(pick(forms, Persona(example_lines=3))) == 3
    one = pick(forms, Persona(example_lines=1))[0].tokens
    tight = Persona(example_tokens=one * 4 + 1)
    got = pick(forms, tight)
    assert sum(e.tokens for e in got) <= tight.example_tokens and len(got) == 4


def test_examples_skip_interjections_long_lines_prompts_and_repeated_scenes():
    short = Example("A", "voice", "Hmph.", 3, "v1")
    long_ = Example("A", "story", "word " * 300, 300, "s1")
    prompt = Example("A", "story", "[Tap to continue]", 12, "s2")
    ok = Example("A", "voice", "Stay by the stove tonight, it is cold.", 12, "v2")
    twin = Example("A", "voice", "Stay near the stove tonight, it is cold.", 12, "v3")
    got = select_examples([short, long_, prompt, ok, twin], ["a", "b", "c", "d", "d"],
                          {"A": 0}, Persona(), [re.compile(r"\[.*\]")])
    assert got == [ok]


def test_material_picks_examples_from_the_folio_and_counts_their_units_as_sent(paths):
    db = open_db(paths)
    mat = for_person(db, "Alice", GEN, Persona(example_min_tokens=1), estimate_tokens, ": ")
    texts = [e.text for e in mat.examples]
    assert "Good evening, <reader>. The lamp is lit." in texts
    assert f"Good morning, Dr.{PLACEHOLDER}." in texts  # placeholder kept verbatim
    assert {e.form_page for e in mat.examples} == {"Alice", "Alice (Winter)"}
    assert {e.unit for e in mat.examples} <= mat.sent
    lean = for_person(db, "Alice", GEN, Persona(material_tokens=1, story_tokens=0),
                      estimate_tokens, ": ")
    assert {e.unit for e in lean.examples} <= lean.sent  # even when the budget cut them


def test_lines_are_stored_with_the_rows_and_restored_from_the_cache(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("responses_ok.json"))
    run(toy, paths, endpoints_file, ep, only=["Alice"])
    r = rows(paths)
    body, version = r[("Alice", "lines")]
    assert "Cold tonight. Stay near the stove." in body.split("\n")
    assert version == r[("Alice", "system")][1]
    assert not any(slot == "fallback" for _s, slot in r)

    build_folio(paths.folio)
    assert ("Alice", "lines") not in rows(paths)
    restored = run(toy, paths, endpoints_file, ep, only=["Alice"], lookup=key_lookup(None))
    assert [o.status for o in restored.report.outcomes] == [RESTORED]
    assert rows(paths)[("Alice", "lines")][0] == body


def test_probe_report_shows_the_example_lines(toy, paths, endpoints_file):
    ep = FakeEndpoint(recorded("chat_ok.json"))
    run(toy, paths, endpoints_file, ep, wire="chat", probe=True, only=["Alice"])
    probe = paths.pack_dir / "persona" / "probe"
    md = next(probe.glob("Alice-*.md")).read_text()
    assert "## Example lines" in md and "Cold tonight. Stay near the stove." in md
    assert "Cold tonight. Stay near the stove." in next(probe.glob("Alice-*.json")).read_text()


def test_the_output_schema_has_no_fallback_line():
    assert "fallback_line" not in GEN.fields(True) + GEN.fields(False)
    assert "fallback_line" not in GEN.schema(True)["properties"]
    assert "fallback" not in GEN.system and "when unsure" not in GEN.system
