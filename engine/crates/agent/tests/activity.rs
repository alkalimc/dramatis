//! The activity events: one speaker's turn from the request going out to the reply being
//! complete, over a scripted endpoint.

mod common;

use agent::Event;
use api::endpoints::WireApi;
use api::events::{ActivityFailure, ActivityState, TurnActivity as Activity};
use api::types::{Author, ChannelId};
use common::*;
use serde_json::json;

fn direct(e: &Env, who: &str) -> world::ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

/// The activity states of one channel, in order.
fn states(events: &[Event], ch: world::ChannelId) -> Vec<ActivityState> {
    let id: ChannelId = agent::view::channel_id(ch);
    events
        .iter()
        .filter_map(|ev| match ev {
            Event::TurnActivity(Activity { channel, state, .. }) if *channel == id => {
                Some(state.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_direct_turn_goes_thinking_writing_done() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say("Cooler water.")]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    let events = e.sink.take();
    assert_eq!(
        states(&events, ch),
        [
            ActivityState::Thinking,
            ActivityState::Writing,
            ActivityState::Done
        ]
    );
    let first = events
        .iter()
        .find_map(|ev| match ev {
            Event::TurnActivity(a) => Some(a.author.clone()),
            _ => None,
        })
        .unwrap();
    assert!(matches!(first, Author::Person { .. }));
    // Done comes after the stored message, never before the text.
    let at = |f: fn(&Event) -> bool| events.iter().position(f).unwrap();
    assert!(
        at(|x| matches!(x, Event::MessageAdded(_)))
            < at(|x| matches!(x, Event::TurnActivity(a) if a.state == ActivityState::Done))
    );
}

#[tokio::test]
async fn a_tool_round_shows_the_tool() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([
        call("search", json!({"query": "forge blade"})),
        say("Cooler water."),
    ]);
    e.agent
        .send(ch, "how should I brew tea?", None)
        .await
        .unwrap();
    assert_eq!(
        states(&e.sink.take(), ch),
        [
            ActivityState::Thinking,
            ActivityState::Tool {
                name: "search".into()
            },
            ActivityState::Thinking,
            ActivityState::Writing,
            ActivityState::Done
        ]
    );
}

#[tokio::test]
async fn an_error_ends_in_failed_with_its_kind() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([Reply::Limited(Some(3600))]);
    let posted = e.agent.post(ch, "how should I brew tea?", None).unwrap();
    e.agent.respond(posted).await.unwrap_err();
    assert_eq!(
        states(&e.sink.take(), ch),
        [
            ActivityState::Thinking,
            ActivityState::Failed {
                reason: ActivityFailure::RateLimited
            }
        ]
    );

    let ch = direct(&e, "bo");
    e.fake.push([Reply::Fail("boom".into())]);
    let posted = e.agent.post(ch, "hello", None).unwrap();
    e.agent.respond(posted).await.unwrap_err();
    assert_eq!(
        states(&e.sink.take(), ch).last(),
        Some(&ActivityState::Failed {
            reason: ActivityFailure::Error
        })
    );
}

#[tokio::test]
async fn a_dropped_turn_still_ends() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    // The call never returns until released; dropping the future cancels the turn.
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake.push([say("never seen")]);
    let turn = e.agent.send(ch, "hello", None);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(300), turn).await;
    assert_eq!(
        states(&e.sink.take(), ch),
        [
            ActivityState::Thinking,
            ActivityState::Failed {
                reason: ActivityFailure::Cancelled
            }
        ]
    );
    hold.release();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn channels_keep_their_own_activity() {
    let e = env(WireApi::Chat);
    let (ann, bo) = (direct(&e, "ann"), direct(&e, "bo"));
    let hold = Hold::new(2);
    e.fake.hold(&hold);
    e.fake.push_for("MARK-ANN", [say("ann answers")]);
    e.fake.push_for("MARK-BO", [say("bo answers")]);
    let (a, b) = tokio::join!(
        e.agent.send(ann, "MARK-ANN hello", None),
        e.agent.send(bo, "MARK-BO hello", None)
    );
    a.unwrap();
    b.unwrap();
    let events = e.sink.take();
    let want = [
        ActivityState::Thinking,
        ActivityState::Writing,
        ActivityState::Done,
    ];
    assert_eq!(states(&events, ann), want);
    assert_eq!(states(&events, bo), want);
    // Both were waiting for the endpoint at the same time.
    let thinking: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(
            |(_, ev)| matches!(ev, Event::TurnActivity(a) if a.state == ActivityState::Thinking),
        )
        .map(|(i, _)| i)
        .collect();
    let first_done = events
        .iter()
        .position(|ev| matches!(ev, Event::TurnActivity(a) if a.state == ActivityState::Done))
        .unwrap();
    assert!(thinking.len() == 2 && thinking.iter().all(|&i| i < first_done));
}

#[tokio::test]
async fn a_round_of_replies_reports_each_speaker_in_turn() {
    let e = env(WireApi::Chat);
    let g = e.agent.create_group(&[p("ann"), p("bo")], None).unwrap();
    e.fake.push([say("Ann: hello."), say("Bo: hi.")]);
    e.agent.send(g, "hello everyone", None).await.unwrap();
    let id: ChannelId = agent::view::channel_id(g);
    let seen: Vec<(Author, ActivityState)> = e
        .sink
        .take()
        .iter()
        .filter_map(|ev| match ev {
            Event::TurnActivity(a) if a.channel == id && a.state == ActivityState::Done => {
                Some((a.author.clone(), a.state.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0].0, seen[1].0);
}
