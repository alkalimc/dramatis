//! A turn that ends early never leaves the channel with an unanswered tool call or an
//! empty message row.

mod common;

use api::endpoints::WireApi;
use common::*;
use serde_json::json;
use world::ChannelId;

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

fn mentions(bodies: &[Vec<u8>], i: usize, needle: &str) -> bool {
    String::from_utf8_lossy(&bodies[i]).contains(needle)
}

#[tokio::test]
async fn a_failed_tool_does_not_break_the_next_turn() {
    let e = env(WireApi::Messages);
    let ch = direct(&e, "ann");
    e.agent.with_world(|w| {
        w.execute_batch(
            "CREATE TRIGGER no_facts BEFORE INSERT ON channel_participant
             BEGIN SELECT RAISE(ABORT, 'disk'); END",
        )
        .unwrap();
    });
    e.fake.push([call("request_join", json!({"person": "bo"}))]);
    e.agent.send(ch, "note this", None).await.unwrap_err();
    e.agent
        .with_world(|w| w.execute_batch("DROP TRIGGER no_facts").unwrap());

    e.fake.push([say("Back again.")]);
    e.agent.send(ch, "hello?", None).await.unwrap();
    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 2);
    // The call is answered in the log itself, so the second request is built from it as is.
    assert!(mentions(&bodies, 1, "The tool did not finish."));
    assert!(mentions(&bodies, 1, "tool_result"));
    assert_prefix_chain(&bodies);
}

#[tokio::test]
async fn a_call_left_open_by_a_cancelled_turn_is_answered_in_the_next_request() {
    let e = env(WireApi::Messages);
    let ch = direct(&e, "ann");
    e.fake.push([say("One.")]);
    e.agent.send(ch, "first", None).await.unwrap();
    // What a turn dropped between its reply and the tool's output leaves behind.
    let open = llm::Message::assistant(vec![llm::Part::ToolCall {
        id: "call_cut".into(),
        name: "search".into(),
        args_json: "{}".into(),
    }]);
    e.agent.with_world(|w| {
        let seg = world::log::current(w, ch).unwrap().unwrap().id;
        world::log::append(
            w,
            seg,
            world::log::Role::Assistant,
            &agent::request::to_bytes(&open),
            T0,
        )
        .unwrap();
    });

    e.fake.push([say("Two.")]);
    e.agent.send(ch, "second", None).await.unwrap();
    let json = e.fake.json();
    let sent = serde_json::to_string(&json[1]).unwrap();
    assert!(sent.contains("call_cut") && sent.contains("tool_use_id"));
    assert!(sent.contains("The tool did not finish."));
}

#[tokio::test]
async fn text_shown_before_a_failure_stays_as_the_message() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    let args = json!({"query": "forge blade"}).to_string();
    // Words, then a tool call; the next step fails.
    let first = sse(
        &[
            json!({"choices": [{"index": 0, "delta": {"content": "Let me look."}}]}),
            json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0,
                "id": "call_search", "function": {"name": "search", "arguments": args}}]}}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 5}}),
        ],
        true,
    );
    e.fake.push([Reply::Raw(first), Reply::Fail("boom".into())]);
    e.agent.send(ch, "find it", None).await.unwrap_err();
    let texts: Vec<String> = e
        .agent
        .history(ch, None, 10)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(texts, ["find it", "Let me look."]);
}
