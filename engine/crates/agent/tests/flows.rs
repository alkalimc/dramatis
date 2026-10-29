//! The agent's acceptance tests, over a scripted endpoint: the prefix invariant on every
//! kind of log, meta-layer isolation, name substitution and rename rollover, metering,
//! and the persona gate.

mod common;

use api::endpoints::WireApi;
use common::*;
use serde_json::{Value, json};
use std::sync::Arc;
use world::fact::{self, Audience, NewFact};
use world::{Actor, ChannelId, FactKind, Mode, Origin, Shape, bond, channel, settings};

fn rows(agent: &agent::Agent, sql: &str) -> Vec<(String, i64, i64, i64)> {
    agent.with_world(|w| {
        let mut stmt = w.prepare(sql).unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

fn meter(agent: &agent::Agent) -> Vec<(String, i64, i64, i64)> {
    rows(
        agent,
        "SELECT shape, uncached, cached, output FROM meter ORDER BY id",
    )
}

/// The tool the request forces, if it forces one.
fn forced_tool(v: &Value) -> Option<&str> {
    let c = &v["tool_choice"];
    if c["type"] != "function" {
        return None;
    }
    c["function"]["name"].as_str().or(c["name"].as_str())
}

fn body_text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

#[tokio::test]
async fn direct_turns_extend_one_prefix_on_every_protocol() {
    for wire in WIRES {
        let e = env(wire);
        let ch = direct(&e, "ann");
        e.fake.push([
            call("search", json!({"query": "forge blade"})),
            say("(Ann looks up) The tea needs cooler water [#u2]."),
            say("Yes."),
            say("Good night."),
        ]);
        e.agent
            .send(ch, "how should I brew jasmine tea?", None)
            .await
            .unwrap();
        e.agent.send(ch, "and the water?", None).await.unwrap();
        e.agent.send(ch, "thanks", None).await.unwrap();
        let bodies = e.fake.bodies();
        assert_eq!(bodies.len(), 4, "{wire:?}");
        assert_prefix_chain(&bodies);
        let json = e.fake.json();
        // Constant shape: model, tools and reasoning identical in every body.
        for v in &json {
            assert_eq!(head(v), head(&json[0]), "{wire:?}");
            assert!(v.get("text").is_none() && v.get("response_format").is_none());
        }
        let seg = segments(&e.agent, ch);
        assert_eq!(seg.len(), 1);
        if wire == WireApi::Responses {
            assert_eq!(
                json[0]["prompt_cache_key"],
                format!("dramatis-{}", seg[0].id)
            );
        }
        // The reasoning the model produced is replayed with its tool call, in that turn.
        match wire {
            WireApi::Messages => {
                let turn = &list(&json[1])[1];
                assert_eq!(turn["role"], "assistant");
                assert_eq!(turn["content"][0]["type"], "thinking");
                assert_eq!(turn["content"][0]["signature"], "sig-0");
                assert_eq!(turn["content"][1]["type"], "tool_use");
                assert_eq!(list(&json[1])[2]["content"][0]["type"], "tool_result");
                // Blocks A and B end cached prefixes; C and the tail ride the moving one.
                let system = json[0]["system"].as_array().unwrap();
                assert_eq!(system.len(), 3);
                assert!(system[0].get("cache_control").is_some());
                assert!(system[1].get("cache_control").is_some());
                assert!(system[2].get("cache_control").is_none());
                assert!(json[0].get("cache_control").is_some());
            }
            WireApi::Responses => {
                let all = list(&json[1])
                    .iter()
                    .map(Value::to_string)
                    .collect::<String>();
                assert!(all.contains("enc-0"), "the reasoning item goes back");
            }
            WireApi::Chat => {}
        }
        // The shared tool list, not the host's.
        let tools = json[0]["tools"].to_string();
        assert!(tools.contains("request_join") && !tools.contains("find_people"));
        // Replies are stored verbatim, scene line included, with the citation resolved.
        let hist = e.agent.history(ch, None, 10).unwrap();
        let reply = hist.iter().find(|m| m.text.contains("cooler")).unwrap();
        assert_eq!(
            reply.text,
            "(Ann looks up) The tea needs cooler water [#u2]."
        );
        assert_eq!(reply.cites.len(), 1);
        assert_eq!(reply.cites[0].chunk_id, "u2");
        assert!(matches!(
            reply.actions[0],
            api::views::ToolAction::Search { .. }
        ));
        assert_eq!(e.fake.left(), 0);
    }
}

#[tokio::test]
async fn deltas_stream_under_one_message_and_the_final_bytes_are_verbatim() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([say("Steep it for three minutes, Dr.Doc.")]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    let events = e.sink.take();
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|ev| match ev {
            agent::Event::MessageDelta(d) => Some(d.clone()),
            _ => None,
        })
        .collect();
    assert!(deltas.len() >= 2);
    assert!(deltas.iter().all(|d| d.message == deltas[0].message));
    let streamed: String = deltas.iter().map(|d| d.delta.as_str()).collect();
    let added = events
        .iter()
        .rev()
        .find_map(|ev| match ev {
            agent::Event::MessageAdded(m) => Some(m.message.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(added.id, deltas[0].message);
    assert_eq!(added.text, streamed);
    assert!(log_bytes(&e.agent, ch).contains(&serde_json::to_string(&streamed).unwrap()));
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, agent::Event::QuotaChanged(_)))
    );
}

#[tokio::test]
async fn recorded_streams_replay_through_the_agent() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([Reply::Raw(
        include_str!("fixtures/responses_text.sse").into(),
    )]);
    e.agent.send(ch, "evening", None).await.unwrap();
    assert_eq!(meter(&e.agent), [("direct".into(), 200, 8960, 57)]);

    let e = env_with(endpoints(WireApi::Chat, true, None), |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([Reply::Raw(
        include_str!("fixtures/chat_hit_miss.sse").into(),
    )]);
    e.agent.send(ch, "hello", None).await.unwrap();
    assert_eq!(meter(&e.agent), [("direct".into(), 64, 1920, 40)]);
}

#[tokio::test]
async fn material_is_deduplicated_and_nothing_found_is_said_plainly() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([say("a"), say("b"), say("c")]);
    e.agent.send(ch, "jasmine tea", None).await.unwrap();
    e.agent.send(ch, "jasmine tea again", None).await.unwrap();
    let last = body_text(&e.fake.bodies()[1]);
    assert_eq!(last.matches("poured the jasmine tea").count(), 1, "{last}");
    assert!(last.matches("[#u1]").count() >= 2, "repeat shown by id");

    e.agent.send(ch, "zebra quantum", None).await.unwrap();
    let v = &e.fake.json()[2];
    let entry = list(v).last().unwrap().to_string();
    assert!(entry.contains("Nothing came back for this."));
    assert!(!entry.contains("request_join") && !entry.contains("[#"));
}

#[tokio::test]
async fn a_persona_carries_the_rules_and_its_own_lines_and_ignores_the_old_fallback() {
    let e = env(WireApi::Messages);
    let ch = direct(&e, "ann");
    let bo = direct(&e, "bo");
    e.fake.push([say("a"), say("b")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    e.agent.send(bo, "hello", None).await.unwrap();
    let json = e.fake.json();
    let system = |v: &Value| v["system"].to_string();
    let ann = system(&json[0]);
    assert!(ann.contains("Lines ann has said"), "{ann}");
    assert!(ann.contains("Mind the steam."));
    // The slot some folios still carry is never sent; no lines means no section.
    assert!(!ann.contains("Not sure"));
    assert!(!system(&json[1]).contains("Lines bo has said"));
    // Reply shape and how not knowing is played.
    assert!(ann.contains("two to five sentences") && ann.contains("every reply with a question"));
    assert!(ann.contains("never talk about whether they have evidence"));
}

#[tokio::test]
async fn low_confidence_sends_the_top_passages_marked_as_possibly_unrelated() {
    let mut weak = retrieve();
    weak.confidence.low_top1 = 1000.0;
    weak.confidence.high_top1 = 1000.0;
    let e = env_retrieving(endpoints(WireApi::Chat, true, None), weak, |c| {
        c.agent.retrieve.low_hits = 1
    });
    let ch = direct(&e, "ann");
    e.fake.push([say("a")]);
    e.agent.send(ch, "jasmine tea", None).await.unwrap();
    let entry = list(&e.fake.json()[0]).last().unwrap().to_string();
    assert!(entry.contains("may be unrelated"), "{entry}");
    assert!(entry.contains("poured the jasmine tea"), "{entry}");
    assert!(!entry.contains("nothing reliable") && !entry.contains("request_join"));
    // Only `low_hits` passages, without neighbours.
    assert_eq!(entry.matches("[#").count(), 1, "{entry}");
}

#[tokio::test]
async fn a_request_runs_on_the_direct_log_and_ends_with_its_report() {
    for wire in WIRES {
        a_request_runs(wire).await;
    }
}

async fn a_request_runs(wire: WireApi) {
    let e = env(wire);
    let ch = direct(&e, "ann");
    e.fake.push([say("Hello.")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    let asked = e
        .agent
        .ask(&p("ann"), "How is jasmine tea brewed?", Some(3), None)
        .unwrap();
    assert_eq!(asked.channel, ch);
    e.fake.push([
        call("search", json!({"query": "jasmine tea water"})),
        call(
            "report",
            json!({"text": "Cooler water [#u2].", "cites": ["u2"], "note": "# Tea\ncooler"}),
        ),
        call(
            "wrapup",
            json!({"facts": [{"kind": "fact", "text": "Likes jasmine tea"}]}),
        ),
    ]);
    e.agent.run_request(asked).await.unwrap();
    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 4);
    assert_prefix_chain(&bodies);
    let t = e
        .agent
        .with_world(|w| world::task::get(w, asked.task).unwrap());
    assert_eq!(t.status, world::TaskStatus::Done);
    assert_eq!(t.cites[0].chunk_id, "u2");
    let note = e.agent.config().office.join(t.note_path.unwrap());
    assert_eq!(std::fs::read_to_string(note).unwrap(), "# Tea\ncooler");
    let reply = e.agent.history(ch, None, 10).unwrap().pop().unwrap();
    assert_eq!(reply.text, "Cooler water [#u2].");
    assert_eq!(reply.task.as_ref().unwrap().0, asked.task.0.to_string());
    // The request's harness turn carries the remaining turns.
    let ask_entry = list(&e.fake.json()[1]).last().unwrap().to_string();
    assert!(ask_entry.contains("look into this") && ask_entry.contains("turns left"));
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["direct", "ask", "ask", "wrapup"]);
    // The wrap-up was forced, after every item, where the protocol allows it; on the
    // messages protocol the choice stays `auto` and the turn asks in words.
    let last = &e.fake.json()[3];
    if wire == WireApi::Messages {
        assert_eq!(forced_tool(last), None);
        assert_eq!(last["tool_choice"]["type"], "auto");
        assert!(
            list(last)
                .last()
                .unwrap()
                .to_string()
                .contains("wrapup call")
        );
    } else {
        assert_eq!(forced_tool(last), Some("wrapup"));
    }
    let facts = e.agent.with_world(|w| fact::all(w, false).unwrap());
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].audience, Audience::Participants(ch));
}

#[tokio::test]
async fn a_request_that_runs_out_of_turns_is_closed_with_what_was_said() {
    let e = env(WireApi::Chat);
    let asked = e
        .agent
        .ask(&p("ann"), "jasmine tea?", Some(1), None)
        .unwrap();
    e.fake.push([
        say("I only know it needs cooler water."),
        call("wrapup", json!({"facts": []})),
    ]);
    e.agent.run_request(asked).await.unwrap();
    let t = e
        .agent
        .with_world(|w| world::task::get(w, asked.task).unwrap());
    assert_eq!(t.status, world::TaskStatus::Done);
    let msgs = e.agent.history(asked.channel, None, 10).unwrap();
    assert_eq!(msgs.len(), 1, "one reply, no second message");
    assert_eq!(msgs[0].text, "I only know it needs cooler water.");
    // One turn: the harness said it must report now.
    let entry = list(&e.fake.json()[0]).last().unwrap().to_string();
    assert!(entry.contains("last turn on this request"));
}

#[tokio::test]
async fn wrapup_is_refused_in_an_ordinary_turn() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake
        .push([call("wrapup", json!({"facts": []})), say("ok")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    let log = log_bytes(&e.agent, ch);
    assert!(log.contains("wrapup is only for closing a conversation."));
    assert!(
        e.agent
            .with_world(|w| fact::all(w, true).unwrap())
            .is_empty()
    );
}

#[tokio::test]
async fn report_with_nothing_asked_is_the_reply_without_another_call() {
    // The tool list is part of the cached prefix and fixed per segment, so `report` stays
    // offered; a call with no request open ends the turn with its text instead of costing
    // the refusal round trip.
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([
        call("report", json!({"text": "Cooler water."})),
        say("never asked"),
    ]);
    e.agent.send(ch, "hi", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
    let msgs = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(msgs.last().unwrap().text, "Cooler water.");
}

#[tokio::test]
async fn wrapup_is_skipped_without_a_forced_tool() {
    let e = env_with(endpoints(WireApi::Responses, false, None), |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([say("hi")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1, "no wrap-up call");
}

#[tokio::test]
async fn on_the_messages_protocol_the_wrapup_is_asked_in_words() {
    // `tool_choice` stays `auto` whatever the model can do; a reply without the call is no
    // wrap-up, exactly as for a model that cannot be forced.
    let e = env_with(endpoints(WireApi::Messages, false, None), |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([say("hi"), say("Nothing to add.")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert_prefix_chain(&e.fake.bodies());
    let v = &e.fake.json()[1];
    assert_eq!(v["tool_choice"]["type"], "auto");
    assert!(list(v).last().unwrap().to_string().contains("wrapup call"));
    assert!(
        e.agent
            .with_world(|w| fact::all(w, false).unwrap())
            .is_empty()
    );
    e.fake.push([
        say("again"),
        call(
            "wrapup",
            json!({"facts": [{"kind": "fact", "text": "Likes tea"}]}),
        ),
    ]);
    e.agent.send(ch, "more", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert_eq!(
        e.agent.with_world(|w| fact::all(w, false).unwrap()).len(),
        1
    );
}

#[tokio::test]
async fn wrapup_closes_the_conversation_on_its_own_log() {
    for wire in WIRES {
        wrapup_closes(wire).await;
    }
}

async fn wrapup_closes(wire: WireApi) {
    let e = env(wire);
    let ch = direct(&e, "ann");
    e.fake.push([
        say("See you tomorrow at nine."),
        call(
            "wrapup",
            json!({"facts": [{"kind": "commitment", "text": "Tea tomorrow", "due": "2026-03-11T09:00"},
                             {"kind": "fact", "text": "Prefers green tea"}],
                   "hurt": {"quote": "you are useless"}}),
        ),
    ]);
    e.agent
        .send(ch, "tea tomorrow at nine? you are useless", None)
        .await
        .unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let bodies = e.fake.bodies();
    assert_prefix_chain(&bodies);
    let v = &e.fake.json()[1];
    assert_eq!(
        forced_tool(v),
        (wire != WireApi::Messages).then_some("wrapup")
    );
    assert_eq!(head(v), head(&e.fake.json()[0]));
    let facts = e.agent.with_world(|w| fact::all(w, false).unwrap());
    assert_eq!(facts.len(), 3);
    assert!(
        facts
            .iter()
            .any(|f| f.kind == FactKind::Commitment && f.due.is_some())
    );
    let trust = e
        .agent
        .with_world(|w| bond::user_bond(w, &p("ann")).unwrap().trust);
    assert!(trust < 100, "the hurt took trust");
}

#[tokio::test]
async fn material_past_the_threshold_rolls_over_after_a_wrapup() {
    for wire in WIRES {
        rolls_over(wire).await;
    }
}

#[tokio::test]
async fn a_per_request_model_does_not_roll_for_material_and_records_no_points() {
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].billing = Some(api::endpoints::Billing::Request);
    let e = env_with(ep, |c| c.agent.roll.k = 10.0);
    let ch = direct(&e, "ann");
    e.fake.push([say("Cooler water."), say("Again: cooler.")]);
    e.agent
        .send(ch, "how should I brew jasmine tea?", None)
        .await
        .unwrap();
    e.agent.send(ch, "and once more?", None).await.unwrap();
    assert_eq!(segments(&e.agent, ch).len(), 1);
    assert_eq!(e.fake.bodies().len(), 2, "no wrap-up call either");
    let points: f64 = e.agent.with_world(|w| {
        w.query_row("SELECT SUM(points) FROM meter", [], |r| r.get(0))
            .unwrap()
    });
    assert_eq!(points, 0.0);
    let calls: i64 = e.agent.with_world(|w| {
        w.query_row("SELECT COUNT(*) FROM meter", [], |r| r.get(0))
            .unwrap()
    });
    assert_eq!(calls, 2);
    let requests: i64 = e.agent.with_world(|w| {
        w.query_row("SELECT SUM(requests) FROM meter", [], |r| r.get(0))
            .unwrap()
    });
    assert_eq!(requests, 2, "each call is flagged for the hourly window");
}

#[tokio::test]
async fn emulated_tools_are_left_out_of_every_turn() {
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let e = env_with(ep, |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([
        say("Cooler water."),
        call(
            "wrapup",
            json!({"facts": [], "summary": "They talked about tea."}),
        ),
    ]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let json = e.fake.json();
    assert!(json[0].get("tools").is_none() && json[0].get("tool_choice").is_none());
    // The wrap-up asks for structured output instead.
    assert!(json[1].get("tools").is_none() && json[1].get("tool_choice").is_none());
    assert_eq!(json[1]["response_format"]["type"], "json_schema");
}

#[tokio::test]
async fn an_emulated_wrapup_written_as_a_text_block_still_counts() {
    // The fence opening may be missing, and the block may sit after some words.
    for block in [
        "```tool_call\n{BODY}\n```",
        "tool_call\n{BODY}\n```",
        "Noted.\n```tool_call\n{BODY}\n```",
    ] {
        let mut ep = endpoints(WireApi::Chat, true, None);
        ep.profiles[0].models[0].native_tools = Some(false);
        let e = env_with(ep, |_| {});
        let ch = direct(&e, "ann");
        let body = json!({"name": "wrapup", "arguments": {"facts": [
            {"kind": "fact", "text": "The user likes jasmine tea.", "due": null}
        ], "hurt": null, "summary": null}});
        e.fake.push([
            say("Cooler water."),
            say(&block.replace("{BODY}", &body.to_string())),
        ]);
        e.agent
            .send(ch, "how should I brew tea?", None)
            .await
            .unwrap();
        e.agent.wrapup(ch).await.unwrap();
        let facts = e.agent.with_world(|w| fact::all(w, false).unwrap());
        assert_eq!(facts.len(), 1, "{block}");
    }
}

#[tokio::test]
async fn an_emulated_wrapup_asks_for_json_and_applies_it() {
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let e = env_with(ep, |_| {});
    let ch = direct(&e, "ann");
    let doc = json!({"facts": [
        {"kind": "fact", "text": "The user likes jasmine tea.", "due": null}
    ], "hurt": null, "summary": null});
    e.fake.push([say("Cooler water."), say(&doc.to_string())]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let mut sent = e.fake.json();
    let last = sent.pop().unwrap();
    // A structured-output request is one whole reply; the turn before it streamed.
    assert_eq!(sent[0]["stream"], true);
    assert!(last.get("stream").is_none());
    assert!(last.get("tools").is_none() && last.get("tool_choice").is_none());
    assert_eq!(last["response_format"]["type"], "json_schema");
    assert!(last["response_format"]["json_schema"]["schema"]["properties"]["facts"].is_object());
    // The closing line asks out of character and names no note-taking.
    let ask = list(&last).last().unwrap().to_string();
    assert!(ask.contains("JSON object") && !ask.contains("note") && !ask.contains("save"));
    assert_eq!(
        e.agent.with_world(|w| fact::all(w, false).unwrap()).len(),
        1
    );
}

#[tokio::test]
async fn a_structured_wrapup_is_not_streamed_on_any_wire() {
    for wire in WIRES {
        let mut ep = endpoints(wire, true, None);
        ep.profiles[0].models[0].native_tools = Some(false);
        let e = env_with(ep, |_| {});
        let ch = direct(&e, "ann");
        let doc = json!({"facts": [
            {"kind": "fact", "text": "The user likes jasmine tea.", "due": null}
        ], "hurt": null, "summary": null});
        e.fake.push([say("Cooler water."), say(&doc.to_string())]);
        e.agent.send(ch, "hi", None).await.unwrap();
        e.agent.wrapup(ch).await.unwrap();
        let sent = e.fake.json();
        assert_eq!(sent[0].get("stream"), Some(&json!(true)), "{wire:?}");
        assert!(sent[1].get("stream").is_none(), "{wire:?}");
        assert_eq!(
            e.agent.with_world(|w| fact::all(w, false).unwrap()).len(),
            1,
            "{wire:?}"
        );
    }
}

#[tokio::test]
async fn an_emulated_wrapup_in_prose_records_nothing() {
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let e = env_with(ep, |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([
        say("Cooler water."),
        say("* **Event**: tea.\n* **State**: calm."),
    ]);
    e.agent.send(ch, "hi", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert!(
        e.agent
            .with_world(|w| fact::all(w, false).unwrap())
            .is_empty()
    );
}

/// The reply a proxy that ignores the schema gave: fenced, with the right keys.
const FENCED: &str = "```json\n{\n  \"facts\": [\n    {\"kind\": \"commitment\", \"text\": \"The host reviews the supply requests.\", \"due\": \"2031-04-02T15:00\"},\n    {\"kind\": \"commitment\", \"text\": \"The host walks the deck with the user.\", \"due\": \"tomorrow afternoon\"}\n  ],\n  \"summary\": \"Paperwork done.\"\n}\n```";

/// The reply that invented its own field names.
const INVENTED: &str = "```json\n{\n  \"summary\": \"Paperwork done.\",\n  \"appointments\": [\n    {\"date\": \"tomorrow\", \"time\": \"15:00\", \"content\": \"Review the requests\"}\n  ]\n}\n```";

fn emulated_env() -> (Env, ChannelId) {
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let e = env_with(ep, |_| {});
    let ch = direct(&e, "ann");
    (e, ch)
}

#[tokio::test]
async fn a_fenced_wrapup_document_writes_its_facts_and_keeps_an_unparsable_due() {
    let (e, ch) = emulated_env();
    e.fake.push([say("Cooler water."), say(FENCED)]);
    e.agent.send(ch, "hi", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let facts = e.agent.with_world(|w| fact::all(w, false).unwrap());
    assert_eq!(facts.len(), 2);
    let texts: Vec<_> = facts.iter().map(|f| f.text.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("deck") && t.contains("tomorrow afternoon"))
    );
}

#[tokio::test]
async fn a_wrapup_with_invented_field_names_writes_nothing() {
    let (e, ch) = emulated_env();
    e.fake.push([say("Cooler water."), say(INVENTED)]);
    e.agent.send(ch, "hi", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert!(
        e.agent
            .with_world(|w| fact::all(w, false).unwrap())
            .is_empty()
    );
}

#[tokio::test]
async fn the_emulated_wrapup_line_shows_the_schema_keys() {
    let (e, ch) = emulated_env();
    e.fake.push([say("Cooler water."), say("{}")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let last = e.fake.json().pop().unwrap();
    let ask = list(&last).last().unwrap().to_string();
    for key in ["facts", "kind", "text", "due", "hurt", "quote", "summary"] {
        assert!(ask.contains(&format!("\\\"{key}\\\"")), "{key}: {ask}");
    }
    assert!(ask.contains("fact|commitment"));
}

#[tokio::test]
async fn the_native_wrapup_line_carries_no_example() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("Cooler water."), say("ok")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let last = e.fake.json().pop().unwrap();
    let ask = list(&last).last().unwrap().to_string();
    assert!(ask.contains("wrapup call") && !ask.contains("fact|commitment"));
}

#[tokio::test]
async fn a_native_endpoint_still_forces_the_wrapup_tool() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([
        say("Cooler water."),
        call(
            "wrapup",
            json!({"facts": [{"kind": "fact", "text": "Likes tea"}]}),
        ),
    ]);
    e.agent.send(ch, "hi", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let last = e.fake.json().pop().unwrap();
    assert_eq!(forced_tool(&last), Some("wrapup"));
    assert!(last.get("response_format").is_none());
    assert_eq!(
        e.agent.with_world(|w| fact::all(w, false).unwrap()).len(),
        1
    );
}

#[tokio::test]
async fn a_text_block_is_not_a_call_on_a_native_endpoint() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    let body = json!({"name": "wrapup", "arguments": {"facts": [
        {"kind": "fact", "text": "The user likes jasmine tea.", "due": null}
    ], "hurt": null, "summary": null}});
    e.fake.push([
        say("Cooler water."),
        say(&format!("```tool_call\n{body}\n```")),
    ]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert!(
        e.agent
            .with_world(|w| fact::all(w, false).unwrap())
            .is_empty()
    );
}

/// An endpoint's canned error, as one of many wordings.
const REFUSAL: &str =
    "I'm having a hard time fulfilling your request. Can I help you with something else instead?";

fn shown_text(e: &Env) -> String {
    format!("{:?}", e.sink.take())
}

#[tokio::test]
async fn a_canned_refusal_is_retried_once_and_never_shown_or_kept() {
    let e = env_cjk(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say(REFUSAL), say("Cooler water.")]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    let hist = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(hist.last().unwrap().text, "Cooler water.");
    assert!(!shown_text(&e).contains("hard time"));
    assert!(!log_bytes(&e.agent, ch).contains("hard time"));
    // Both calls were paid.
    assert_eq!(meter(&e.agent).len(), 2);
}

#[tokio::test]
async fn a_repeated_refusal_leaves_the_turn_without_a_reply() {
    let e = env_cjk(WireApi::Chat);
    let ch = direct(&e, "ann");
    // Worded differently each time.
    e.fake.push([
        say(REFUSAL),
        say("I encountered an error doing what you asked. Could you try again?"),
    ]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    let hist = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(hist.len(), 1);
    assert!(!shown_text(&e).contains("hard time"));
    assert!(!log_bytes(&e.agent, ch).contains("hard time"));
    assert!(!log_bytes(&e.agent, ch).contains("encountered"));
    assert_eq!(meter(&e.agent).len(), 2);
}

#[tokio::test]
async fn short_replies_and_replies_in_cjk_text_are_not_boilerplate() {
    let e = env_cjk(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake
        .push([say("OK."), say("Fine, \u{597d}\u{7684}. I will see to it.")]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    e.agent.send(ch, "and then?", None).await.unwrap();
    let hist = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(hist[1].text, "OK.");
    assert_eq!(
        hist.last().unwrap().text,
        "Fine, \u{597d}\u{7684}. I will see to it."
    );
    assert_eq!(meter(&e.agent).len(), 2);
}

#[tokio::test]
async fn a_name_alone_and_a_long_english_answer_are_not_held_as_boilerplate() {
    let e = env_cjk(WireApi::Chat);
    let ch = direct(&e, "ann");
    let long = "The steel comes from the northern hold, where it is smelted twice and \
                then folded by hand before it is ever sent south to the forge, and the \
                forge keeps the finished blades in a locked room until the buyer comes.";
    assert!(long.chars().count() > 200);
    e.fake.push([say("Ann Lee."), say(long)]);
    e.agent.send(ch, "who is it?", None).await.unwrap();
    e.agent.send(ch, "and the steel?", None).await.unwrap();
    let hist = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(hist[1].text, "Ann Lee.");
    assert_eq!(hist.last().unwrap().text, long);
    assert_eq!(meter(&e.agent).len(), 2);
}

#[tokio::test]
async fn an_english_corpus_keeps_english_replies() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say(REFUSAL)]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    assert_eq!(
        e.agent.history(ch, None, 10).unwrap().last().unwrap().text,
        REFUSAL
    );
}

#[tokio::test]
async fn a_rate_limit_keeps_the_message_and_the_host_says_so() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake
        .push([Reply::Limited(Some(3600)), say("Cooler water.")]);
    let posted = e.agent.post(ch, "how should I brew tea?", None).unwrap();
    let err = e.agent.respond(posted.clone()).await.unwrap_err();
    assert!(matches!(
        err,
        agent::Error::RateLimited {
            retry_after: Some(3600)
        }
    ));
    assert!(err.to_string().contains("rate limited"));
    // The message stays, and the host says why nothing came back.
    let hist = e.agent.history(ch, None, 10).unwrap();
    assert_eq!(hist.len(), 1);
    assert!(
        e.sink
            .take()
            .iter()
            .any(|ev| format!("{ev:?}").contains("rate limited"))
    );
    // Asking again goes through.
    e.agent.respond(posted).await.unwrap();
    assert_eq!(e.agent.history(ch, None, 10).unwrap().len(), 2);
}

async fn rolls_over(wire: WireApi) {
    let e = env_with(endpoints(wire, true, None), |c| c.agent.roll.k = 10.0);
    let ch = direct(&e, "ann");
    e.fake.push([
        say("Cooler water."),
        call(
            "wrapup",
            json!({"facts": [], "summary": "They talked about brewing tea."}),
        ),
        say("Again: cooler."),
    ]);
    e.agent
        .send(ch, "how should I brew jasmine tea?", None)
        .await
        .unwrap();
    e.agent.send(ch, "and once more?", None).await.unwrap();
    let segs = segments(&e.agent, ch);
    assert_eq!(segs.len(), 2);
    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 3);
    // The wrap-up extends the old segment; the new segment starts again with A and B.
    assert_prefix_chain(&bodies[..2]);
    assert_eq!(segs[0].prefix_a, segs[1].prefix_a);
    assert_eq!(segs[0].prefix_b, segs[1].prefix_b);
    let c = body_text(&segs[1].prefix_c);
    assert!(c.contains("They talked about brewing tea."));
    assert!(
        c.contains("how should I brew jasmine tea?"),
        "the tail carries over"
    );
    assert!(
        !c.contains("once more"),
        "the new turn is appended, not carried"
    );
    let json = e.fake.json();
    if wire == WireApi::Responses {
        assert_eq!(
            json[1]["prompt_cache_key"],
            format!("dramatis-{}", segs[0].id)
        );
        assert_eq!(
            json[2]["prompt_cache_key"],
            format!("dramatis-{}", segs[1].id)
        );
    }
    // A, B, C, the new turn; the messages protocol keeps the blocks apart.
    let blocks = if wire == WireApi::Messages { 0 } else { 3 };
    assert_eq!(list(&json[2]).len(), blocks + 1);
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["direct", "wrapup", "direct"]);
}

#[tokio::test]
async fn a_context_near_the_window_rolls_over() {
    let e = env_with(endpoints(WireApi::Responses, true, Some(200)), |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([
        say("one"),
        call("wrapup", json!({"facts": [], "summary": "s"})),
        say("two"),
    ]);
    e.agent.send(ch, "hello", None).await.unwrap();
    e.agent.send(ch, "hello again", None).await.unwrap();
    assert_eq!(segments(&e.agent, ch).len(), 2);
}

#[tokio::test]
async fn groups_share_one_log_address_one_person_and_interject_at_most_once() {
    for wire in WIRES {
        group_log(wire).await;
    }
}

async fn group_log(wire: WireApi) {
    let e = env(wire);
    let g = e
        .agent
        .create_group(&[p("ann"), p("bo")], Some("smithing"))
        .unwrap();
    e.fake
        .push([say("Bo: steel from the north."), say("Ann: and tea after.")]);
    // Frozen group: only the addressed person answers.
    e.agent
        .send(g, "where does the forge blade steel come from?", None)
        .await
        .unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
    let first = list(&e.fake.json()[0]).last().unwrap().to_string();
    assert!(first.contains("bo answers."), "{first}");

    // Enabled group with enabled members: one interjection, never two.
    e.agent.with_world(|w| {
        channel::set_mode(w, g, Mode::Enabled).unwrap();
        bond::set_mode(w, &p("ann"), Mode::Enabled).unwrap();
        bond::set_mode(w, &p("bo"), Mode::Enabled).unwrap();
    });
    e.fake.push([say("Ann: by name."), say("Bo: a line.")]);
    e.agent
        .send(g, "ann, jasmine tea at the forge blade?", None)
        .await
        .unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 3);
    let named = list(&json[1]).last().unwrap().to_string();
    assert!(
        named.contains("ann answers."),
        "named person first: {named}"
    );
    let inter = list(&json[2]).last().unwrap().to_string();
    assert!(inter.contains("bo may add one line."), "{inter}");
    assert_prefix_chain(&e.fake.bodies());
    // Both personas in one prefix.
    let segs = segments(&e.agent, g);
    let b = body_text(&segs[0].prefix_b);
    assert!(b.contains("You are ann.") && b.contains("You are bo."));
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["group", "group", "interject"]);
}

#[tokio::test]
async fn a_message_to_everyone_gets_a_round_of_replies_in_order() {
    let e = env(WireApi::Chat);
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    // The group and both members are frozen: speaking to everyone still reaches both.
    e.fake
        .push([say("Ann: good morning."), say("Bo: morning.")]);
    e.agent
        .send(g, "good morning everyone", None)
        .await
        .unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 2);
    let second = list(&json[1]).last().unwrap().to_string();
    assert!(
        second.contains("answers too, after the others."),
        "{second}"
    );
    assert_prefix_chain(&e.fake.bodies());
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["group", "interject"]);

    // Someone named in the same message still answers first.
    e.fake.push([say("Bo: first."), say("Ann: then me.")]);
    e.agent
        .send(g, "bo, and everyone, how are you?", None)
        .await
        .unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 4);
    assert!(
        list(&json[2])
            .last()
            .unwrap()
            .to_string()
            .contains("bo answers.")
    );
}

#[tokio::test]
async fn the_cap_a_named_question_and_disabled_members_bound_the_round() {
    // Cap of one: the addressed member alone.
    let e = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.agent.group.collective_max = 1;
    });
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.fake.push([say("Ann: hello.")]);
    e.agent.send(g, "hello everyone", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);

    // A named question in a frozen group keeps today's single reply.
    let e = env(WireApi::Chat);
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.fake.push([say("Bo: steel.")]);
    e.agent
        .send(g, "bo, where is the steel from?", None)
        .await
        .unwrap();
    assert_eq!(e.fake.bodies().len(), 1);

    // A disabled member never speaks, even when everyone is addressed.
    let e = env(WireApi::Chat);
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.agent
        .with_world(|w| bond::set_mode(w, &p("bo"), Mode::Disabled).unwrap());
    e.fake.push([say("Ann: hello.")]);
    e.agent.send(g, "hello everyone", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
}

#[tokio::test]
async fn a_round_stops_when_the_quota_band_closes_unprompted_speech() {
    let e = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.world.quota.quiet_at = 0.0;
    });
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.fake.push([say("Ann: hello."), say("Bo: hi.")]);
    e.agent.send(g, "hello everyone", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1, "the extra turn is unprompted");
}

fn classifying(max: u32) -> Env {
    env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.agent.group.classify = true;
        c.agent.group.collective_max = max;
    })
}

fn three(e: &Env) -> world::ChannelId {
    e.agent
        .create_group(&[p("ann"), p("bo"), p("dee")], None)
        .unwrap()
}

#[tokio::test]
async fn the_model_names_the_audience_and_the_cap_bounds_the_round() {
    // "all": everyone speaks, after one small request outside the group's log.
    let e = classifying(5);
    let g = three(&e);
    e.fake.push([
        say(r#"{"addressed": ["all"]}"#),
        say("one"),
        say("two"),
        say("three"),
    ]);
    e.agent.send(g, "what a fine morning", None).await.unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 4);
    let asked = json[0].to_string();
    assert!(
        asked.contains("Decide who") && asked.contains("json_schema"),
        "{asked}"
    );
    assert!(!asked.contains("You are ann."), "outside the group log");
    assert_prefix_chain(&e.fake.bodies()[1..]);

    // The cap is a hard bound on speakers.
    let e = classifying(2);
    let g = three(&e);
    e.fake
        .push([say(r#"{"addressed": ["all"]}"#), say("one"), say("two")]);
    e.agent.send(g, "what a fine morning", None).await.unwrap();
    assert_eq!(e.fake.json().len(), 3);

    // One member named by the model: one reply, no round.
    let e = classifying(5);
    let g = three(&e);
    e.fake
        .push([say(r#"{"addressed": ["bo"]}"#), say("Bo here.")]);
    e.agent.send(g, "what a fine morning", None).await.unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 2);
    assert!(
        list(&json[1])
            .last()
            .unwrap()
            .to_string()
            .contains("bo answers.")
    );

    // Two of three, the disabled one never among them.
    let e = classifying(5);
    let g = three(&e);
    e.agent
        .with_world(|w| bond::set_mode(w, &p("dee"), Mode::Disabled).unwrap());
    e.fake
        .push([say(r#"{"addressed": ["bo", "dee"]}"#), say("Bo here.")]);
    e.agent.send(g, "what a fine morning", None).await.unwrap();
    assert_eq!(e.fake.json().len(), 2);
}

#[tokio::test]
async fn an_unusable_audience_answer_falls_back_to_the_plain_rule() {
    let e = classifying(5);
    let g = three(&e);
    e.fake.push([say("no idea"), say("Ann: cooler water.")]);
    e.agent
        .send(g, "how should the jasmine tea be brewed?", None)
        .await
        .unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 2);
    assert!(
        list(&json[1])
            .last()
            .unwrap()
            .to_string()
            .contains("ann answers.")
    );

    // A failed request does the same, and the user's turn still gets its reply.
    let e = classifying(5);
    let g = three(&e);
    e.fake
        .push([Reply::Fail("boom".into()), say("Ann: cooler water.")]);
    e.agent
        .send(g, "how should the jasmine tea be brewed?", None)
        .await
        .unwrap();
    assert!(
        e.fake
            .json()
            .last()
            .unwrap()
            .to_string()
            .contains("ann answers.")
    );

    // One member named, no collective word: no extra request at all.
    let e = classifying(5);
    let g = three(&e);
    e.fake.push([say("Bo here.")]);
    e.agent.send(g, "bo, how are you?", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
}

#[tokio::test]
async fn a_colleague_pulled_in_joins_the_log_as_an_entry() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([
        call("request_join", json!({"topic": "forge blade steel"})),
        say("Bo here: northern steel."),
    ]);
    e.agent
        .send(ch, "who knows about blades?", None)
        .await
        .unwrap();
    assert_prefix_chain(&e.fake.bodies());
    let json = e.fake.json();
    let joined = list(&json[1]).last().unwrap().to_string();
    assert!(joined.contains("You are bo.") && joined.contains("bo joins the conversation."));
    let segs = segments(&e.agent, ch);
    assert!(
        !body_text(&segs[0].prefix_b).contains("You are bo."),
        "prefix untouched"
    );
    let ch_now = e.agent.with_world(|w| channel::get(w, ch).unwrap());
    assert!(ch_now.has(&Actor::Person(p("bo"))));
}

#[tokio::test]
async fn the_owner_is_addressed_again_after_a_colleague_answered() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    // The owner says nothing before handing over.
    e.fake.push([
        call("request_join", json!({"topic": "forge blade steel"})),
        say("Bo here: northern steel."),
    ]);
    e.agent
        .send(ch, "who knows about blades?", None)
        .await
        .unwrap();
    let texts: Vec<String> = e
        .agent
        .history(ch, None, 10)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(
        texts,
        ["who knows about blades?", "Bo here: northern steel."],
        "no empty bubble from the owner"
    );
    e.fake.push([say("Ann again.")]);
    e.agent.send(ch, "and you, ann?", None).await.unwrap();
    let entry = list(&e.fake.json()[2]).last().unwrap().to_string();
    assert!(entry.contains("ann answers."), "{entry}");
    // Once the owner has spoken, the next turn needs no reminder.
    e.fake.push([say("Still ann.")]);
    e.agent.send(ch, "thanks", None).await.unwrap();
    let entry = list(&e.fake.json()[3]).last().unwrap().to_string();
    assert!(!entry.contains("ann answers."), "{entry}");
}

#[tokio::test]
async fn a_weak_match_does_not_pull_anyone_in() {
    // Every score in this small corpus is below the bar for a clear answer.
    let mut weak = retrieve();
    weak.confidence.low_top1 = 1000.0;
    weak.confidence.high_top1 = 1000.0;
    let e = env_retrieving(endpoints(WireApi::Chat, true, None), weak, |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([
        call("request_join", json!({"topic": "forge blade steel"})),
        say("Nobody came."),
    ]);
    e.agent.send(ch, "someone?", None).await.unwrap();
    assert!(log_bytes(&e.agent, ch).contains("Nobody suitable is available."));
    let ch_now = e.agent.with_world(|w| channel::get(w, ch).unwrap());
    assert!(!ch_now.has(&Actor::Person(p("bo"))));
}

#[tokio::test]
async fn the_host_has_its_own_log_tools_and_authorised_actions() {
    for wire in WIRES {
        host_log(wire).await;
    }
}

async fn host_log(wire: WireApi) {
    let e = env(wire);
    // First open: the host's welcome is pre-written, so no model call is made.
    let out = e.agent.presence(true).await.unwrap();
    assert!(out.host_spoke && e.fake.bodies().is_empty());
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    assert!(
        e.agent.history(host, None, 5).unwrap()[0]
            .text
            .contains("Welcome")
    );

    // A harness turn of the host's: a tool call it attempts is refused.
    e.clock.advance(31 * 60_000);
    e.fake.push([call(
        "set_mode",
        json!({"target_kind": "person", "target": "ann", "mode": "enabled"}),
    )]);
    let out = e.agent.presence(false).await.unwrap();
    assert!(!out.host_spoke);
    assert_eq!(
        e.agent.with_world(|w| bond::mode(w, &p("ann")).unwrap()),
        Mode::Frozen
    );
    assert!(log_bytes(&e.agent, host).contains("Only when Doc asks for it."));
    // The last step offers no tool where the protocol lets the choice vary.
    match wire {
        WireApi::Messages => assert_eq!(e.fake.json()[0]["tool_choice"]["type"], "auto"),
        _ => assert_eq!(e.fake.json()[0]["tool_choice"], "none"),
    }

    // The user's own turn may.
    e.fake.push([
        call(
            "set_mode",
            json!({"target_kind": "person", "target": "ann", "mode": "enabled"}),
        ),
        say("Ann is enabled."),
    ]);
    e.agent.send(host, "please enable ann", None).await.unwrap();
    assert_eq!(
        e.agent.with_world(|w| bond::mode(w, &p("ann")).unwrap()),
        Mode::Enabled
    );

    // Back after idle: one line about the digest (ann's birthday), on the same log.
    e.clock.advance(31 * 60_000);
    e.fake.push([say("It is ann's birthday today.")]);
    let out = e.agent.presence(false).await.unwrap();
    assert!(out.host_spoke);
    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 4);
    assert_prefix_chain(&bodies);
    let json = e.fake.json();
    let tools = json[0]["tools"].to_string();
    assert!(tools.contains("find_people") && tools.contains("set_mode"));
    for v in &json {
        assert_eq!(head(v), head(&json[0]));
    }
    let digest = list(&json[3]).last().unwrap().to_string();
    assert!(digest.contains("Today is ann's birthday."), "{digest}");
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["host", "host", "host", "host"]);
}

#[tokio::test]
async fn the_meta_layer_never_leaves_the_hosts_own_log() {
    let e = env(WireApi::Responses);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.agent
        .with_world(|w| channel::add_participant(w, g, &Actor::Host).unwrap());
    let ch = direct(&e, "ann");
    e.fake
        .push([say("host hi"), say("group hi"), say("ann hi")]);
    e.agent.send(host, "hello", None).await.unwrap();
    e.agent.send(g, "jasmine tea", None).await.unwrap();
    e.agent.send(ch, "jasmine tea", None).await.unwrap();
    let bodies: Vec<String> = e.fake.bodies().iter().map(|b| body_text(b)).collect();
    assert!(bodies[0].contains(META) && bodies[0].contains(IN_WORLD));
    assert!(!bodies[1].contains(META), "group log carries no meta layer");
    assert!(
        bodies[1].contains(IN_WORLD),
        "the host appears in-world in a group"
    );
    assert!(!bodies[2].contains(META) && !bodies[2].contains(IN_WORLD));
    for c in [g, ch] {
        assert!(!log_bytes(&e.agent, c).contains(META));
    }
    // The host's tools stay in its own log too.
    assert!(!bodies[1].contains("find_people"));
}

#[tokio::test]
async fn a_shared_log_carries_only_what_everyone_there_may_recall() {
    let e = env(WireApi::Responses);
    let direct_ann = direct(&e, "ann");
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.agent.with_world(|w| {
        let write = |aud: Audience, text: &str| {
            fact::write(w, &NewFact::new(Actor::User, aud, FactKind::Fact, text), T0).unwrap()
        };
        write(Audience::Own(p("ann")), "ann secret jasmine recipe");
        write(
            Audience::Participants(direct_ann),
            "private jasmine promise",
        );
        write(Audience::Participants(g), "group jasmine plan");
        write(Audience::World, "world jasmine conclusion");
    });
    e.fake.push([say("g"), say("d")]);
    e.agent.send(g, "jasmine tea", None).await.unwrap();
    e.agent.send(direct_ann, "jasmine tea", None).await.unwrap();
    let group_log = log_bytes(&e.agent, g);
    assert!(group_log.contains("group jasmine plan") && group_log.contains("world jasmine"));
    assert!(!group_log.contains("secret") && !group_log.contains("private jasmine"));
    let direct_log = log_bytes(&e.agent, direct_ann);
    assert!(direct_log.contains("secret") && direct_log.contains("private jasmine"));
}

#[tokio::test]
async fn the_placeholder_becomes_the_users_name_and_a_rename_rolls_every_log() {
    let e = env(WireApi::Responses);
    let ann = direct(&e, "ann");
    let bo = direct(&e, "bo");
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([say("a"), say("b"), say("h")]);
    e.agent.send(ann, "jasmine tea", None).await.unwrap();
    e.agent.send(bo, "forge", None).await.unwrap();
    e.agent.send(host, "hi", None).await.unwrap();
    for b in e.fake.bodies() {
        let t = body_text(&b);
        assert!(!t.contains("<U>"), "a placeholder reached the model: {t}");
    }
    let first = body_text(&e.fake.bodies()[0]);
    assert!(
        first.contains("Dr.Doc"),
        "no name yet: the corpus's word for you"
    );
    assert!(
        first.contains("Dr.Doc poured the jasmine tea"),
        "retrieved text too"
    );
    let before = [ann, bo, host].map(|c| segments(&e.agent, c));

    e.agent
        .with_world(|w| assert!(settings::set_user_name(w, Some("Rhea")).unwrap()));
    e.fake.push([
        call("wrapup", json!({"facts": [], "summary": "tea"})),
        call("wrapup", json!({"facts": [], "summary": "forge"})),
    ]);
    e.agent.on_rename().await.unwrap();
    for (c, old) in [ann, bo, host].iter().zip(&before) {
        let segs = segments(&e.agent, *c);
        assert_eq!(segs.len(), 2, "channel {c} rolled");
        // Sent bytes are never rewritten.
        assert_eq!(segs[0], old[0]);
        let shape = agent::request::Shape::parse(&segs[1].shape).unwrap();
        assert_eq!(shape.user, "Rhea");
        assert!(!body_text(&segs[1].prefix_b).contains("Dr.Doc"));
    }
    assert!(body_text(&segments(&e.agent, ann)[1].prefix_b).contains("Dr.Rhea"));
    e.fake.push([say("Hello, Dr.Rhea.")]);
    e.agent.send(ann, "jasmine tea", None).await.unwrap();
    let last = body_text(e.fake.bodies().last().unwrap());
    assert!(last.contains("Dr.Rhea poured the jasmine tea") && !last.contains("Dr.Doc"));
}

#[tokio::test]
async fn a_rename_without_the_hook_still_rolls_on_the_next_turn() {
    let e = env(WireApi::Chat);
    let ann = direct(&e, "ann");
    e.fake.push([say("a")]);
    e.agent.send(ann, "hi", None).await.unwrap();
    e.agent
        .with_world(|w| settings::set_user_name(w, Some("Rhea")).unwrap());
    e.fake
        .push([call("wrapup", json!({"facts": []})), say("b")]);
    e.agent.send(ann, "hi again", None).await.unwrap();
    assert_eq!(segments(&e.agent, ann).len(), 2);
}

#[tokio::test]
async fn the_bio_reaches_every_log_when_set_and_nothing_is_said_when_empty() {
    let e = env(WireApi::Chat);
    let ann = direct(&e, "ann");
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([say("a"), say("h")]);
    e.agent.send(ann, "hi", None).await.unwrap();
    e.agent.send(host, "hi", None).await.unwrap();
    for b in e.fake.bodies() {
        assert!(
            !body_text(&b).contains("about themselves"),
            "no bio, no line"
        );
    }
    assert_eq!(
        segments(&e.agent, ann)[0]
            .shape
            .matches("\"about\":\"\"")
            .count(),
        1
    );

    e.agent.with_world(|w| {
        assert!(settings::set_user_bio(w, Some("Keeps bees on the roof.")).unwrap())
    });
    e.fake.push([
        call("wrapup", json!({"facts": [], "summary": "hi"})),
        call("wrapup", json!({"facts": [], "summary": "hi"})),
    ]);
    e.agent.on_rename().await.unwrap();
    for c in [ann, host] {
        let segs = segments(&e.agent, c);
        assert_eq!(segs.len(), 2, "channel {c} rolled for the new bio");
        // The segment before it keeps its bytes; the new one carries the user's words as typed.
        assert!(!body_text(&segs[0].prefix_a).contains("bees"));
        let a = body_text(&segs[1].prefix_a);
        assert!(
            a.contains("says this about themselves: Keeps bees on the roof."),
            "{a}"
        );
    }
    // Setting the same bio again rolls nothing.
    e.agent.with_world(|w| {
        assert!(!settings::set_user_bio(w, Some("Keeps bees on the roof.")).unwrap())
    });
    e.agent.on_rename().await.unwrap();
    assert_eq!(segments(&e.agent, ann).len(), 2);

    // Clearing it rolls again and the line is gone.
    e.agent
        .with_world(|w| assert!(settings::set_user_bio(w, None).unwrap()));
    e.fake.push([
        call("wrapup", json!({"facts": [], "summary": "hi"})),
        call("wrapup", json!({"facts": [], "summary": "hi"})),
    ]);
    e.agent.on_rename().await.unwrap();
    let segs = segments(&e.agent, ann);
    assert_eq!(segs.len(), 3);
    assert!(!body_text(&segs[2].prefix_a).contains("about themselves"));
}

#[tokio::test]
async fn usage_is_metered_per_shape_with_cache_hits_and_quota() {
    let e = env_with(endpoints(WireApi::Responses, true, None), |c| {
        c.world.quota.window_1h.middle = 1_000_000.0;
    });
    let ch = direct(&e, "ann");
    e.fake.push([say("one"), say("two")]);
    e.agent.send(ch, "jasmine tea", None).await.unwrap();
    e.agent.send(ch, "more", None).await.unwrap();
    let m = meter(&e.agent);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].2, 0, "first call: nothing cached");
    assert!(
        m[1].2 > 0,
        "second call reads the first one's prefix from cache"
    );
    let now = T0;
    let report = e.agent.with_world(|w| {
        let status =
            world::quota::status(w, world::Tier::Middle, now, &Default::default()).unwrap();
        agent::meter::report(w, now, world::quota::WINDOW_MS, Some(&status)).unwrap()
    });
    let get = |k: &str| report.iter().find(|(key, _)| key == k).map(|(_, v)| *v);
    assert!(get("inject.direct").unwrap() > 0.0);
    let hit = get("inject.cache_hit").unwrap();
    assert!(hit > 0.0 && hit < 1.0);
    assert!(get("quota.window_use").is_some());
    // Points follow the register's price ratios: uncached + c * cached + o * output.
    let points: Vec<f64> = e.agent.with_world(|w| {
        let mut s = w.prepare("SELECT points FROM meter ORDER BY id").unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    });
    let expect = m[1].1 as f64 + 0.25 * m[1].2 as f64 + 4.0 * m[1].3 as f64;
    assert!((points[1] - expect).abs() < 1e-9);
    let totals = e
        .agent
        .with_world(|w| world::quota::totals_by_shape(w, 0, now).unwrap());
    assert_eq!(totals[0].0, Shape::Direct);
    assert_eq!(totals[0].1.calls, 2);
}

#[tokio::test]
async fn cache_writes_on_the_messages_protocol_cost_a_third_term() {
    // The fake reports 3 fresh input tokens and the rest of what the prefix has not read
    // as written to the cache. `cost.w` prices a write when the model has no price.
    for (w, priced) in [(1.25, false), (2.0, true)] {
        let mut ep = endpoints(WireApi::Messages, true, None);
        if priced {
            let m = &mut ep.profiles[0].models[0];
            (
                m.price_in,
                m.price_in_cached,
                m.price_in_cache_write,
                m.price_out,
            ) = (Some(2.0), Some(0.5), Some(4.0), Some(8.0));
        }
        let e = env_with(ep, |_| {});
        let ch = direct(&e, "ann");
        e.fake.push([say("one"), say("two")]);
        e.agent.send(ch, "jasmine tea", None).await.unwrap();
        e.agent.send(ch, "more", None).await.unwrap();
        let m = meter(&e.agent);
        let points: Vec<f64> = e.agent.with_world(|w| {
            let mut s = w.prepare("SELECT points FROM meter ORDER BY id").unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        });
        let (uncached, cached, output) = (m[1].1 as f64, m[1].2 as f64, m[1].3 as f64);
        let written = uncached - 3.0;
        assert!(written > 0.0 && cached > 0.0, "{m:?}");
        // Ratios: c and o from the prices when there are any (0.25, 4 either way).
        let expect = 3.0 + w * written + 0.25 * cached + 4.0 * output;
        assert!(
            (points[1] - expect).abs() < 1e-9,
            "{} vs {expect}",
            points[1]
        );
    }
}

#[tokio::test]
async fn an_exhausted_window_stops_calls_with_the_hosts_fixed_line() {
    let e = env_with(endpoints(WireApi::Responses, true, None), |c| {
        c.world.quota.window_1h.middle = 10.0;
        c.world.quota.call_points = 1.0;
    });
    let ch = direct(&e, "ann");
    e.fake.push([say("one")]);
    e.agent.send(ch, "hi", None).await.unwrap();
    let err = e.agent.send(ch, "again", None).await.unwrap_err();
    assert!(matches!(
        err,
        agent::Error::QuotaExhausted {
            release_at: Some(_)
        }
    ));
    assert_eq!(e.fake.bodies().len(), 1, "no call was made");
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    let line = e.agent.history(host, None, 5).unwrap().pop().unwrap();
    assert!(
        line.text
            .starts_with("The quota for this period is used up")
    );
    // The user's message is kept.
    assert!(
        e.agent
            .history(ch, None, 5)
            .unwrap()
            .iter()
            .any(|m| m.text == "again")
    );
}

#[tokio::test]
async fn nobody_without_a_persona_is_ever_given_a_session() {
    let e = env(WireApi::Responses);
    assert!(e.agent.has_persona("ann") && !e.agent.has_persona("cy"));
    assert!(matches!(
        e.agent.open_direct(&p("cy")),
        Err(agent::Error::NoPersona(_))
    ));
    assert!(matches!(
        e.agent.ask(&p("cy"), "q", None, None),
        Err(agent::Error::NoPersona(_))
    ));
    assert!(matches!(
        e.agent.create_group(&[p("ann"), p("cy")], None),
        Err(agent::Error::NoPersona(_))
    ));
    let enable = api::types::Target::Person { id: "cy".into() };
    assert!(
        e.agent
            .set_mode(&enable, api::types::Mode::Enabled)
            .is_err()
    );
    // A group that already holds him (made elsewhere): he is never addressed.
    let g = e.agent.with_world(|w| {
        channel::create_group(w, &[p("ann"), p("cy")], None, Origin::User).unwrap()
    });
    e.fake.push([say("ann answers")]);
    e.agent
        .send(g, "cy, the tower over the river?", None)
        .await
        .unwrap();
    let entry = list(&e.fake.json()[0]).last().unwrap().to_string();
    assert!(entry.contains("ann answers."), "{entry}");
    assert!(!body_text(&segments(&e.agent, g)[0].prefix_b).contains("You are cy."));
    // Not a colleague to pull in, not someone the host can find or ask.
    let ch = direct(&e, "ann");
    e.fake
        .push([call("request_join", json!({"person": "cy"})), say("no one")]);
    e.agent.send(ch, "get cy", None).await.unwrap();
    assert!(log_bytes(&e.agent, ch).contains("Nobody suitable is available."));
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        call("find_people", json!({"topic": "tower river"})),
        call("ask", json!({"person": "cy", "question": "tower?"})),
        say("nobody"),
    ]);
    e.agent
        .send(host, "who knows the tower?", None)
        .await
        .unwrap();
    let log = log_bytes(&e.agent, host);
    assert!(!log.contains("(cy)"), "find_people never offers him");
    assert!(log.contains("Nobody suitable is available."));
}

#[tokio::test]
async fn presence_opens_with_a_seed_as_a_harness_turn() {
    let e = env(WireApi::Responses);
    e.agent.with_world(|w| {
        bond::set_mode(w, &p("ann"), Mode::Enabled).unwrap();
        settings::set(w, settings::USER_BIRTHDAY, "03-10").unwrap();
    });
    // The first open is the welcome alone.
    let first = e.agent.presence(true).await.unwrap();
    assert!(first.host_spoke && first.opened.is_none() && e.fake.bodies().is_empty());
    e.clock.advance(31 * 60_000);
    e.fake
        .push([say("Ann's birthday."), say("Happy birthday, Dr.Doc.")]);
    let out = e.agent.presence(true).await.unwrap();
    assert!(out.host_spoke);
    let (who, ch) = out.opened.unwrap();
    assert_eq!(who, p("ann"));
    let entry = list(&e.fake.json()[1]).last().unwrap().to_string();
    assert!(entry.contains("ann speaks first.") && entry.contains("Doc's birthday"));
    let msgs = e.agent.history(ch, None, 5).unwrap();
    assert_eq!(msgs[0].text, "Happy birthday, Dr.Doc.");
    assert!(
        e.sink
            .take()
            .iter()
            .any(|ev| matches!(ev, agent::Event::Notification(_)))
    );
    assert_eq!(meter(&e.agent).last().unwrap().0, "opening");
    // Used once: the next presence has no seed. His opening session idled out while
    // the user was away, so it is wrapped up first, then the host speaks.
    e.clock.advance(31 * 60_000);
    e.fake
        .push([call("wrapup", json!({"facts": []})), say("Ann wrote.")]);
    let again = e.agent.presence(false).await.unwrap();
    assert!(again.opened.is_none() && again.host_spoke);
    assert_eq!(e.fake.left(), 0);
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["host", "opening", "wrapup", "host"]);
}

#[tokio::test]
async fn the_host_announces_each_waiting_item_once_even_across_a_restart() {
    let dir = tempdir::Dir::new();
    let file = dir.0.join("world.db");
    let open = || {
        let e = env_on(
            world::World::open(&file).unwrap(),
            endpoints(WireApi::Responses, true, None),
            retrieve(),
            |_| {},
        );
        e.agent
            .with_world(|w| bond::set_mode(w, &p("ann"), Mode::Enabled).unwrap());
        e
    };
    let e = open();
    // The first open is the pre-written welcome; the birthday is then news on the next return.
    assert!(e.agent.presence(true).await.unwrap().host_spoke);
    assert_eq!(e.fake.bodies().len(), 0);
    e.clock.advance(31 * 60_000);
    e.fake.push([say("It is ann's birthday.")]);
    assert!(e.agent.presence(false).await.unwrap().host_spoke);
    let calls = e.fake.bodies().len();

    // Back again and again with nothing new: the panel is full, the host is silent.
    for _ in 0..3 {
        e.clock.advance(31 * 60_000);
        let out = e.agent.presence(true).await.unwrap();
        assert!(!out.host_spoke && !out.digest.items.is_empty());
    }
    assert_eq!(e.fake.bodies().len(), calls);

    // A restart on the same world is no different, and the introduction does not repeat.
    drop(e);
    let e = open();
    let out = e.agent.presence(true).await.unwrap();
    assert!(!out.host_spoke && !out.digest.items.is_empty());
    assert_eq!(e.fake.bodies().len(), 0);

    // A new item is news: exactly one call, and it names only that item.
    let ch = direct(&e, "ann");
    e.agent.with_world(|w| {
        world::message::append(w, ch, &Actor::Person(p("ann")), "hi", None, None, T0).unwrap();
    });
    e.clock.advance(31 * 60_000);
    e.fake.push([say("Ann wrote.")]);
    assert!(e.agent.presence(false).await.unwrap().host_spoke);
    let bodies = e.fake.json();
    assert_eq!(bodies.len(), 1);
    let entry = list(&bodies[0]).last().unwrap().to_string();
    assert!(!entry.contains("birthday"), "{entry}");

    // The next day the birthday is over and nothing else is new.
    e.clock.advance(24 * 3_600_000);
    let out = e.agent.presence(false).await.unwrap();
    assert!(!out.host_spoke);
    assert_eq!(e.fake.bodies().len(), 1);
}

#[test]
fn flows_are_send() {
    fn send<T: Send>(_: &T) {}
    let e = env(WireApi::Chat);
    let posted = agent::Posted {
        channel: ChannelId(1),
        message: world::MessageId(1),
        text: String::new(),
        image: None,
        authorized: false,
    };
    send(&e.agent.respond(posted));
    send(&e.agent.presence(true));
    send(&e.agent.wrapup(ChannelId(1)));
    send(&e.agent.on_rename());
    send(&e.agent.on_event());
}

#[tokio::test]
async fn tick_wraps_up_idle_conversations_and_nothing_else() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([say("hi")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    assert_eq!(e.agent.tick(false).await.unwrap(), None);
    assert_eq!(e.fake.bodies().len(), 1, "not idle yet: no call");
    e.clock.advance(31 * 60_000);
    e.fake.push([call(
        "wrapup",
        json!({"facts": [{"kind": "fact", "text": "Said hello"}]}),
    )]);
    e.agent.tick(false).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 2);
    assert_prefix_chain(&e.fake.bodies());
    assert_eq!(
        e.agent.with_world(|w| fact::all(w, false).unwrap()).len(),
        1
    );
    e.agent.tick(false).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 2, "wrapped up once");
}

#[tokio::test]
async fn a_colleague_never_reads_memories_he_was_not_there_for() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.agent.with_world(|w| {
        let new = NewFact::new(
            Actor::User,
            Audience::Own(p("ann")),
            FactKind::Fact,
            "ann secret jasmine recipe",
        );
        fact::write(w, &new, T0).unwrap();
    });
    e.fake.push([say("Mm.")]);
    e.agent.send(ch, "jasmine tea", None).await.unwrap();
    assert!(
        log_bytes(&e.agent, ch).contains("secret"),
        "alone with ann it is hers"
    );
    e.fake.push([
        call("request_join", json!({"person": "bo"})),
        call("wrapup", json!({"facts": []})),
        say("Bo: hello."),
    ]);
    e.agent
        .send(ch, "ask bo about the forge blade", None)
        .await
        .unwrap();
    let segs = segments(&e.agent, ch);
    assert_eq!(segs.len(), 2, "rolled before bo spoke");
    let bodies = e.fake.bodies();
    let bo = body_text(bodies.last().unwrap());
    assert!(bo.contains("You are bo.") && !bo.contains("secret"), "{bo}");
    assert!(!body_text(&segs[1].prefix_c).contains("secret"));
    assert!(body_text(&segs[1].prefix_c).contains("ask bo about the forge blade"));
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["direct", "direct", "wrapup", "direct"]);
}

#[tokio::test]
async fn a_log_continues_across_a_change_of_wire_api() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([say("one"), say("two"), say("three")]);
    e.agent.send(ch, "jasmine tea", None).await.unwrap();
    // The user points the chat role at another protocol: the log goes on, only the
    // next call misses the cache, and the call after it extends that one again.
    e.agent.set_endpoints(endpoints(WireApi::Chat, true, None));
    e.agent.send(ch, "more", None).await.unwrap();
    e.agent.send(ch, "and more", None).await.unwrap();
    assert_eq!(segments(&e.agent, ch).len(), 1);
    let bodies = e.fake.bodies();
    let (first, second) = (e.fake.json()[0].clone(), e.fake.json()[1].clone());
    assert_ne!(
        head(&first),
        head(&second),
        "another protocol, another body"
    );
    assert!(
        !e.fake.json()[1].to_string().contains("enc-0"),
        "the old protocol's reasoning item is not sent to the new one"
    );
    assert_prefix_chain(&bodies[1..]);
    let v = &e.fake.json()[2];
    let all = list(v).iter().map(Value::to_string).collect::<String>();
    assert!(
        all.contains("one") && all.contains("two"),
        "the whole log is sent"
    );
}

