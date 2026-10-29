//! Daily events: made once a day while the user is about, posted by the host with offers,
//! known to the people they involve for that day only.

mod common;

use agent::Event;
use api::endpoints::WireApi;
use api::views::ToolAction;
use common::*;
use serde_json::{Value, json};
use world::clock::{DAY_MS, HOUR_MS, MINUTE_MS};
use world::{Actor, ChannelId, channel, event, fact, message, settings};

/// What only the event request says.
const ASK: &str = "Make up one small happening";
const TEA: &str = "Jasmine tea on the deck";

fn reply() -> String {
    json!({
        "title": TEA,
        "text": "Ann brewed jasmine tea for whoever passed by. Bo came to see about the water.",
        "persons": ["ann", "bo"],
        "group": {"members": ["ann", "bo"], "purpose": "a pot of jasmine tea"},
        "talk": ["ann"],
    })
    .to_string()
}

fn emulated() -> api::endpoints::Endpoints {
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    ep
}

/// One event a day, and a user who has said something.
fn setup(ep: api::endpoints::Endpoints) -> Env {
    let e = env_with(ep, |c| c.world.events.per_day = 1);
    speak(&e);
    e
}

fn speak(e: &Env) {
    e.agent.with_world(|w| {
        let ch = channel::direct(w, &p("ann")).unwrap();
        message::append(w, ch, &Actor::User, "hello", None, None, T0 - 1000).unwrap();
    });
}

fn host(e: &Env) -> ChannelId {
    e.agent.with_world(|w| channel::host(w).unwrap())
}

fn posts(e: &Env) -> Vec<api::views::Message> {
    e.agent.history(host(e), None, 20).unwrap()
}

fn body_text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn requests(e: &Env) -> usize {
    e.fake.bodies().len()
}

fn both() -> [api::endpoints::Endpoints; 2] {
    [endpoints(WireApi::Chat, true, None), emulated()]
}

#[tokio::test]
async fn an_event_is_made_once_a_day_and_posted_with_offers() {
    for ep in both() {
        let e = setup(ep);
        e.fake.push([say(&reply())]);
        e.agent.tick(true).await.unwrap();
        e.agent.tick(true).await.unwrap();
        assert_eq!(requests(&e), 1, "once a day");
        let posted = posts(&e);
        assert_eq!(posted.len(), 1);
        assert!(posted[0].text.contains(TEA) && posted[0].text.contains("Ann brewed"));
        assert_eq!(posted[0].author, api::types::Author::Host);
        let kinds: Vec<_> = posted[0].actions.iter().collect();
        assert!(matches!(
            kinds[0],
            ToolAction::CreateGroup { members, channel: None, purpose }
                if members.len() == 2 && purpose.as_deref() == Some("a pot of jasmine tea")
        ));
        assert!(matches!(
            kinds[1],
            ToolAction::FindPeople { persons, .. } if persons.len() == 1
        ));
        // The post is also the host's unread line, and a metered host call.
        let metered: i64 = e.agent.with_world(|w| {
            w.query_row("SELECT count(*) FROM meter WHERE shape = 'host'", [], |r| {
                r.get(0)
            })
            .unwrap()
        });
        assert_eq!(metered, 1);
        assert_eq!(e.agent.daily_events().unwrap().len(), 1);
        // The next day brings another; the one missed in between is not made up.
        e.clock.advance(DAY_MS);
        e.fake.push([say(&reply().replace(TEA, "Another"))]);
        e.agent.tick(true).await.unwrap();
        assert_eq!(requests(&e), 2);
        assert_eq!(e.agent.daily_events().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn the_request_asks_for_json_and_shows_the_shape_only_where_it_is_not_enforced() {
    let native = setup(endpoints(WireApi::Chat, true, None));
    native.fake.push([say(&reply())]);
    native.agent.tick(true).await.unwrap();
    let em = setup(emulated());
    em.fake.push([say(&reply())]);
    em.agent.tick(true).await.unwrap();
    let (n, m) = (&native.fake.json()[0], &em.fake.json()[0]);
    for body in [n, m] {
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert!(body.get("tools").is_none());
    }
    let text = |b: &Value| b["messages"][0]["content"].to_string();
    assert!(!text(n).contains("\"title\""), "{}", text(n));
    assert!(text(m).contains("title"), "{}", text(m));
    // The theme comes from the rotating list, and the people from the roster.
    assert!(text(n).contains("ann: ann"));
}

#[tokio::test]
async fn nothing_is_made_unless_the_user_is_present_and_the_moment_is_open() {
    // Not present: the window hidden.
    let e = setup(endpoints(WireApi::Chat, true, None));
    e.agent.tick(false).await.unwrap();
    assert_eq!(requests(&e), 0);
    // Quiet hours.
    e.clock.advance(11 * HOUR_MS + 30 * MINUTE_MS);
    e.agent.tick(true).await.unwrap();
    assert_eq!(requests(&e), 0);
    // The quiet quota band.
    let q = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.world.events.per_day = 1;
        c.world.quota.quiet_at = 0.0;
    });
    speak(&q);
    q.agent.tick(true).await.unwrap();
    assert_eq!(requests(&q), 0);
    // Library mode: no endpoint at all.
    let l = env_with(api::endpoints::Endpoints::default(), |c| {
        c.world.events.per_day = 1
    });
    speak(&l);
    l.agent.tick(true).await.unwrap();
    assert_eq!(requests(&l), 0);
    // A world where the user has not yet said anything keeps to the welcome.
    let n = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.world.events.per_day = 1
    });
    n.agent.tick(true).await.unwrap();
    assert_eq!(requests(&n), 0);
}

