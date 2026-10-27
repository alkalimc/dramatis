//! Turns in different channels overlap; turns in one channel keep their order.

mod common;

use std::time::Duration;

use api::endpoints::WireApi;
use common::*;
use world::{ChannelId, channel, session};

/// A hung overlap would otherwise hang the suite: fail instead.
async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), f)
        .await
        .expect("the flows did not finish: they are not running side by side")
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_direct_channels_run_at_once() {
    let e = env(WireApi::Chat);
    let (ann, bo) = (direct(&e, "ann"), direct(&e, "bo"));
    // Neither call returns until both have reached the endpoint.
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake.push_for("MARK-ANN", [say("ann answers")]);
    e.fake.push_for("MARK-BO", [say("bo answers")]);
    let (a, b) = within(async {
        tokio::join!(
            e.agent.send(ann, "MARK-ANN hello", None),
            e.agent.send(bo, "MARK-BO hello", None)
        )
    })
    .await;
    a.unwrap();
    b.unwrap();
    assert_eq!(hold.seen(), 2);
    assert!(texts(&e, ann).contains(&"ann answers".to_owned()));
    assert!(texts(&e, bo).contains(&"bo answers".to_owned()));
    // Each log holds only its own channel's words.
    let (la, lb) = (log_bytes(&e.agent, ann), log_bytes(&e.agent, bo));
    assert!(la.contains("MARK-ANN") && !la.contains("MARK-BO"));
    assert!(lb.contains("MARK-BO") && !lb.contains("MARK-ANN"));
    // Both calls were metered, once each.
    let n: i64 = e.agent.with_world(|w| {
        w.query_row("SELECT count(*) FROM meter", [], |r| r.get(0))
            .unwrap()
    });
    assert_eq!(n, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_posts_to_one_channel_are_answered_in_order() {
    let e = env(WireApi::Chat);
    let ann = direct(&e, "ann");
    e.fake.push([say("first answer"), say("second answer")]);
    let one = e.agent.post(ann, "first question", None).unwrap();
    let two = e.agent.post(ann, "second question", None).unwrap();
    // Polled in this order, the first takes the channel and the second queues behind it.
    let (a, b) = within(async { tokio::join!(e.agent.respond(one), e.agent.respond(two)) }).await;
    a.unwrap();
    b.unwrap();
    let all = texts(&e, ann);
    let at = |t: &str| all.iter().position(|x| x == t).unwrap();
    assert!(at("first answer") < at("second answer"), "{all:?}");
    // The second call saw the first reply: the turns did not overlap.
    let bodies = e.fake.bodies();
    assert_eq!(bodies.len(), 2);
    assert!(String::from_utf8_lossy(&bodies[1]).contains("first answer"));
    assert_prefix_chain(&bodies);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_turn_proceeds_while_a_person_turn_is_in_flight() {
    let e = env_with(endpoints_with_host(WireApi::Chat), |_| {});
    let ann = direct(&e, "ann");
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake.push_for("MARK-ANN", [say("ann answers")]);
    e.fake.push_for("MARK-HOST", [say("host answers")]);
    let (a, h) = within(async {
        tokio::join!(
            e.agent.send(ann, "MARK-ANN hello", None),
            e.agent.send(host, "MARK-HOST hello", None)
        )
    })
    .await;
    a.unwrap();
    h.unwrap();
    assert!(texts(&e, host).contains(&"host answers".to_owned()));
    assert!(texts(&e, ann).contains(&"ann answers".to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_turn_waits_for_its_channel_but_not_for_another() {
    let e = env(WireApi::Chat);
    let (ann, bo) = (direct(&e, "ann"), direct(&e, "bo"));
    // Ann's first call stays in the endpoint until a second call arrives.
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake
        .push_for("MARK-ANN", [say("ann one"), say("ann two")]);
    e.fake.push_for("MARK-BO", [say("bo one")]);
    let first = e.agent.post(ann, "MARK-ANN one", None).unwrap();
    let agent = e.agent.clone();
    let running = tokio::spawn(async move { agent.respond(first).await });
    within(async {
        while hold.seen() < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    let second = e.agent.post(ann, "MARK-ANN two", None).unwrap();
    let agent = e.agent.clone();
    let queued = tokio::spawn(async move { agent.respond(second).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        hold.seen(),
        1,
        "the queued turn must not call the model yet"
    );
    // Bo's call is the second to arrive: it frees ann's first call.
    within(e.agent.send(bo, "MARK-BO one", None)).await.unwrap();
    within(async {
        running.await.unwrap().unwrap();
        queued.await.unwrap().unwrap();
    })
    .await;
    assert_eq!(hold.seen(), 3);
    let all = texts(&e, ann);
    let at = |t: &str| all.iter().position(|x| x == t).unwrap();
    assert!(at("ann one") < at("ann two"), "{all:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tick_does_not_end_the_conversation_of_a_turn_in_flight() {
    let e = env(WireApi::Chat);
    let ann = direct(&e, "ann");
    let hold = Hold::new(usize::MAX);
    e.fake.hold(&hold);
    e.fake.push([say("late answer")]);
    let posted = e.agent.post(ann, "hello", None).unwrap();
    let agent = e.agent.clone();
    let running = tokio::spawn(async move { agent.respond(posted).await });
    within(async {
        while hold.seen() < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    // The model is slow: far longer than the idle timeout passes.
    e.clock.advance(31 * 60_000);
    within(e.agent.tick()).await.unwrap();
    let open = e
        .agent
        .with_world(|w| session::open_sessions(w).unwrap())
        .iter()
        .any(|s| s.channel == ann);
    assert!(open, "the session of a running turn was closed by the tick");
    hold.release();
    within(running).await.unwrap().unwrap();
    // Only the turn's own call was made: no wrap-up ran on the live conversation.
    assert_eq!(e.fake.bodies().len(), 1);
}