#[tokio::test]
async fn a_due_commitment_opens_on_its_own_log_even_for_a_frozen_person() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.agent.with_world(|w| {
        let new = NewFact::new(
            Actor::Person(p("ann")),
            Audience::Participants(ch),
            FactKind::Commitment,
            "Tea together at noon",
        )
        .due(T0 - 60_000);
        fact::write(w, &new, T0 - 3_600_000).unwrap();
    });
    e.fake.push([say("It is noon, Dr.Doc. Tea?")]);
    let opened = e.agent.on_event().await.unwrap();
    assert_eq!(opened, Some((p("ann"), ch)));
    let entry = list(&e.fake.json()[0]).last().unwrap().to_string();
    assert!(
        entry.contains("A promise is due: Tea together at noon"),
        "{entry}"
    );
    assert_eq!(meter(&e.agent)[0].0, "opening");
    let due = e.agent.with_world(|w| fact::all(w, false).unwrap());
    assert!(due[0].delivered, "brought up once");
    assert_eq!(e.agent.on_event().await.unwrap(), None);
}

#[tokio::test]
async fn a_relay_is_posted_as_a_retelling_in_the_recipients_channel() {
    let e = env(WireApi::Responses);
    let posted = e
        .agent
        .relay("Ann said the water must be cooler.", &p("bo"))
        .unwrap();
    e.fake.push([say("Cooler water, then.")]);
    e.agent.respond(posted.clone()).await.unwrap();
    let msgs = e.agent.history(posted.channel, None, 5).unwrap();
    assert!(msgs[0].relayed && msgs[0].text == "Ann said the water must be cooler.");
    assert_eq!(msgs[1].text, "Cooler water, then.");
    assert!(matches!(
        e.agent.relay("x", &p("cy")),
        Err(agent::Error::NoPersona(_))
    ));
}