#[tokio::test]
async fn presence_makes_it_too() {
    let e = setup(endpoints(WireApi::Chat, true, None));
    // The welcome first.
    e.agent.presence(true).await.unwrap();
    assert_eq!(requests(&e), 0);
    e.clock.advance(HOUR_MS);
    // The digest has a birthday in it, so the host speaks of that too.
    e.fake.push([say("It is ann's birthday today.")]);
    e.fake.push_for(ASK, [say(&reply())]);
    let out = e.agent.presence(true).await.unwrap();
    assert!(out.host_spoke);
    let posted = posts(&e);
    assert_eq!(posted.len(), 3, "the welcome, the birthday line, the event");
    assert!(posted[2].text.contains(TEA));
    // Today's birthdays are no material for the event: with hundreds of people nearly
    // every day has one.
    let ask = e
        .fake
        .bodies()
        .iter()
        .map(|b| String::from_utf8(b.clone()).unwrap())
        .find(|b| b.contains(ASK))
        .expect("the event request");
    assert!(!ask.contains("birthday"), "{ask}");
}

#[tokio::test]
async fn the_setting_chooses_how_many_and_zero_turns_them_off() {
    let e = setup(endpoints(WireApi::Chat, true, None));
    e.agent
        .with_world(|w| settings::set(w, settings::EVENTS_PER_DAY, &0).unwrap());
    e.agent.tick(true).await.unwrap();
    assert_eq!(requests(&e), 0);
    e.agent
        .with_world(|w| settings::set(w, settings::EVENTS_PER_DAY, &2).unwrap());
    e.fake
        .push([say(&reply()), say(&reply().replace(TEA, "Later"))]);
    e.agent.tick(true).await.unwrap();
    e.agent.tick(true).await.unwrap();
    assert_eq!(requests(&e), 1, "a failed or finished try waits");
    e.clock.advance(61 * MINUTE_MS);
    e.agent.tick(true).await.unwrap();
    assert_eq!(requests(&e), 2);
    e.clock.advance(61 * MINUTE_MS);
    e.agent.tick(true).await.unwrap();
    assert_eq!(requests(&e), 2, "two a day at most");
}

#[tokio::test]
async fn a_reply_that_is_not_an_event_is_skipped_quietly() {
    for bad in [
        "Something nice happened.".to_owned(),
        json!({"title": "", "text": "x", "persons": ["ann"], "group": null, "talk": []})
            .to_string(),
        json!({"title": "T", "text": "x", "persons": ["nobody"], "group": null, "talk": []})
            .to_string(),
        "```json\n{\"headline\": 1}\n```".to_owned(),
    ] {
        let e = setup(emulated());
        e.fake.push([say(&bad)]);
        e.agent.tick(true).await.unwrap();
        assert_eq!(requests(&e), 1);
        assert!(posts(&e).is_empty(), "{bad}");
        assert!(e.agent.daily_events().unwrap().is_empty());
        // The failed try is not repeated at once.
        e.agent.tick(true).await.unwrap();
        assert_eq!(requests(&e), 1);
    }
    // A refused call is as quiet.
    let e = setup(endpoints(WireApi::Chat, true, None));
    e.fake.push([agent_fail()]);
    e.agent.tick(true).await.unwrap();
    assert!(posts(&e).is_empty());
}

fn agent_fail() -> Reply {
    Reply::Fail("down".into())
}

