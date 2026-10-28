//! Taking a message back: the model log returns to where it was, and the same text can
//! be sent again.

mod common;

use std::time::Duration;

use api::endpoints::WireApi;
use common::*;
use world::ChannelId;

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

fn texts(e: &Env, ch: ChannelId) -> Vec<String> {
    e.agent
        .history(ch, None, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect()
}

#[tokio::test]
async fn a_rewound_turn_leaves_no_trace_in_the_log_and_the_text_can_go_again() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake
        .push([say("ann one"), say("ann two"), say("ann again")]);
    e.agent.send(ch, "first words", None).await.unwrap();
    e.clock.advance(60_000);
    let second = e.agent.send(ch, "second words", None).await.unwrap();
    e.clock.advance(60_000);

    let text = e.agent.rewind(ch, second).unwrap();
    assert_eq!(text, "second words");
    assert_eq!(texts(&e, ch), ["first words", "ann one"]);
    let log = log_bytes(&e.agent, ch);
    assert!(log.contains("first words") && log.contains("ann one"));
    assert!(!log.contains("second words") && !log.contains("ann two"));
    let events = e.sink.take();
    assert!(events.iter().any(
        |ev| matches!(ev, agent::Event::Rewound(r) if r.from == agent::view::message_id(second))
    ));

    e.agent.send(ch, &text, None).await.unwrap();
    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 3);
    // The resent turn continues the first turn's bytes, as if the second never happened.
    assert_prefix_chain(&[bodies[0].clone(), bodies[2].clone()]);
    assert!(!String::from_utf8_lossy(&bodies[2]).contains("ann two"));
}

#[tokio::test]
async fn only_the_users_own_message_can_be_taken_back() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("ann one")]);
    e.agent.send(ch, "hello", None).await.unwrap();
    let hist = e.agent.history(ch, None, 10).unwrap();
    let reply = hist.iter().find(|m| m.text == "ann one").unwrap();
    assert!(
        e.agent
            .rewind(ch, world::MessageId(reply.id.0.parse().unwrap()))
            .is_err()
    );
    assert_eq!(texts(&e, ch).len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_turn_blocks_the_rewind() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake.push([say("late")]);
    let posted = e.agent.post(ch, "hello", None).unwrap();
    let id = posted.message;
    let agent = e.agent.clone();
    let running = tokio::spawn(async move { agent.respond(posted).await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while hold.seen() < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(e.agent.rewind(ch, id), Err(agent::Error::Busy)));
    hold.release();
    running.await.unwrap().unwrap();
    assert!(e.agent.rewind(ch, id).is_ok());
}

/// On the messages wire: every tool_use is answered by the very next turn, and every
/// tool_result answers a tool_use of the turn before it.
fn assert_calls_paired(body: &[u8]) {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap();
    let turns = list(&v);
    let ids = |turn: &serde_json::Value, kind: &str, key: &str| -> Vec<String> {
        turn["content"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter(|p| p["type"] == kind)
                    .map(|p| p[key].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    for (i, turn) in turns.iter().enumerate() {
        let mut calls = ids(turn, "tool_use", "id");
        let mut answers = turns
            .get(i + 1)
            .map(|n| ids(n, "tool_result", "tool_use_id"))
            .unwrap_or_default();
        calls.sort();
        answers.sort();
        assert_eq!(calls, answers, "calls of turn {i} are not answered exactly");
        if i == 0 {
            assert!(ids(turn, "tool_result", "tool_use_id").is_empty());
        }
    }
}

/// A turn whose tool failed leaves the error outputs in the log. Taking back either
/// that turn or a later one still gives a request with no orphan call or output.
#[tokio::test]
async fn rewinding_around_a_failed_tool_turn_keeps_calls_and_outputs_paired() {
    let e = env(WireApi::Messages);
    let ch = direct(&e, "ann");
    e.fake.push([say("ann one")]);
    e.agent.send(ch, "first", None).await.unwrap();
    e.clock.advance(60_000);

    e.agent.with_world(|w| {
        w.execute_batch(
            "CREATE TRIGGER no_facts BEFORE INSERT ON fact
             BEGIN SELECT RAISE(ABORT, 'disk'); END",
        )
        .unwrap();
    });
    e.fake
        .push([call("remember", serde_json::json!({"text": "likes tea"}))]);
    let posted = e.agent.post(ch, "note this", None).unwrap();
    let failed = posted.message;
    e.agent.respond(posted).await.unwrap_err();
    e.agent
        .with_world(|w| w.execute_batch("DROP TRIGGER no_facts").unwrap());
    e.clock.advance(60_000);

    e.fake.push([say("ann two")]);
    let third = e.agent.send(ch, "third", None).await.unwrap();
    e.clock.advance(60_000);

    // Taking back the later turn keeps the failed one, with its error output, in the log.
    e.agent.rewind(ch, third).unwrap();
    e.fake.push([say("ann three")]);
    e.agent.send(ch, "third", None).await.unwrap();
    let bodies = e.fake.bodies();
    let last = bodies.last().unwrap();
    assert!(String::from_utf8_lossy(last).contains("The tool did not finish."));
    assert_calls_paired(last);
    e.clock.advance(60_000);

    // Taking back the failed turn removes its call and its output together.
    e.agent.rewind(ch, failed).unwrap();
    e.fake.push([say("ann four")]);
    e.agent.send(ch, "again", None).await.unwrap();
    let bodies = e.fake.bodies();
    let last = bodies.last().unwrap();
    let sent = String::from_utf8_lossy(last);
    assert!(!sent.contains("tool_use") && !sent.contains("tool_result"));
    assert!(!sent.contains("The tool did not finish."));
    assert_calls_paired(last);
    assert_prefix_chain(&[bodies[0].clone(), last.clone()]);
}