fn models_sent(e: &Env) -> Vec<String> {
    e.fake
        .json()
        .iter()
        .map(|v| v["model"].as_str().unwrap().to_owned())
        .collect()
}

fn metered_models(agent: &agent::Agent) -> Vec<(String, String)> {
    agent.with_world(|w| {
        let mut stmt = w
            .prepare("SELECT shape, model FROM meter ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

#[tokio::test]
async fn the_host_role_serves_the_host_and_chat_serves_persons() {
    for wire in WIRES {
        let e = env_with(endpoints_with_host(wire), |_| {});
        let host = e.agent.with_world(|w| channel::host(w).unwrap());
        let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
        e.agent
            .with_world(|w| channel::add_participant(w, g, &Actor::Host).unwrap());
        let ch = direct(&e, "ann");
        e.fake
            .push([say("host hi"), say("group hi"), say("ann hi")]);
        e.agent.send(host, "hello", None).await.unwrap();
        e.agent.send(g, "jasmine tea", None).await.unwrap();
        e.agent.send(ch, "jasmine tea", None).await.unwrap();
        assert_eq!(models_sent(&e), ["model-h", "model-a", "model-a"]);
        let metered = metered_models(&e.agent);
        assert_eq!(
            metered.iter().map(|m| m.1.as_str()).collect::<Vec<_>>(),
            ["model-h", "model-a", "model-a"],
            "{metered:?}"
        );
    }
}

#[tokio::test]
async fn without_a_host_role_the_host_uses_chat() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([say("host hi")]);
    e.agent.send(host, "hello", None).await.unwrap();
    assert_eq!(models_sent(&e), ["model-a"]);
}

#[tokio::test]
async fn host_tool_actions_carry_what_a_front_end_needs_to_offer_a_switch() {
    use agent::Event;
    use api::views::ToolAction;
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        call("create_group", json!({"members": ["ann", "bo"]})),
        call("find_people", json!({"topic": "forge blade"})),
        say("The group is ready; you can open it from here."),
    ]);
    e.agent.send(host, "make a group", None).await.unwrap();
    let actions: Vec<ToolAction> = e
        .sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            Event::ToolCalled(t) => Some(t.action),
            _ => None,
        })
        .collect();
    let groups = e.agent.with_world(|w| {
        w.query_row("SELECT id FROM channel WHERE kind = 'group'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    });
    assert!(
        actions.iter().any(|a| matches!(a,
            ToolAction::CreateGroup { members, channel: Some(c), .. }
                if members.len() == 2 && c.0 == groups.to_string())),
        "{actions:?}"
    );
    assert!(
        actions.iter().any(|a| matches!(a,
            ToolAction::FindPeople { persons, .. } if !persons.is_empty())),
        "{actions:?}"
    );
    // The model is told the id, and a stored message round-trips without it too.
    assert!(log_bytes(&e.agent, host).contains(&format!("Group {groups} created.")));
    let old: ToolAction = serde_json::from_str(r#"{"tool":"create_group","members":[]}"#).unwrap();
    assert!(matches!(old, ToolAction::CreateGroup { channel: None, .. }));
}

#[tokio::test]
async fn list_channels_names_the_conversations_and_offers_each() {
    use agent::Event;
    use api::views::{ChannelKind, ToolAction};
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    // A direct channel never written to is not a conversation yet.
    let _unused = direct(&e, "bo");
    let ann = direct(&e, "ann");
    e.fake.push([say("Ann: hello.")]);
    e.agent.send(ann, "hi", None).await.unwrap();
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.fake.push([say("Ann: here."), say("Bo: here.")]);
    e.agent.send(g, "everyone?", None).await.unwrap();
    e.sink.take();

    e.fake.push([
        call("list_channels", json!({})),
        say("Two are open; pick one from the choices."),
    ]);
    e.agent
        .send(host, "what can I go back to?", None)
        .await
        .unwrap();
    let listed = e
        .sink
        .take()
        .into_iter()
        .find_map(|ev| match ev {
            Event::ToolCalled(t) => match t.action {
                ToolAction::ListChannels { channels } => Some(channels),
                _ => None,
            },
            _ => None,
        })
        .expect("a list_channels action");
    // The host's own channel and the empty one are left out. The fake clock does not
    // advance, so the order between the two is not asserted here.
    assert_eq!(listed.len(), 2);
    let group = listed
        .iter()
        .find(|c| c.kind == ChannelKind::Group)
        .unwrap();
    let direct = listed
        .iter()
        .find(|c| c.kind == ChannelKind::Direct)
        .unwrap();
    assert_eq!(group.id.0, g.to_string());
    assert_eq!(direct.members.len(), 1);
    let log = log_bytes(&e.agent, host);
    assert!(log.contains("group of") && log.contains("with "), "{log}");
    // The usage guide is the host's alone.
    assert!(log.contains("operates this place"));
    assert!(!log_bytes(&e.agent, ann).contains("operates this place"));
    assert!(!log_bytes(&e.agent, g).contains("operates this place"));
}

// ---- hand-offs: a reply may end with a line naming who speaks next ----

fn texts(e: &Env, ch: world::ChannelId) -> Vec<(api::types::Author, String)> {
    e.agent
        .history(ch, None, 50)
        .unwrap()
        .into_iter()
        .map(|m| (m.author, m.text))
        .collect()
}

#[tokio::test]
async fn a_reply_can_hand_the_turn_to_another_member() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.fake
        .push([say("Ann: bo knows.\n\u{2192} bo"), say("Bo: steel.")]);
    e.agent
        .send(g, "ann, where is the steel?", None)
        .await
        .unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 2);
    let second = list(&json[1]).last().unwrap().to_string();
    assert!(second.contains("bo answers too"), "{second}");
    // The group's log explains the line; the stored words never carry it.
    let segs = segments(&e.agent, g);
    assert!(body_text(&segs[0].prefix_a).contains("hand the turn on"));
    let said = texts(&e, g);
    assert!(
        said.iter().all(|(_, t)| !t.contains('\u{2192}')),
        "{said:?}"
    );
    assert!(said.iter().any(|(_, t)| t == "Ann: bo knows."));
    assert!(said.iter().any(|(_, t)| t == "Bo: steel."));
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["group", "interject"]);
}