#[tokio::test]
async fn offers_for_people_the_host_may_not_name_are_dropped() {
    let e = setup(endpoints(WireApi::Chat, true, None));
    let doc = json!({
        "title": "A quiet shift", "text": "Nothing much. Ann sat by the window.",
        "persons": ["ann", "cy"],
        "group": {"members": ["ann", "cy"], "purpose": "x"},
        "talk": ["cy", "dee"],
    });
    e.fake.push([say(&doc.to_string())]);
    e.agent.tick(true).await.unwrap();
    let posted = &posts(&e)[0];
    // `cy` has no persona: no gathering of one, and only `dee` to talk to.
    assert_eq!(posted.actions.len(), 1);
    assert!(matches!(&posted.actions[0],
        ToolAction::FindPeople { persons, .. } if persons.len() == 1 && persons[0].id.0 == "dee"));
    let involved = e.agent.daily_events().unwrap()[0].persons.clone();
    assert_eq!(involved, vec![p("ann")]);
}

#[tokio::test]
async fn the_people_it_involves_know_of_it_that_day_and_not_the_next() {
    let e = setup(endpoints(WireApi::Chat, true, None));
    // A conversation already open with `bo` when it happens.
    let bo = e.agent.open_direct(&p("bo")).unwrap();
    e.fake.push([say("Hm.")]);
    e.agent.send(bo, "hello", None).await.unwrap();
    e.fake.push([say(&reply())]);
    e.agent.tick(true).await.unwrap();
    // Involved: the open segment is told on its next turn, once.
    e.fake.push([say("Yes."), say("Still.")]);
    e.agent
        .send(bo, "what happened today?", None)
        .await
        .unwrap();
    let told = body_text(e.fake.bodies().last().unwrap());
    assert!(told.contains("Bo came to see about the water"), "{told}");
    e.agent.send(bo, "and then?", None).await.unwrap();
    let again = body_text(e.fake.bodies().last().unwrap());
    assert_eq!(
        again.matches("Bo came to see about the water").count(),
        1,
        "told once per segment"
    );
    // A new segment the same day carries it in its opening block, for an involved person.
    let ann = e.agent.open_direct(&p("ann")).unwrap();
    e.fake.push([say("Mm.")]);
    e.agent.send(ann, "hi", None).await.unwrap();
    assert!(body_text(e.fake.bodies().last().unwrap()).contains("Ann brewed jasmine tea"));
    // The next day it is gone from the world, and a fresh segment does not have it.
    e.clock.advance(DAY_MS);
    e.agent.with_world(|w| {
        let _ = fact::sweep_expired(w, e.clock.0.load(std::sync::atomic::Ordering::SeqCst));
    });
    let facts = e.agent.with_world(|w| fact::all(w, true).unwrap());
    assert!(facts.is_empty(), "{facts:?}");
    let dee = e.agent.open_direct(&p("dee")).unwrap();
    e.fake.push([say("Mm.")]);
    e.agent.send(dee, "hi", None).await.unwrap();
    assert!(!body_text(e.fake.bodies().last().unwrap()).contains("jasmine tea for whoever"));
    // The log keeps the event itself.
    assert_eq!(e.agent.daily_events().unwrap().len(), 1);
}

#[tokio::test]
async fn one_of_them_may_bring_it_up_as_an_opening_within_the_usual_limits() {
    let e = setup(endpoints(WireApi::Chat, true, None));
    e.agent.with_world(|w| {
        for who in ["ann", "bo"] {
            world::bond::set_mode(w, &p(who), world::Mode::Enabled).unwrap();
        }
    });
    e.fake.push_for(ASK, [say(&reply())]);
    e.agent.tick(true).await.unwrap();
    // A user who is not mid-conversation, back after a while.
    e.clock.advance(2 * HOUR_MS);
    e.fake.push([
        say("It is ann's birthday today."),
        say("About that tea, Doc."),
    ]);
    let out = e.agent.presence(true).await.unwrap();
    let (who, ch) = out.opened.expect("an opening");
    assert!(["ann", "bo"].contains(&who.as_str()));
    let opened = body_text(e.fake.bodies().last().unwrap());
    assert!(opened.contains("part of"), "{opened}");
    assert!(opened.contains("Ann brewed jasmine tea"), "{opened}");
    let said = e.agent.history(ch, None, 5).unwrap();
    assert_eq!(said.last().unwrap().text, "About that tea, Doc.");
}

#[tokio::test]
async fn the_host_post_reaches_the_ui_as_one_message_with_its_offers() {
    let e = setup(endpoints(WireApi::Chat, true, None));
    e.sink.take();
    e.fake.push([say(&reply())]);
    e.agent.tick(true).await.unwrap();
    let added: Vec<_> = e
        .sink
        .take()
        .into_iter()
        .filter_map(|ev| match ev {
            Event::MessageAdded(a) => Some(a.message),
            _ => None,
        })
        .collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].actions.len(), 2);
    let stored = e.agent.with_world(|w| event::recent(w, 5).unwrap());
    assert_eq!(
        stored[0].message.map(|m| m.0.to_string()),
        Some(added[0].id.0.clone())
    );
}
