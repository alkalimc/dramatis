//! Deleting one message and writing the latest reply again.

mod common;

use std::time::Duration;

use api::endpoints::WireApi;
use common::*;
use serde_json::json;
use world::{ChannelId, MessageId, settings};

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

fn id_of(e: &Env, ch: ChannelId, text: &str) -> MessageId {
    let m = e
        .agent
        .history(ch, None, 50)
        .unwrap()
        .into_iter()
        .find(|m| m.text == text)
        .unwrap_or_else(|| panic!("no message {text}"));
    MessageId(m.id.0.parse().unwrap())
}

fn texts(e: &Env, ch: ChannelId) -> Vec<String> {
    e.agent
        .history(ch, None, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect()
}

fn removed(e: &Env) -> Vec<String> {
    e.sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            agent::Event::MessageRemoved(r) => Some(r.message.0),
            _ => None,
        })
        .collect()
}

fn last_request(e: &Env) -> String {
    String::from_utf8(e.fake.bodies().last().unwrap().clone()).unwrap()
}

#[tokio::test]
async fn a_deleted_reply_is_gone_from_the_chat_and_from_what_the_model_sees_next() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([
        say("An unfortunate first line."),
        say("A good second line."),
        say("A third."),
    ]);
    e.agent.send(ch, "first words", None).await.unwrap();
    e.clock.advance(60_000);
    e.agent.send(ch, "second words", None).await.unwrap();
    let bad = id_of(&e, ch, "An unfortunate first line.");
    e.sink.take();

    e.agent.delete_message(ch, bad).unwrap();
    assert_eq!(removed(&e), [bad.0.to_string()]);
    assert_eq!(
        texts(&e, ch),
        ["first words", "second words", "A good second line."]
    );
    let log = log_bytes(&e.agent, ch);
    assert!(!log.contains("unfortunate"), "{log}");
    assert!(log.contains("A good second line.") && log.contains("first words"));

    e.clock.advance(60_000);
    e.agent.send(ch, "third words", None).await.unwrap();
    let sent = last_request(&e);
    assert!(!sent.contains("unfortunate"));
    assert!(sent.contains("A good second line.") && sent.contains("third words"));
}

#[tokio::test]
async fn a_deleted_user_message_leaves_its_replies_but_not_its_words() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("Reply one."), say("Reply two.")]);
    e.agent.send(ch, "embarrassing words", None).await.unwrap();
    e.clock.advance(60_000);
    e.agent.send(ch, "fine words", None).await.unwrap();
    let mine = id_of(&e, ch, "embarrassing words");

    e.agent.delete_message(ch, mine).unwrap();
    assert_eq!(texts(&e, ch), ["Reply one.", "fine words", "Reply two."]);
    assert!(!log_bytes(&e.agent, ch).contains("embarrassing"));
    e.fake.push([say("Reply three.")]);
    e.clock.advance(60_000);
    e.agent.send(ch, "more words", None).await.unwrap();
    assert!(!last_request(&e).contains("embarrassing"));
    // A message of no channel, and one of another channel, are refused.
    assert!(e.agent.delete_message(ch, MessageId(99_999)).is_err());
    let other = direct(&e, "bo");
    assert!(e.agent.delete_message(other, mine).is_err());
}

#[tokio::test]
async fn a_message_in_the_opening_block_of_a_later_segment_leaves_that_block() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("The harbor was quiet tonight.")]);
    e.agent.send(ch, "how was the harbor?", None).await.unwrap();
    e.agent
        .with_world(|w| settings::set_user_name(w, Some("Rhea")).unwrap());
    e.fake
        .push([call("wrapup", json!({"facts": [], "summary": "harbor"}))]);
    e.agent.on_rename().await.unwrap();
    let segs = segments(&e.agent, ch);
    assert_eq!(segs.len(), 2);
    let block = |s: &world::log::Segment| String::from_utf8(s.prefix_c.clone()).unwrap();
    assert!(block(&segs[1]).contains("The harbor was quiet tonight."));

    let said = id_of(&e, ch, "The harbor was quiet tonight.");
    e.agent.delete_message(ch, said).unwrap();
    let segs = segments(&e.agent, ch);
    assert!(
        !block(&segs[1]).contains("quiet tonight"),
        "{}",
        block(&segs[1])
    );
    assert!(block(&segs[1]).contains("how was the harbor?"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_deleted_while_a_reply_is_being_written() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("Early.")]);
    e.agent.send(ch, "first", None).await.unwrap();
    let early = id_of(&e, ch, "Early.");
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake.push([say("Late.")]);
    let posted = e.agent.post(ch, "second", None).unwrap();
    let agent = e.agent.clone();
    let running = tokio::spawn(async move { agent.respond(posted).await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while hold.seen() < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        e.agent.delete_message(ch, early),
        Err(agent::Error::Busy)
    ));
    hold.release();
    running.await.unwrap().unwrap();
    assert!(e.agent.delete_message(ch, early).is_ok());
}

#[tokio::test]
async fn the_latest_reply_is_replaced_by_a_new_one_and_the_old_one_leaves_the_model() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("A poor reply."), say("A better reply.")]);
    e.agent.send(ch, "hello there", None).await.unwrap();
    let old = id_of(&e, ch, "A poor reply.");
    e.sink.take();

    e.agent.regenerate(ch, old).await.unwrap();
    assert_eq!(texts(&e, ch), ["hello there", "A better reply."]);
    assert!(removed(&e).contains(&old.0.to_string()));
    let log = log_bytes(&e.agent, ch);
    assert!(!log.contains("poor") && log.contains("A better reply."));
    // The second request asked the same question, without the poor reply.
    let sent = last_request(&e);
    assert!(sent.contains("hello there") && !sent.contains("poor"));
    assert_eq!(e.fake.bodies().len(), 2);
    // Only the latest reply, and nothing of the user's, can be written again.
    assert!(e.agent.regenerate(ch, MessageId(99_999)).await.is_err());
    let mine = id_of(&e, ch, "hello there");
    assert!(e.agent.regenerate(ch, mine).await.is_err());
}

#[tokio::test]
async fn a_failed_try_brings_the_old_reply_back() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake
        .push([say("The first reply."), Reply::Fail("boom".into())]);
    e.agent.send(ch, "hello there", None).await.unwrap();
    let old = id_of(&e, ch, "The first reply.");
    assert!(e.agent.regenerate(ch, old).await.is_err());
    assert_eq!(texts(&e, ch), ["hello there", "The first reply."]);
    assert!(log_bytes(&e.agent, ch).contains("The first reply."));
    // And it can be tried again.
    e.fake.push([say("The second reply.")]);
    let back = id_of(&e, ch, "The first reply.");
    e.agent.regenerate(ch, back).await.unwrap();
    assert_eq!(texts(&e, ch), ["hello there", "The second reply."]);
}