#[tokio::test]
async fn a_chain_never_repeats_a_speaker() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.fake.push([
        say("Ann: over to bo.\n\u{2192} bo"),
        say("Bo: back to ann.\n\u{2192} ann, bo"),
        say("Ann: never asked."),
    ]);
    e.agent.send(g, "ann, hello", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 2);
    assert_eq!(e.fake.left(), 1);
}

#[tokio::test]
async fn a_hand_off_chain_stops_at_the_cap() {
    let e = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.agent.group.collective_max = 2;
    });
    let g = three(&e);
    e.fake.push([
        say("Ann: to bo.\n\u{2192} bo"),
        say("Bo: to dee.\n\u{2192} dee"),
        say("Dee: too late."),
    ]);
    e.agent.send(g, "ann, hello", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 2);

    // Handed members queue behind the remaining first speakers, then the cap counts all.
    let e = classifying(3);
    let g = three(&e);
    e.fake.push([
        say(r#"{"addressed": ["ann", "bo"]}"#),
        say("Ann: to dee.\n\u{2192} dee"),
        say("Bo: second."),
        say("Dee: third."),
        say("unused"),
    ]);
    e.agent.send(g, "ann and bo, hello", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 4, "address call plus three replies");
    let said: Vec<String> = texts(&e, g).into_iter().map(|(_, t)| t).collect();
    let pos = |s: &str| said.iter().position(|t| t.starts_with(s)).unwrap();
    assert!(pos("Ann") < pos("Bo") && pos("Bo") < pos("Dee"), "{said:?}");
}

