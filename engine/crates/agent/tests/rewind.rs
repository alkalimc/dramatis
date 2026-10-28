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
