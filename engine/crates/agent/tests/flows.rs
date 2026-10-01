//! The agent's acceptance tests, over a scripted endpoint: the prefix invariant on every
//! kind of log, meta-layer isolation, name substitution and rename rollover, metering,
//! and the persona gate.

mod common;

use agent::store;
use api::endpoints::WireApi;
use common::*;
use serde_json::{Value, json};
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

fn items(v: &Value) -> &Vec<Value> {
    v.get("input")
        .or_else(|| v.get("messages"))
        .and_then(Value::as_array)
        .unwrap()
}

/// The head of a body: everything but the item list and the tail.
fn head(v: &Value) -> Value {
    let mut h = v.clone();
    let o = h.as_object_mut().unwrap();
    o.remove("input");
    o.remove("messages");
    o.remove("tool_choice");
    h
}

fn body_text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

#[tokio::test]
async fn direct_turns_extend_one_prefix_on_both_wire_apis() {
    for wire in [WireApi::Responses, WireApi::Chat] {
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
async fn material_is_deduplicated_and_low_confidence_sends_none() {
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
    let entry = items(v).last().unwrap().to_string();
    assert!(entry.contains("The archive has nothing on this."));
    assert!(entry.contains("request_join"));
    assert!(!entry.contains("[#"));
}

#[tokio::test]
async fn a_request_runs_on_the_direct_log_and_ends_with_its_report() {
    let e = env(WireApi::Responses);
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
    let ask_entry = items(&e.fake.json()[1]).last().unwrap().to_string();
    assert!(ask_entry.contains("look into this") && ask_entry.contains("turns left"));
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["direct", "ask", "ask", "wrapup"]);
    // The wrap-up was forced, after every item.
    let last = &e.fake.json()[3];
    assert_eq!(last["tool_choice"]["name"], "wrapup");
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
    let entry = items(&e.fake.json()[0]).last().unwrap().to_string();
    assert!(entry.contains("last turn on this request"));
}

#[tokio::test]
async fn report_and_wrapup_are_refused_where_they_do_not_belong() {
    let e = env(WireApi::Responses);
    let ch = direct(&e, "ann");
    e.fake.push([
        call("report", json!({"text": "x"})),
        call("wrapup", json!({"facts": []})),
        say("ok"),
    ]);
    e.agent.send(ch, "hi", None).await.unwrap();
    let log = log_bytes(&e.agent, ch);
    assert!(log.contains("There is no request to report on."));
    assert!(log.contains("wrapup is only for closing a conversation."));
    assert!(
        e.agent
            .with_world(|w| fact::all(w, true).unwrap())
            .is_empty()
    );
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
async fn wrapup_closes_the_conversation_on_its_own_log() {
    let e = env(WireApi::Chat);
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
    assert_eq!(v["tool_choice"]["function"]["name"], "wrapup");
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
    let e = env_with(endpoints(WireApi::Responses, true, None), |c| {
        c.agent.roll.k = 10.0
    });
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
    assert_eq!(
        json[1]["prompt_cache_key"],
        format!("dramatis-{}", segs[0].id)
    );
    assert_eq!(
        json[2]["prompt_cache_key"],
        format!("dramatis-{}", segs[1].id)
    );
    assert_eq!(items(&json[2]).len(), 4, "A, B, C, the new turn");
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
    let e = env(WireApi::Responses);
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
    let first = items(&e.fake.json()[0]).last().unwrap().to_string();
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
    let named = items(&json[1]).last().unwrap().to_string();
    assert!(
        named.contains("ann answers."),
        "named person first: {named}"
    );
    let inter = items(&json[2]).last().unwrap().to_string();
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
    let joined = items(&json[1]).last().unwrap().to_string();
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
async fn the_host_has_its_own_log_tools_and_authorised_actions() {
    let e = env(WireApi::Responses);
    // First open: the host introduces itself; a tool call it attempts is refused.
    e.fake.push([call(
        "set_mode",
        json!({"target_kind": "person", "target": "ann", "mode": "enabled"}),
    )]);
    let out = e.agent.presence(true).await.unwrap();
    assert!(!out.host_spoke);
    assert_eq!(
        e.agent.with_world(|w| bond::mode(w, &p("ann")).unwrap()),
        Mode::Frozen
    );
    let host = e.agent.with_world(|w| channel::host(w).unwrap());
    assert!(log_bytes(&e.agent, host).contains("Only when Doc asks for it."));
    assert_eq!(e.fake.json()[0]["tool_choice"], "none");

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
    let digest = items(&json[3]).last().unwrap().to_string();
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
async fn usage_is_metered_per_shape_with_cache_hits_and_quota() {
    let e = env_with(endpoints(WireApi::Responses, true, None), |c| {
        c.world.quota.window_5h.middle = 1_000_000.0;
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
        agent::meter::report(w, now, world::quota::Span::FiveHours, Some(&status)).unwrap()
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
        .with_world(|w| store::totals_by_shape(w, 0, now).unwrap());
    assert_eq!(totals[0].0, Shape::Direct);
    assert_eq!(totals[0].1.calls, 2);
}

#[tokio::test]
async fn an_exhausted_window_stops_calls_with_the_hosts_fixed_line() {
    let e = env_with(endpoints(WireApi::Responses, true, None), |c| {
        c.world.quota.window_5h.middle = 10.0;
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
    let entry = items(&e.fake.json()[0]).last().unwrap().to_string();
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
    e.fake
        .push([say("Welcome."), say("Happy birthday, Dr.Doc.")]);
    let out = e.agent.presence(true).await.unwrap();
    assert!(out.host_spoke);
    let (who, ch) = out.opened.unwrap();
    assert_eq!(who, p("ann"));
    let entry = items(&e.fake.json()[1]).last().unwrap().to_string();
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
    e.fake.push([
        call("wrapup", json!({"facts": []})),
        say("Still a birthday."),
    ]);
    let again = e.agent.presence(false).await.unwrap();
    assert!(again.opened.is_none() && again.host_spoke);
    assert_eq!(e.fake.left(), 0);
    let shapes: Vec<String> = meter(&e.agent).into_iter().map(|r| r.0).collect();
    assert_eq!(shapes, ["host", "opening", "wrapup", "host"]);
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
    assert_eq!(e.agent.tick().await.unwrap(), None);
    assert_eq!(e.fake.bodies().len(), 1, "not idle yet: no call");
    e.clock.advance(31 * 60_000);
    e.fake.push([call(
        "wrapup",
        json!({"facts": [{"kind": "fact", "text": "Said hello"}]}),
    )]);
    e.agent.tick().await.unwrap();
    assert_eq!(e.fake.bodies().len(), 2);
    assert_prefix_chain(&e.fake.bodies());
    assert_eq!(
        e.agent.with_world(|w| fact::all(w, false).unwrap()).len(),
        1
    );
    e.agent.tick().await.unwrap();
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
    assert!(!bodies[1].starts_with(agent::open_prefix(&bodies[0])));
    assert_prefix_chain(&bodies[1..]);
    let v = &e.fake.json()[2];
    let all = items(v).iter().map(Value::to_string).collect::<String>();
    assert!(
        all.contains("one") && all.contains("two"),
        "the whole log is sent"
    );
}