#[tokio::test]
async fn hand_offs_to_strangers_the_user_and_the_host_are_ignored() {
    let e = env(WireApi::Chat);
    // Cy has no persona and is not in the group; bo is not either.
    let g = e.agent.create_group(&[p("ann"), p("dee")], None).unwrap();
    e.fake.push([
        say("Ann: hm.\n\u{2192} bo, cy, Doc, Hostess, nobody"),
        say("unused"),
    ]);
    e.agent.send(g, "ann, hello", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
    let said = texts(&e, g);
    assert!(
        said.iter().all(|(_, t)| !t.contains('\u{2192}')),
        "{said:?}"
    );
}

#[tokio::test]
async fn a_disabled_member_is_not_handed_the_turn() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.agent
        .with_world(|w| bond::set_mode(w, &p("bo"), Mode::Disabled).unwrap());
    e.fake.push([say("Ann: hm.\n\u{2192} bo"), say("unused")]);
    e.agent.send(g, "ann, hello", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
}

#[tokio::test]
async fn a_hand_off_round_ends_with_the_quota_band() {
    let e = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.world.quota.quiet_at = 0.0;
    });
    let g = three(&e);
    e.fake.push([say("Ann: hm.\n\u{2192} bo"), say("unused")]);
    e.agent.send(g, "ann, hello", None).await.unwrap();
    assert_eq!(e.fake.bodies().len(), 1);
}

