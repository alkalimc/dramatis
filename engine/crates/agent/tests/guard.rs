//! A reply that is no reply: boilerplate, a document or a call written as speech, nothing.
//! It is retried once, then the turn fails; none of it is stored or shown. Habits of a good
//! reply (the speaker's own name, message tags) are tidied away.

mod common;

use std::time::Duration;

use api::endpoints::WireApi;
use common::*;
use world::ChannelId;

fn direct(e: &Env, who: &str) -> ChannelId {
    e.agent.open_direct(&p(who)).unwrap()
}

const WRAPUP_JSON: &str = r#"{"facts":[],"hurt":{},"summary":""}"#;
const PLAIN: &str = "Cooler water works better, and a little patience.";

fn diagnostics(e: &Env) -> String {
    std::fs::read_to_string(e._dir.0.join("logs/dramatis.log")).unwrap_or_default()
}

fn meter(e: &Env) -> usize {
    e.agent.with_world(|w| {
        w.query_row("SELECT count(*) FROM meter", [], |r| r.get::<_, usize>(0))
            .unwrap()
    })
}

fn replies(e: &Env, ch: ChannelId) -> Vec<String> {
    e.agent
        .history(ch, None, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect()
}

#[tokio::test]
async fn wrapup_json_as_a_reply_is_retried_once_and_never_kept() {
    // Messages 293 and 302 of the owner's save were exactly this.
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([say(WRAPUP_JSON), say(PLAIN)]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    assert_eq!(replies(&e, ch), ["tea?", PLAIN]);
    assert!(!log_bytes(&e.agent, ch).contains("facts"));
    assert!(!format!("{:?}", e.sink.take()).contains("facts"));
    assert_eq!(meter(&e), 2);
    assert!(diagnostics(&e).contains("reason=json"));
}

#[tokio::test]
async fn garbage_twice_fails_the_turn_and_stores_nothing() {
    for (first, second) in [
        (WRAPUP_JSON, "```json\n{\"facts\": []}\n```"),
        (
            r#"```tool_call {"name": "wrapup", "arguments": {"facts":[]}} ```"#,
            "   ",
        ),
        ("#msg12", "[]"),
    ] {
        let e = env(WireApi::Chat);
        let ch = direct(&e, "ann");
        e.fake.push([say(first), say(second)]);
        let err = e.agent.send(ch, "tea?", None).await.unwrap_err();
        assert!(matches!(err, agent::error::Error::Refused), "{first}");
        assert_eq!(replies(&e, ch), ["tea?"], "{first}");
        let shown = format!("{:?}", e.sink.take());
        assert!(shown.contains("Failed") && shown.contains("Refused"));
    }
}

#[tokio::test]
async fn the_owners_boilerplate_replies_are_retried_and_a_server_room_is_not() {
    for canned in [
        "Sorry, something went wrong. Please try your request again.",
        "I'm having a hard time fulfilling your request. Can I help you with something else instead?",
        "As a large language model, I do not have personal opinions.",
        "(smiles) I'm Gemini, a model trained by Google.",
    ] {
        let e = env(WireApi::Chat);
        let ch = direct(&e, "ann");
        e.fake.push([say(canned), say(PLAIN)]);
        e.agent.send(ch, "tea?", None).await.unwrap();
        assert_eq!(replies(&e, ch), ["tea?", PLAIN], "{canned}");
        assert!(diagnostics(&e).contains("what=refused"), "{canned}");
    }
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    let room = "The server room hums on the third floor, as it always does.";
    e.fake.push([say(room)]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    assert_eq!(replies(&e, ch), ["tea?", room]);
}

#[tokio::test]
async fn the_corpus_rules_apply_in_its_own_language() {
    // The test corpus lists the search engine's name and "scholar" as `boilerplate.any`.
    let e = env_cjk(WireApi::Chat);
    let ch = direct(&e, "ann");
    let brand = "\u{6211}\u{53bb}\u{8c37}\u{6b4c}\u{67e5}\u{4e86}\u{4e00}\u{4e0b}\u{3002}";
    let good = "\u{6c34}\u{51c9}\u{4e00}\u{70b9}\u{66f4}\u{597d}\u{3002}";
    e.fake.push([say(brand), say(good)]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    assert_eq!(replies(&e, ch), ["tea?", good]);
    assert!(!log_bytes(&e.agent, ch).contains("\u{8c37}\u{6b4c}"));
    let log = diagnostics(&e);
    assert!(log.contains("reason=boilerplate"));
}

#[tokio::test]
async fn the_speakers_own_name_and_message_tags_do_not_reach_the_message_or_the_log() {
    let e = env(WireApi::Chat);
    let ch = direct(&e, "ann");
    e.fake.push([
        say("ann: (looks up) The water wants to be cooler.\nAnn\u{ff1a}And a little patience.\n#msg1"),
        say("(ann: nods) Fine.\n[#msg2]"),
    ]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    e.agent.send(ch, "and then?", None).await.unwrap();
    let all = replies(&e, ch);
    assert_eq!(
        all[1],
        "(looks up) The water wants to be cooler.\nAnd a little patience."
    );
    assert_eq!(all[3], "(nods) Fine.");
    let log = log_bytes(&e.agent, ch);
    // (The user's own entries carry their tags; the person's replies do not.)
    assert!(log.contains("(looks up) The water wants to be cooler."));
    assert!(!log.contains("ann: ") && !log.contains("Ann\u{ff1a}"));
}

#[tokio::test]
async fn an_emulated_wrapup_written_on_one_line_is_a_call_not_a_refusal() {
    // The shape Gemini wrote four times in the owner's log.
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let e = env_with(ep, |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([
        say(PLAIN),
        say(
            r#"```tool_call {"name": "wrapup", "arguments": {"facts":[],"hurt":{"person":"","quote":""},"summary":""}} ```"#,
        ),
    ]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    assert_eq!(meter(&e), 2);
    let log = diagnostics(&e);
    assert!(!log.contains("what=refused"), "{log}");
}

#[tokio::test]
async fn a_wrapup_leaves_no_trace_in_the_log_for_the_next_turn_to_copy() {
    // The owner's openings came back as the wrap-up document because the segment ended
    // with the wrap-up's request and answer.
    let mut ep = endpoints(WireApi::Chat, true, None);
    ep.profiles[0].models[0].native_tools = Some(false);
    let e = env_with(ep, |_| {});
    let ch = direct(&e, "ann");
    e.fake.push([say(PLAIN), say(WRAPUP_JSON)]);
    e.agent.send(ch, "tea?", None).await.unwrap();
    e.agent.wrapup(ch).await.unwrap();
    let log = log_bytes(&e.agent, ch);
    assert!(log.contains(PLAIN));
    assert!(!log.contains("facts"), "{log}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_that_never_answers_ends_the_turn_as_timed_out() {
    let e = env_with(endpoints(WireApi::Chat, true, None), |c| {
        c.agent.timeout.first_token_secs = 1;
        c.agent.timeout.total_secs = 2;
    });
    let ch = direct(&e, "ann");
    let hold = Hold::new(usize::MAX);
    e.fake.hold(&hold);
    e.fake.push([say("never seen")]);
    let started = std::time::Instant::now();
    let err = tokio::time::timeout(Duration::from_secs(20), e.agent.send(ch, "hello", None))
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, agent::error::Error::TimedOut { .. }), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    hold.release();
    let shown = format!("{:?}", e.sink.take());
    assert!(
        shown.contains("Failed") && shown.contains("TimedOut"),
        "{shown}"
    );
    assert_eq!(replies(&e, ch), ["hello"]);
    assert!(diagnostics(&e).contains("what=timed out"));
    // The message is kept and a send again works.
    e.fake.push([say(PLAIN)]);
    let id = e.agent.history(ch, None, 5).unwrap()[0].id.clone();
    let text = e
        .agent
        .rewind(ch, world::MessageId(id.0.parse().unwrap()))
        .unwrap();
    e.agent.send(ch, &text, None).await.unwrap();
    assert_eq!(replies(&e, ch), ["hello", PLAIN]);
}