#[tokio::test]
async fn the_hand_off_line_is_never_streamed() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.fake
        .push([say("Ann: bo knows.\n\u{2192} bo"), say("Bo: steel.")]);
    e.agent
        .send(g, "ann, where is the steel?", None)
        .await
        .unwrap();
    let streamed: Vec<String> = e
        .sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            agent::Event::MessageDelta(d) => Some(d.delta),
            _ => None,
        })
        .collect();
    assert!(!streamed.is_empty());
    assert!(
        streamed.iter().all(|d| !d.contains('\u{2192}')),
        "{streamed:?}"
    );
    let all = streamed.concat();
    assert!(
        all.contains("Ann: bo knows.") && all.contains("Bo: steel."),
        "{all}"
    );
}

#[tokio::test]
async fn an_ordinary_interjection_hands_the_turn_to_nobody() {
    let e = env(WireApi::Chat);
    let g = e
        .agent
        .create_group(&[p("ann"), p("bo")], Some("smithing"))
        .unwrap();
    e.agent.with_world(|w| {
        channel::set_mode(w, g, Mode::Enabled).unwrap();
        bond::set_mode(w, &p("ann"), Mode::Enabled).unwrap();
        bond::set_mode(w, &p("bo"), Mode::Enabled).unwrap();
    });
    e.fake.push([
        say("Ann: by name."),
        say("Bo: a line.\n\u{2192} ann"),
        say("unused"),
    ]);
    e.agent
        .send(g, "ann, jasmine tea at the forge blade?", None)
        .await
        .unwrap();
    assert_eq!(e.fake.bodies().len(), 2);
    let said = texts(&e, g);
    assert!(said.iter().any(|(_, t)| t == "Bo: a line."), "{said:?}");
}

#[tokio::test]
async fn a_reply_that_opens_with_the_previous_message_keeps_only_its_own_words() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.fake.push([
        say("Ann: the steel is in the hold.\n\u{2192} bo"),
        say("Ann: the steel is in the hold.\n\nBo: and the hold is locked.\n[#u1]\n"),
    ]);
    e.agent
        .send(g, "ann, where is the steel?", None)
        .await
        .unwrap();
    let said = texts(&e, g);
    assert!(
        said.iter()
            .any(|(_, t)| t.starts_with("Bo: and the hold is locked.")),
        "{said:?}"
    );
    assert_eq!(
        said.iter()
            .filter(|(_, t)| t.contains("the steel is in the hold"))
            .count(),
        1,
        "{said:?}"
    );
}

#[tokio::test]
async fn a_hand_off_line_before_the_citation_line_still_hands_the_turn_on() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.fake
        .push([say("Ann: ask bo.\n\u{2192} bo\n[#u1]"), say("Bo: steel.")]);
    e.agent
        .send(g, "ann, where is the steel?", None)
        .await
        .unwrap();
    let said = texts(&e, g);
    assert!(said.iter().any(|(_, t)| t == "Bo: steel."), "{said:?}");
    assert!(
        said.iter().all(|(_, t)| !t.contains('\u{2192}')),
        "{said:?}"
    );
}

fn host_ask(person: &str, question: &str) -> Reply {
    call(
        "ask",
        json!({"person": person, "question": question, "turns": 2}),
    )
}

fn last_entry(e: &Env) -> String {
    list(e.fake.json().last().unwrap())
        .last()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_request_the_host_makes_runs_and_its_reply_lands_in_the_persons_channel() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        host_ask("ann", "How is jasmine tea brewed?"),
        say("Ann is looking into it."),
        call("report", json!({"text": "Cooler water.", "cites": []})),
        call("wrapup", json!({"facts": []})),
    ]);
    e.agent.send(host, "ask ann about tea", None).await.unwrap();
    let t = e
        .agent
        .with_world(|w| world::task::list(w).unwrap().remove(0));
    assert_eq!(t.status, world::TaskStatus::Done);
    assert_eq!(t.failure, None);
    assert!(t.reply.is_some());
    let ann = direct(&e, "ann");
    let msgs = e.agent.history(ann, None, 10).unwrap();
    assert_eq!(msgs.last().unwrap().text, "Cooler water.");
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["host", "host", "ask", "wrapup"]);
}

#[tokio::test]
async fn a_host_turn_that_fails_after_asking_marks_the_request_failed_and_the_host_is_told() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    // The request is made, then the call that would carry the host's answer fails.
    e.fake.push([
        host_ask("ann", "How is tea brewed?"),
        Reply::Fail("boom".into()),
    ]);
    let err = e.agent.send(host, "ask ann about tea", None).await;
    assert!(err.is_err());
    let t = e
        .agent
        .with_world(|w| world::task::list(w).unwrap().remove(0));
    assert_eq!(
        (t.status, t.failure),
        (world::TaskStatus::Done, Some(world::task::Failure::Failed))
    );
    assert!(t.reply.is_none());
    // Nothing ran in her channel.
    assert!(
        e.agent
            .history(direct(&e, "ann"), None, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        e.agent
            .with_world(|w| world::task::open_requests(w).unwrap())
            .is_empty()
    );
    // On its next turn the host is shown the failure, once.
    e.fake.push([say("It never ran."), say("Still nothing.")]);
    e.agent.send(host, "how is it going?", None).await.unwrap();
    let entry = last_entry(&e);
    assert!(
        entry.contains("did not run") && entry.contains("How is tea brewed?"),
        "{entry}"
    );
    e.agent.send(host, "and now?", None).await.unwrap();
    assert!(!last_entry(&e).contains("did not run"));
}

#[tokio::test]
async fn a_request_that_fails_to_run_is_marked_failed_and_later_ones_are_not_run() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        host_ask("ann", "first?"),
        host_ask("ann", "second?"),
        say("Both asked."),
        Reply::Fail("boom".into()),
    ]);
    assert!(e.agent.send(host, "ask ann twice", None).await.is_err());
    let tasks = e.agent.with_world(|w| world::task::list(w).unwrap());
    assert_eq!(tasks.len(), 2);
    assert!(
        tasks
            .iter()
            .all(|t| t.failure == Some(world::task::Failure::Failed))
    );
    assert_eq!(e.fake.left(), 0);
    assert_eq!(e.fake.bodies().len(), 4, "the second request made no call");
    // A request run directly that errors is marked too.
    let asked = e.agent.ask(&p("ann"), "third?", Some(2), None).unwrap();
    e.fake.push([Reply::Fail("boom".into())]);
    assert!(e.agent.run_request(asked).await.is_err());
    let t = e
        .agent
        .with_world(|w| world::task::get(w, asked.task).unwrap());
    assert_eq!(t.failure, Some(world::task::Failure::Failed));
}

#[tokio::test]
async fn a_failed_first_request_also_fails_the_second_and_the_host_hears_each_once() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        host_ask("ann", "first question?"),
        host_ask("bo", "second question?"),
        say("Both asked."),
        Reply::Fail("boom".into()),
    ]);
    assert!(e.agent.send(host, "ask ann and bo", None).await.is_err());
    let tasks = e.agent.with_world(|w| world::task::list(w).unwrap());
    assert_eq!(tasks.len(), 2);
    for t in &tasks {
        assert_eq!(
            (t.status, t.failure),
            (world::TaskStatus::Done, Some(world::task::Failure::Failed))
        );
    }
    // Neither person's channel saw a request.
    for who in ["ann", "bo"] {
        assert!(
            e.agent
                .history(direct(&e, who), None, 10)
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        e.agent
            .with_world(|w| world::task::open_requests(w).unwrap())
            .is_empty()
    );

    e.fake.push([say("Neither ran."), say("Still neither.")]);
    e.agent.send(host, "any news?", None).await.unwrap();
    let entry = last_entry(&e);
    for q in ["first question?", "second question?"] {
        assert_eq!(entry.matches(q).count(), 1, "{entry}");
    }
    assert_eq!(entry.matches("did not run").count(), 2, "{entry}");
    e.agent.send(host, "and now?", None).await.unwrap();
    let entry = last_entry(&e);
    assert!(!entry.contains("first question?") && !entry.contains("did not run"));
}

#[tokio::test]
async fn the_host_sees_each_open_request_with_its_turns_left() {
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.agent
        .ask(&p("ann"), "How is tea brewed?", Some(5), None)
        .unwrap();
    e.fake.push([say("Not yet.")]);
    e.agent.send(host, "any news?", None).await.unwrap();
    let entry = last_entry(&e);
    assert!(
        entry.contains("How is tea brewed?") && entry.contains("still open, 5 turns left"),
        "{entry}"
    );
}

#[tokio::test]
async fn only_requests_no_live_process_owns_are_interrupted_at_start() {
    let folio_file = corpus();
    let index = index::Index::open(folio_file.path(), retrieve()).unwrap();
    let world = world::World::in_memory().unwrap();
    let asked = world::task::ask(
        &world,
        &p("ann"),
        "left over",
        Some(3),
        None,
        world::Tier::Middle,
        &world::params::Ask::default(),
        T0 - world::task::LEASE_MS,
    )
    .unwrap();
    // Another process (the terminal next to the app) is working on this one right now.
    let alive = world::task::ask(
        &world,
        &p("bo"),
        "in flight",
        Some(3),
        None,
        world::Tier::Middle,
        &world::params::Ask::default(),
        T0 - 1000,
    )
    .unwrap();
    let agent = agent::Agent::new(agent::Parts {
        world,
        index,
        config: agent::Config::default(),
        endpoints: endpoints(WireApi::Chat, true, None),
        keys: None,
        llm: llm::Llm::new(Arc::new(Fake::default()), llm::Retry::none()),
        sink: Arc::new(Recorder::default()),
        clock: Arc::new(TestClock(std::sync::atomic::AtomicI64::new(T0))),
    })
    .unwrap();
    let t = agent.with_world(|w| world::task::get(w, asked.task).unwrap());
    assert_eq!(
        (t.status, t.failure),
        (
            world::TaskStatus::Done,
            Some(world::task::Failure::Interrupted)
        )
    );
    let t = agent.with_world(|w| world::task::get(w, alive.task).unwrap());
    assert_eq!((t.status, t.failure), (world::TaskStatus::Active, None));
}

/// The user entries of the last request, as texts.
fn user_texts(e: &Env) -> Vec<String> {
    let last = e.fake.json().pop().unwrap();
    list(&last)
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn every_user_entry_carries_the_clock_and_a_long_silence() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("One."), say("Two."), say("Three.")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    // 12:20 on a Tuesday: no gap worth a word.
    e.clock.advance(20 * 60_000);
    e.agent.send(ch, "what time is it?", None).await.unwrap();
    e.clock.advance(30 * 3_600_000);
    e.agent.send(ch, "i am back", None).await.unwrap();
    let texts = user_texts(&e);
    assert_eq!(texts.len(), 3);
    assert!(
        texts[0].ends_with("[2026-03-10 Tuesday 12:00, midday]"),
        "{}",
        texts[0]
    );
    assert!(
        texts[1].ends_with("[2026-03-10 Tuesday 12:20, midday]"),
        "{}",
        texts[1]
    );
    assert!(
        texts[2].ends_with(
            "[2026-03-11 Wednesday 18:20, evening] (Doc's previous message was 30 hours ago)"
        ),
        "{}",
        texts[2]
    );
    // Entries already in the log are never rewritten: the prefix chain holds.
    assert_prefix_chain(&e.fake.bodies());
}

#[tokio::test]
async fn a_late_hour_reads_as_night_and_days_are_counted_from_two() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("One."), say("Two.")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    e.clock
        .advance(3 * 86_400_000 + 11 * 3_600_000 + 30 * 60_000);
    e.agent.send(ch, "still there?", None).await.unwrap();
    let last = user_texts(&e).pop().unwrap();
    assert!(
        last.ends_with("[2026-03-13 Friday 23:30, night] (Doc's previous message was 3 days ago)"),
        "{last}"
    );
}

const CLOCK_NOON: &str = "[2026-03-10 Tuesday 12:00, midday]";

#[tokio::test]
async fn the_clock_line_is_in_group_and_interject_entries_and_never_in_the_prefix() {
    let e = env(WireApi::Chat);
    let g = e
        .agent
        .create_group(&[p("ann"), p("bo")], Some("smithing"))
        .unwrap();
    e.agent.with_world(|w| {
        channel::set_mode(w, g, Mode::Enabled).unwrap();
        bond::set_mode(w, &p("ann"), Mode::Enabled).unwrap();
        bond::set_mode(w, &p("bo"), Mode::Enabled).unwrap();
    });
    e.fake.push([
        say("Ann: by name."),
        say("Bo: a line."),
        say("Ann: later."),
        say("Bo: later too."),
    ]);
    e.agent
        .send(g, "ann, jasmine tea at the forge blade?", None)
        .await
        .unwrap();
    let before = segments(&e.agent, g).remove(0);
    e.clock.advance(20 * 60_000);
    e.agent
        .send(g, "ann, jasmine tea at the forge blade again?", None)
        .await
        .unwrap();
    let json = e.fake.json();
    assert_eq!(json.len(), 4);
    let last = |i: usize| list(&json[i]).last().unwrap()["content"].to_string();
    for i in [0, 1] {
        assert!(last(i).ends_with(&format!("{CLOCK_NOON}\"")), "{}", last(i));
    }
    assert!(last(1).contains("may add one line"), "{}", last(1));
    for i in [2, 3] {
        assert!(
            last(i).ends_with("[2026-03-10 Tuesday 12:20, midday]\""),
            "{}",
            last(i)
        );
    }
    // The same prefix bytes on every turn: the clock is only ever in entries.
    let after = segments(&e.agent, g).remove(0);
    assert_eq!(
        (&before.prefix_a, &before.prefix_b, &before.prefix_c),
        (&after.prefix_a, &after.prefix_b, &after.prefix_c)
    );
    for block in [&after.prefix_a, &after.prefix_b, &after.prefix_c] {
        assert!(!body_text(block).contains("Tuesday"));
    }
    assert_prefix_chain(&e.fake.bodies());
}

#[tokio::test]
async fn a_chorus_reply_carries_the_clock_line_too() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    e.fake
        .push([say("Ann: bo knows.\n\u{2192} bo"), say("Bo: steel.")]);
    e.agent
        .send(g, "ann, where is the steel?", None)
        .await
        .unwrap();
    let json = e.fake.json();
    let second = list(&json[1]).last().unwrap()["content"].to_string();
    assert!(second.contains("bo answers too"), "{second}");
    assert!(second.ends_with(&format!("{CLOCK_NOON}\"")), "{second}");
}

#[tokio::test]
async fn a_reply_may_open_with_a_short_quote_and_a_new_round_never_trims_against_an_old_one() {
    let e = env(WireApi::Chat);
    let g = three(&e);
    // The user's message is shorter than the guard's threshold: a quote of it stays.
    e.fake.push([
        say("Ann, hi to you. The steel is in the hold.\n\u{2192} bo"),
        say("Bo: and the hold is locked."),
        // Next round: the first speaker opens with the last reply of the round before.
        say("Bo: and the hold is locked. I agree."),
    ]);
    e.agent.send(g, "Ann, hi", None).await.unwrap();
    e.agent
        .send(g, "bo, any news on the hold?", None)
        .await
        .unwrap();
    let said: Vec<String> = texts(&e, g).into_iter().map(|(_, t)| t).collect();
    assert!(
        said.contains(&"Ann, hi to you. The steel is in the hold.".to_owned()),
        "{said:?}"
    );
    assert!(
        said.contains(&"Bo: and the hold is locked. I agree.".to_owned()),
        "{said:?}"
    );
}

/// Every tool action the sink saw, with who made it.
fn tool_actions(e: &Env) -> Vec<(api::types::Author, api::views::ToolAction)> {
    e.sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            agent::Event::ToolCalled(t) => Some((t.author, t.action)),
            _ => None,
        })
        .collect()
}

fn group_count(e: &Env) -> i64 {
    e.agent.with_world(|w| {
        w.query_row(
            "SELECT count(*) FROM channel WHERE kind = 'group'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
}

#[tokio::test]
async fn a_persons_suggestion_is_an_offer_and_changes_nothing() {
    use api::views::ToolAction;
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([
        call("ask_host", json!({"find": "forge blade steel"})),
        say("Bo knows more about that than I do."),
    ]);
    e.agent.send(ch, "who forges blades?", None).await.unwrap();
    let seen = tool_actions(&e);
    assert!(
        seen.iter()
            .any(|(who, a)| matches!(who, api::types::Author::Person { .. })
                && matches!(a, ToolAction::FindPeople { persons, .. }
                if persons.iter().any(|x| x.id.0 == "bo"))),
        "{seen:?}"
    );
    // The offer is stored with the person's message; nothing was opened or joined.
    let stored = e.agent.history(ch, None, 5).unwrap();
    assert!(stored.last().unwrap().actions.len() == 1);
    assert_eq!(group_count(&e), 0);
    let here = e.agent.with_world(|w| channel::get(w, ch).unwrap());
    assert!(!here.has(&Actor::Person(p("bo"))));
    assert!(log_bytes(&e.agent, ch).contains("nothing has happened yet"));

    // A group: the offer names the person too and no group exists until it is accepted.
    e.fake.push([
        call("ask_host", json!({"group": ["bo", "dee"]})),
        say("A group would help."),
    ]);
    e.agent
        .send(ch, "can we get everyone?", None)
        .await
        .unwrap();
    let seen = tool_actions(&e);
    let members = seen.iter().find_map(|(_, a)| match a {
        ToolAction::CreateGroup {
            members,
            channel: None,
            ..
        } => Some(members.iter().map(|m| m.id.0.clone()).collect::<Vec<_>>()),
        _ => None,
    });
    assert_eq!(members.unwrap(), ["ann", "bo", "dee"]);
    assert_eq!(group_count(&e), 0);
    // Accepting is the user's own create_group, which makes it and opens nothing else.
    let g = e
        .agent
        .create_group(&[p("ann"), p("bo"), p("dee")], None)
        .unwrap();
    assert_eq!(group_count(&e), 1);
    assert!(
        e.agent
            .with_world(|w| channel::get(w, g).unwrap())
            .has(&Actor::Person(p("dee")))
    );
}

#[tokio::test]
async fn a_suggestion_is_limited_to_the_roster_the_quota_and_once_per_reply() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    // Once per reply; a stranger, a person without a persona and a disabled person are refused.
    e.agent
        .with_world(|w| bond::set_mode(w, &p("dee"), Mode::Disabled).unwrap());
    e.fake.push([
        call("ask_host", json!({"group": ["bo"]})),
        call("ask_host", json!({"find": "forge blade steel"})),
        call("ask_host", json!({"group": ["nobody"]})),
        say("Done."),
    ]);
    e.agent.send(ch, "hello", None).await.unwrap();
    let log = log_bytes(&e.agent, ch);
    assert!(
        log.contains("One suggestion is enough for a reply."),
        "{log}"
    );
    assert_eq!(tool_actions(&e).len(), 1);

    e.fake.push([
        call("ask_host", json!({"group": ["cy"]})),
        call("ask_host", json!({"group": ["dee"]})),
        say("No."),
    ]);
    e.agent.send(ch, "again", None).await.unwrap();
    assert!(
        tool_actions(&e).is_empty(),
        "no persona and disabled are never offered"
    );
    assert!(log_bytes(&e.agent, ch).contains("Nobody suitable is available."));

    // Neither or both of the arguments is no suggestion.
    e.fake.push([
        call("ask_host", json!({})),
        call("ask_host", json!({"find": "x", "group": ["bo"]})),
        say("Hm."),
    ]);
    e.agent.send(ch, "third", None).await.unwrap();
    assert!(tool_actions(&e).is_empty());

    // The quiet band closes unprompted offers like unprompted speech.
    let q = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.world.quota.quiet_at = 0.0;
    });
    let ch = direct(&q, "ann");
    q.fake.push([
        call("ask_host", json!({"group": ["bo"]})),
        say("Never mind."),
    ]);
    q.agent.send(ch, "hello", None).await.unwrap();
    assert!(tool_actions(&q).is_empty());
}

// ---- emulated tools: a person proposes through a closing line of the reply ----

fn group_topic(e: &Env, g: ChannelId) -> Option<String> {
    e.agent.with_world(|w| channel::get(w, g).unwrap().topic)
}

#[tokio::test]
async fn a_proposal_carries_a_short_purpose_on_both_paths() {
    use api::views::ToolAction;
    let purpose_of = |actions: &[ToolAction]| {
        actions.iter().find_map(|a| match a {
            ToolAction::CreateGroup { purpose, .. } => Some(purpose.clone()),
            _ => None,
        })
    };
    // Native tools: cut to one short line.
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    let long = format!("plan a party\nsecond line {}", "x".repeat(80));
    e.fake.push([
        call("ask_host", json!({"group": ["bo", "dee"], "purpose": long})),
        say("A group would help."),
    ]);
    e.agent.send(ch, "everyone?", None).await.unwrap();
    let (_, actions) = turn_events(&e);
    assert_eq!(purpose_of(&actions), Some(Some("plan a party".to_owned())));
    // Without one, the offer has none.
    e.fake.push([
        call("ask_host", json!({"group": ["bo", "dee"]})),
        say("A group would help."),
    ]);
    e.agent.send(ch, "everyone?", None).await.unwrap();
    let (_, actions) = turn_events(&e);
    assert_eq!(purpose_of(&actions), Some(None));

    // Emulated tools: after a bar, ASCII or full-width.
    let (e, ch) = emulated_env();
    for bar in ["|", "\u{ff5c}"] {
        e.fake.push([say(&format!(
            "A group would help.\n{GROUP} Bo, dee {bar} plan a party"
        ))]);
        e.agent.send(ch, "everyone?", None).await.unwrap();
        let (streamed, actions) = turn_events(&e);
        assert!(!streamed.contains('\u{21d2}') && !streamed.contains("plan a party"));
        assert_eq!(purpose_of(&actions), Some(Some("plan a party".to_owned())));
    }
    // A line without a purpose still works.
    e.fake
        .push([say(&format!("A group would help.\n{GROUP} Bo, dee"))]);
    e.agent.send(ch, "everyone?", None).await.unwrap();
    assert_eq!(purpose_of(&turn_events(&e).1), Some(None));
    // The model is told how to write it.
    let body = String::from_utf8(e.fake.bodies()[0].clone()).unwrap();
    assert!(body.contains("the purpose of"), "{body}");
}

#[tokio::test]
async fn a_group_made_for_a_purpose_says_why_and_tells_every_member() {
    let e = env(WireApi::Chat);
    // As the user accepts a person's proposal: the topic is the purpose, and the host's
    // first line names it and who suggested it. No model call is made.
    let g = e
        .agent
        .create_group(&[p("ann"), p("bo")], Some("plan a party"))
        .unwrap();
    e.agent
        .group_opening(g, Some(&p("ann")), "plan a party")
        .unwrap();
    assert_eq!(group_topic(&e, g).as_deref(), Some("plan a party"));
    assert!(e.fake.bodies().is_empty());
    let first = e.agent.history(g, None, 5).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].author, api::types::Author::Host);
    assert!(
        first[0].text.contains("suggested this group: plan a party")
            && first[0].text.contains("ann"),
        "{}",
        first[0].text
    );
    // Every member's prompt carries the purpose as the topic.
    e.fake.push([say("Bo: gladly.")]);
    e.agent.send(g, "bo, shall we start?", None).await.unwrap();
    let body = String::from_utf8(e.fake.bodies()[0].clone()).unwrap();
    assert!(body.contains("Topic: plan a party"), "{body}");

    // The host's own create_group takes the same argument (and the old name).
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        call(
            "create_group",
            json!({"members": ["ann", "dee"], "purpose": "plan a party"}),
        ),
        say("Done."),
    ]);
    e.agent.send(host, "make a group", None).await.unwrap();
    let made = e.agent.with_world(|w| {
        channel::list(w)
            .unwrap()
            .into_iter()
            .find(|c| c.kind == world::ChannelKind::Group && c.id != g)
            .unwrap()
    });
    assert_eq!(made.topic.as_deref(), Some("plan a party"));
    let line = e.agent.history(made.id, None, 5).unwrap();
    assert!(
        line[0].author == api::types::Author::Host
            && line[0].text.contains("suggested this group: plan a party"),
        "{line:?}"
    );
}

#[tokio::test]
async fn a_found_person_is_told_why_when_the_user_takes_them() {
    let e = env(WireApi::Chat);
    let bo = direct(&e, "bo");
    e.agent
        .found_opening(bo, Some(&p("ann")), "forge blade steel")
        .await
        .unwrap();
    // No model call; the line is in the person's log and shown as a service line.
    assert!(e.fake.bodies().is_empty());
    let shown = e.agent.history(bo, None, 5).unwrap();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].service, "{shown:?}");
    assert!(
        shown[0].text.contains("suggested you could help")
            && shown[0].text.contains("forge blade steel")
            && shown[0].text.contains("ann"),
        "{}",
        shown[0].text
    );
    assert!(log_bytes(&e.agent, bo).contains("forge blade steel"));
    // Taking the same person again does not repeat it.
    e.agent
        .found_opening(bo, Some(&p("ann")), "forge blade steel")
        .await
        .unwrap();
    assert_eq!(e.agent.history(bo, None, 5).unwrap().len(), 1);
    // The host's own find offers say it was the host.
    let cy = direct(&e, "dee");
    e.agent.found_opening(cy, None, "tea").await.unwrap();
    assert!(
        e.agent.history(cy, None, 5).unwrap()[0]
            .text
            .contains("Host")
    );
    // The person answers when the user writes, and the reason is in what the model sees.
    e.fake.push([say("Steel, then.")]);
    e.agent.send(bo, "tell me about steel", None).await.unwrap();
    let body = String::from_utf8(e.fake.bodies()[0].clone()).unwrap();
    assert!(body.contains("suggested you could help"), "{body}");
}

const FIND: &str = "\u{21d2} find:";
const GROUP: &str = "\u{21d2} group:";

/// The events of a turn: streamed text, and the tool actions shown.
fn turn_events(e: &Env) -> (String, Vec<api::views::ToolAction>) {
    let mut streamed = String::new();
    let mut actions = Vec::new();
    for ev in e.sink.take() {
        match ev {
            agent::Event::MessageDelta(d) => streamed.push_str(&d.delta),
            agent::Event::ToolCalled(t) => actions.push(t.action),
            _ => {}
        }
    }
    (streamed, actions)
}

#[tokio::test]
async fn an_emulated_person_proposes_with_a_closing_line() {
    use api::views::ToolAction;
    let (e, ch) = emulated_env();
    e.fake.push([say(&format!(
        "Bo knows more than I do.\n{FIND} forge blade steel"
    ))]);
    e.agent.send(ch, "find someone for me", None).await.unwrap();
    let (streamed, actions) = turn_events(&e);
    // The line is neither streamed nor stored, and the offer waits under the message.
    assert!(
        !streamed.contains('\u{21d2}') && streamed.contains("Bo knows"),
        "{streamed}"
    );
    assert!(
        matches!(actions.as_slice(), [ToolAction::FindPeople { persons, .. }]
            if persons.iter().any(|x| x.id.0 == "bo")),
        "{actions:?}"
    );
    let stored = e.agent.history(ch, None, 5).unwrap();
    let last = stored.last().unwrap();
    assert_eq!(last.text, "Bo knows more than I do.");
    assert_eq!(last.actions.len(), 1);
    assert_eq!(group_count(&e), 0);
    // No tool list is sent; the protocol is explained instead.
    let body = String::from_utf8(e.fake.bodies()[0].clone()).unwrap();
    assert!(body.contains(FIND) && body.contains(GROUP), "{body}");
    assert!(body.contains("nothing happens until"));
}

#[tokio::test]
async fn an_emulated_group_proposal_matches_the_tool_path_and_makes_nothing() {
    use api::views::ToolAction;
    let (e, ch) = emulated_env();
    e.fake
        .push([say(&format!("A group would help.\n{GROUP} Bo, dee"))]);
    e.agent
        .send(ch, "can we get everyone?", None)
        .await
        .unwrap();
    let (streamed, text_path) = turn_events(&e);
    assert!(!streamed.contains('\u{21d2}'), "{streamed}");
    assert_eq!(group_count(&e), 0);
    let [
        ToolAction::CreateGroup {
            members,
            channel: None,
            ..
        },
    ] = text_path.as_slice()
    else {
        panic!("{text_path:?}");
    };
    assert_eq!(
        members.iter().map(|m| m.id.0.as_str()).collect::<Vec<_>>(),
        ["ann", "bo", "dee"]
    );
    // The same offer as the tool call makes, so accepting it takes the one-shot path.
    let n = env(WireApi::Chat);
    let nch = direct(&n, "ann");
    n.fake.push([
        call("ask_host", json!({"group": ["bo", "dee"]})),
        say("A group would help."),
    ]);
    n.agent
        .send(nch, "can we get everyone?", None)
        .await
        .unwrap();
    let (_, tool_path) = turn_events(&n);
    assert_eq!(text_path, tool_path);
    let stored = e.agent.history(ch, None, 5).unwrap();
    assert_eq!(stored.last().unwrap().actions, text_path);
}

#[tokio::test]
async fn an_emulated_proposal_keeps_the_same_limits_as_the_tool() {
    let (e, ch) = emulated_env();
    e.agent
        .with_world(|w| bond::set_mode(w, &p("dee"), Mode::Disabled).unwrap());
    // One per reply: the second line is refused.
    e.fake
        .push([say(&format!("Ok.\n{FIND} forge blade steel\n{GROUP} Bo"))]);
    e.agent.send(ch, "hello", None).await.unwrap();
    let (_, actions) = turn_events(&e);
    assert_eq!(actions.len(), 1, "{actions:?}");
    // A stranger, a person without a persona and a disabled person are never offered.
    for who in ["nobody", "cy", "dee"] {
        e.fake.push([say(&format!("No.\n{GROUP} {who}"))]);
        e.agent.send(ch, "again", None).await.unwrap();
        let (streamed, actions) = turn_events(&e);
        assert!(actions.is_empty(), "{who}: {actions:?}");
        assert!(!streamed.contains('\u{21d2}'));
    }
    let stored = e.agent.history(ch, None, 9).unwrap();
    assert!(stored.last().unwrap().text == "No.");
    // The quiet band closes unprompted offers.
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let q = env_with(ep, |c| c.world.quota.quiet_at = 0.0);
    let qch = direct(&q, "ann");
    q.fake.push([say(&format!("Never mind.\n{GROUP} Bo"))]);
    q.agent.send(qch, "hello", None).await.unwrap();
    assert!(turn_events(&q).1.is_empty());
}

#[tokio::test]
async fn a_malformed_proposal_line_is_speech_and_native_tools_ignore_the_mark() {
    let (e, ch) = emulated_env();
    let reply = format!("Mind the {FIND} mark.\n{FIND}");
    e.fake.push([say(&reply)]);
    e.agent.send(ch, "hello", None).await.unwrap();
    let (streamed, actions) = turn_events(&e);
    assert!(actions.is_empty());
    assert_eq!(streamed, reply);
    assert_eq!(
        e.agent.history(ch, None, 5).unwrap().last().unwrap().text,
        reply
    );
    // A line in the middle of the text is speech too.
    let mid = format!("{FIND} steel\nand more");
    e.fake.push([say(&mid)]);
    e.agent.send(ch, "again", None).await.unwrap();
    let (streamed, actions) = turn_events(&e);
    assert!(actions.is_empty() && streamed == mid, "{streamed}");
    // Native tools: the model has the tool, the line means nothing and the prompt is silent.
    let n = env(WireApi::Chat);
    let nch = direct(&n, "ann");
    let line = format!("Hm.\n{FIND} steel");
    n.fake.push([say(&line)]);
    n.agent.send(nch, "hello", None).await.unwrap();
    let (streamed, actions) = turn_events(&n);
    assert!(actions.is_empty() && streamed == line);
    let body = String::from_utf8(n.fake.bodies()[0].clone()).unwrap();
    assert!(!body.contains("nothing happens until"));
}

#[tokio::test]
async fn the_host_keeps_its_tools_on_an_emulated_endpoint_and_reads_a_fenced_call() {
    use api::views::ToolAction;
    let (e, _) = emulated_env();
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    let body = json!({"name": "find_people", "arguments": {"topic": "forge blade steel"}});
    e.fake.push([
        say(&format!("```tool_call\n{body}\n```")),
        say("Bo knows it."),
    ]);
    e.agent.send(host, "find me a smith", None).await.unwrap();
    // The request carries the host's tools (the endpoint emulates them or the text block
    // is read), unlike a person's.
    assert!(e.fake.json()[0].get("tools").is_some());
    let (_, actions) = turn_events(&e);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, ToolAction::FindPeople { persons, .. } if !persons.is_empty())),
        "{actions:?}"
    );
}

#[tokio::test]
async fn a_new_save_opens_with_the_hosts_welcome_once_and_without_a_model_call() {
    for (frontend, marker) in [
        (agent::Frontend::Terminal, "/talk"),
        (agent::Frontend::App, "Roster"),
    ] {
        // No endpoint at all: the welcome is the same.
        let e = env_with(api::endpoints::Endpoints::default(), |c| {
            c.frontend = frontend
        });
        let host = e.agent.with_world(|w| channel::host(w).unwrap());
        assert!(e.agent.presence(true).await.unwrap().host_spoke);
        let said = e.agent.history(host, None, 5).unwrap();
        assert_eq!(said.len(), 1);
        assert!(said[0].text.contains(marker), "{}", said[0].text);
        assert!(e.fake.bodies().is_empty());
        // Opening again says nothing more, and the host's own session can tell it again.
        assert!(!e.agent.presence(true).await.unwrap().host_spoke);
        assert_eq!(e.agent.history(host, None, 5).unwrap().len(), 1);
    }
    let e = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.frontend = agent::Frontend::Terminal
    });
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.agent.presence(true).await.unwrap();
    e.fake.push([say("It works like this.")]);
    e.agent
        .send(host, "how does this work?", None)
        .await
        .unwrap();
    let first = body_text(&e.fake.bodies()[0]);
    assert!(first.contains("I am Hostess. Try /talk"), "{first}");
    assert!(
        first.contains("You assist Doc."),
        "the host's manner is in its own session"
    );
}

#[tokio::test]
async fn pre_written_host_lines_have_no_unfilled_placeholder() {
    let e = env(WireApi::Responses);
    assert!(e.agent.presence(true).await.unwrap().host_spoke);
    let lines: Vec<String> = e.agent.with_world(|w| {
        let host = channel::host(w).unwrap();
        world::message::history(w, host, None, 50)
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect()
    });
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(line.contains("Hostess"), "{line}");
        assert!(!line.contains('{') && !line.contains('}'), "{line}");
    }
}

#[tokio::test]
async fn one_named_person_is_a_talk_offer_never_a_group() {
    use agent::Event;
    use api::views::ToolAction;
    let e = env(WireApi::Chat);
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    e.fake.push([
        call("create_group", json!({"members": ["ann", "ann"]})),
        call("find_people", json!({"topic": "ANN"})),
        say("Ann can be reached from the offer."),
    ]);
    e.agent.send(host, "find ann", None).await.unwrap();
    let actions: Vec<ToolAction> = e
        .sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            Event::ToolCalled(t) => Some(t.action),
            _ => None,
        })
        .collect();
    let offers = actions
        .iter()
        .filter(|a| {
            matches!(a, ToolAction::FindPeople { topic, persons }
                if topic.is_empty() && persons.len() == 1 && persons[0].id.0 == "ann")
        })
        .count();
    assert_eq!(offers, 2, "{actions:?}");
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, ToolAction::CreateGroup { .. })),
        "{actions:?}"
    );
    let groups: i64 = e.agent.with_world(|w| {
        w.query_row(
            "SELECT COUNT(*) FROM channel WHERE kind = 'group'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(groups, 0);
    let said = log_bytes(&e.agent, host);
    assert!(said.contains("No group exists"), "{said}");
}
