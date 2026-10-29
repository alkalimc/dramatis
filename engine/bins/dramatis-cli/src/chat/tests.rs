//! The session end to end over a scripted endpoint: no network, no real key.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

use agent::endpoint::StaticKeys;
use api::home::Home;
use api::{Api, Engine};
use async_trait::async_trait;
use folio::fixture::{Builder, TempFolio, UnitSpec};
use futures::StreamExt;
use llm::Transport;
use llm::transport::{HttpRequest, HttpResponse, StreamResponse};
use runtime::{Options, Runtime};
use serde_json::json;

use super::command::{Command, parse};
use super::interrupt::Interrupt;
use super::render::{Marker, Style};
use super::session::Session;
use super::show::{Names, Show, Term};

enum Reply {
    Say(&'static str),
    /// A tool call with its JSON arguments.
    Tool(&'static str, serde_json::Value),
    Fail(&'static str),
    /// The endpoint never answers.
    Hang,
}

/// Answers chat-completion streams from a script, cutting each text in two deltas.
#[derive(Default)]
struct Scripted(Mutex<VecDeque<Reply>>);

fn sse(events: &[serde_json::Value]) -> String {
    let mut s: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    s.push_str("data: [DONE]\n\n");
    s
}

#[async_trait]
impl Transport for Scripted {
    async fn send(&self, _: HttpRequest) -> llm::Result<HttpResponse> {
        unreachable!("chat only streams")
    }

    async fn open(&self, _: HttpRequest) -> llm::Result<StreamResponse> {
        let reply = self.0.lock().unwrap().pop_front().expect("script ran out");
        let body = match reply {
            Reply::Say(text) => {
                let mid = text.char_indices().nth(text.chars().count() / 2).unwrap().0;
                let delta = |t: &str| json!({"choices": [{"index": 0, "delta": {"content": t}}]});
                sse(&[
                    delta(&text[..mid]),
                    delta(&text[mid..]),
                    json!({"choices": [], "usage": {"prompt_tokens": 40,
                        "completion_tokens": 7, "prompt_tokens_details": {"cached_tokens": 8}}}),
                ])
            }
            Reply::Tool(name, args) => {
                let chunk = |d: serde_json::Value| json!({"choices": [{"index": 0, "delta": d}]});
                sse(&[
                    chunk(json!({"tool_calls": [{"index": 0, "id": "call_1",
                        "function": {"name": name, "arguments": ""}}]})),
                    chunk(json!({"tool_calls": [{"index": 0,
                        "function": {"arguments": args.to_string()}}]})),
                    json!({"choices": [], "usage": {"prompt_tokens": 40, "completion_tokens": 7}}),
                ])
            }
            Reply::Fail(message) => sse(&[json!({"error": {"message": message}})]),
            Reply::Hang => std::future::pending().await,
        };
        Ok(StreamResponse {
            status: 200,
            chunks: futures::stream::iter(vec![Ok(body.into_bytes())]).boxed(),
            retry_after: None,
        })
    }
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

const WORDING: &str = r#"{"user.you": "Doc", "host.name": "Hostess"}"#;

fn corpus(wording: &str, host: bool) -> TempFolio {
    let mut b = Builder::new()
        .manifest("user_placeholder", r#""<U>""#)
        .manifest("wording", wording)
        .manifest("scene_marker", r#"["(", ")"]"#);
    b.person("ann", None).person("bo", None).person("cy", None);
    b.alias("Annie", "ann", "term_exact");
    b.alias("Lady A", "ann", "redirect");
    b.alias("Lady B", "bo", "redirect");
    for (id, page, text, persons) in [
        (
            "u1",
            "Tea page",
            "Ann said the jasmine tea needs cooler water",
            &["ann"][..],
        ),
        (
            "u2",
            "Forge page",
            "Bo hammered the blade at the forge until dawn",
            &["bo"][..],
        ),
    ] {
        b.unit(UnitSpec {
            id,
            template: "dialogue",
            page,
            header: page,
            text,
            persons,
            span: None,
        });
    }
    for subject in ["ann", "cy"] {
        for (slot, body) in [
            ("system", "You are a person."),
            ("tone", "Brief."),
            ("capability", "Knows tea."),
        ] {
            b.conn()
                .execute(
                    "INSERT INTO prompts(subject, slot, body) VALUES (?1, ?2, ?3)",
                    [subject, slot, body],
                )
                .unwrap();
        }
    }
    if host {
        b.conn()
            .execute(
                "INSERT INTO prompts(subject, slot, body) VALUES ('host', 'in_world', 'You run the desk.')",
                [],
            )
            .unwrap();
    }
    b.finish()
}

/// A throwaway home directory.
struct TempHome(std::path::PathBuf);

impl TempHome {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("dramatis-chat-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Rig {
    session: Session,
    completions: super::complete::Completions,
    show: Arc<Show>,
    engine: Arc<Engine>,
    names: Arc<Names>,
    bridge: Arc<runtime::AgentBridge>,
    interrupt: Arc<Interrupt>,
    out: Buf,
    script: Arc<Scripted>,
    _home: TempHome,
    _folio: TempFolio,
}

async fn rig() -> Rig {
    rig_wording(WORDING).await
}

async fn rig_wording(wording: &str) -> Rig {
    rig_with(wording, false).await
}

async fn rig_with(wording: &str, host: bool) -> Rig {
    rig_full(wording, host, true, None).await
}

/// `with_save: false` is a home where the user has not picked a save yet.
async fn rig_full(wording: &str, host: bool, with_save: bool, recorded: Option<&str>) -> Rig {
    rig_setup(wording, host, with_save, recorded, true).await
}

/// `endpoints: false` is a home with no endpoints file: library mode from the start.
async fn rig_setup(
    wording: &str,
    host: bool,
    with_save: bool,
    recorded: Option<&str>,
    endpoints: bool,
) -> Rig {
    let folio = corpus(wording, host);
    let home = TempHome::new();
    if endpoints {
        std::fs::write(
            home.0.join("endpoints.toml"),
            "[[profile]]\nname = 'p'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = 'chat'\n\
             [[profile.model]]\nid = 'model-a'\n\
             [roles]\nchat = { profile = 'p', model = 'model-a' }\n",
        )
        .unwrap();
    }
    // The scripted endpoint streams only: no ship event, which is a whole reply.
    std::fs::write(home.0.join("params.toml"), "[events]\nper_day = 0\n").unwrap();
    if with_save {
        // A save to open, as a first run leaves the home after the user picks one.
        let h = Home::new(&home.0);
        h.create().unwrap();
        drop(world::World::open(h.save_path("default")).unwrap());
    }
    if let Some(name) = recorded {
        let h = Home::new(&home.0);
        h.create().unwrap();
        h.set_current_save(name).unwrap();
    }
    let out = Buf::default();
    let term = Arc::new(Term::new(Box::new(out.clone()), Style::PLAIN));
    let show = Arc::new(Show::new(term.clone()));
    let script = Arc::new(Scripted::default());
    let runtime = Runtime::open(
        Home::new(&home.0),
        Some(folio.path().to_owned()),
        show.clone(),
        Options {
            transport: Some(script.clone()),
            keys: Some(Arc::new(StaticKeys(vec![(
                "p".into(),
                llm::Secret::new("sk-test"),
            )]))),
            quiet: true,
            frontend: runtime::Frontend::Terminal,
            password: None,
            restart: None,
            choose_file: None,
        },
    )
    .unwrap();
    let engine = runtime.engine;
    let names = Arc::new(Names::load(&engine).await.unwrap());
    let scene = engine.wording().await.unwrap().scene_marker;
    show.bind(names.clone(), Marker::new(&(scene.open, scene.close)));
    let bridge = runtime.bridge.unwrap();
    let completions = super::complete::Completions::default();
    let interrupt = Arc::new(Interrupt::default());
    Rig {
        session: Session::new(
            engine.clone(),
            bridge.clone(),
            term,
            show.clone(),
            names.clone(),
            completions.clone(),
            interrupt.clone(),
        )
        .await,
        completions,
        show,
        engine,
        names,
        bridge,
        interrupt,
        out,
        script,
        _home: home,
        _folio: folio,
    }
}

impl Rig {
    async fn run(&mut self, line: &str) -> String {
        self.session.input(line).await;
        self.out.take()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_cancels_a_hung_turn_and_the_session_goes_on() {
    let mut r = rig().await;
    r.run("/talk ann").await;
    r.script.0.lock().unwrap().push_back(Reply::Hang);
    let interrupt = r.interrupt.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        interrupt.hit();
    });
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), r.run("hello?"))
        .await
        .expect("Ctrl-C ends the wait");
    assert!(out.contains("cancelled"), "{out}");
    r.script.0.lock().unwrap().push_back(Reply::Say("Back."));
    assert!(r.run("again").await.contains("Back."));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_reply_is_rendered_and_the_session_survives_a_failure() {
    let mut r = rig().await;
    let listed = r.run("/list").await;
    assert!(
        listed.contains("ann") && !listed.contains("bo "),
        "{listed}"
    );
    assert!(r.run("/talk nobody").await.contains("nobody matches"));
    assert!(r.run("/talk bo").await.contains("has no persona"));
    assert!(r.run("hello").await.contains("no channel yet"));
    assert!(r.run("/talk ann").await.contains("talking to ann"));

    r.script.0.lock().unwrap().extend([
        Reply::Say("(Ann pours tea)\nCooler water, Doc [#u1]. Always."),
        Reply::Fail("overloaded"),
        Reply::Say("Better now."),
    ]);
    let first = r.run("how do I brew jasmine tea?").await;
    assert!(
        first.starts_with("ann is typing...\nann: Ann pours tea\nCooler water, Doc. Always.\n"),
        "{first:?}"
    );
    assert!(!first.contains("[#"), "{first:?}");
    assert!(
        first.contains("sources: 1  (/sources to show)") && !first.contains("Tea page"),
        "{first:?}"
    );
    assert!(!first.contains("thin archive"), "{first:?}");
    // The prompt carries the user's own name, not the channel's speaker.
    let prompt = r.session.prompt();
    assert!(prompt.starts_with("[ann] Doc"), "{prompt:?}");
    assert!(first.contains("1 call,"), "{first:?}");

    let failed = r.run("and the water?").await;
    assert!(
        failed.contains("error:") && failed.contains("overloaded"),
        "{failed:?}"
    );

    let again = r.run("thanks").await;
    assert!(again.contains("ann: Better now."), "{again:?}");

    // The sources unfold on request.
    let unfolded = r.run("/sources").await;
    assert!(
        unfolded.contains("sources (1):\n  1. Tea page"),
        "{unfolded:?}"
    );
    assert!(!unfolded.contains("jasmine tea needs"), "{unfolded:?}");
    // Then an entry shows its text.
    let entry = r.run("/sources 1 1").await;
    assert!(
        entry.contains("Ann said the jasmine tea needs cooler water"),
        "{entry:?}"
    );
    assert!(r.run("/source 1").await.contains("jasmine tea needs"));
    assert!(r.run("/sources 1 2").await.contains("no entry 2"));
    assert!(r.run("/sources 2").await.contains("no reply with sources"));

    let history = r.run("/history 2").await;
    assert!(history.contains("ann: Better now."), "{history:?}");
    // Taking the last message back drops it and its reply and returns its text.
    let resent = r.run("/resend").await;
    assert!(resent.contains("took back 2 messages"), "{resent:?}");
    assert_eq!(r.session.take_prefill().as_deref(), Some("thanks"));
    let after = r.run("/history 10").await;
    assert!(!after.contains("Better now."), "{after:?}");
    assert!(
        after.contains("Cooler water, Doc. Always.") && after.contains("and the water?"),
        "{after:?}"
    );
    assert!(!after.contains("thanks"), "{after:?}");
    assert!(r.run("/resend 9").await.contains("no message of yours"));
    assert!(r.session.take_prefill().is_none());

    let stats = r.run("/stats").await;
    assert!(
        stats.contains("direct") && stats.contains("model-a") && stats.contains("total"),
        "{stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn search_runs_retrieval_as_the_current_speaker_without_a_model_call() {
    let mut r = rig().await;
    r.run("/talk ann").await;
    let found = r.run("/search jasmine tea").await;
    assert!(
        found.contains("as ann") && found.contains("Tea page"),
        "{found}"
    );
    assert!(r.run("/stats").await.contains("no model calls yet"));
    assert!(r.run("/memories").await.contains("no memories"));
    assert!(r.run("/memories ann").await.contains("no memories"));
    let group = r.run("/group ann bo").await;
    assert!(group.contains("has no persona"), "{group}");
    assert!(r.run("/host").await.contains("no host"));
}

fn host_called(action: api::views::ToolAction) -> api::port::Event {
    api::port::Event::ToolCalled(api::events::ToolCalled {
        channel: api::types::ChannelId("1".into()),
        message: api::types::MessageId("1".into()),
        author: api::types::Author::Host,
        action,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn go_takes_a_choice_the_host_offered_and_name_sets_the_users_name() {
    use api::port::Sink;
    use api::types::PersonId;
    use api::views::ToolAction;
    let mut r = rig().await;
    let person = |id: &str| api::types::PersonRef {
        id: PersonId(id.into()),
        name: id.into(),
        pronoun: None,
    };
    let group = r
        .engine
        .create_group(vec![PersonId("ann".into()), PersonId("cy".into())], None)
        .await
        .unwrap();
    assert!(r.run("/go 1").await.contains("offered nothing"));

    // The host's tool calls become a numbered list after its reply.
    let find = ToolAction::FindPeople {
        topic: "tea".into(),
        persons: vec![person("ann")],
    };
    let host = r.engine.host_channel().await.unwrap();
    let message = {
        let w = world::World::open(Home::new(&r._home.0).save_path("default")).unwrap();
        let calls = serde_json::to_value([&find]).unwrap();
        let id = world::message::append(
            &w,
            world::ChannelId(host.0.parse().unwrap()),
            &world::Actor::Host,
            "Here.",
            Some(&calls),
            None,
            1,
        )
        .unwrap();
        api::types::MessageId(id.to_string())
    };
    r.show
        .emit(api::port::Event::ToolCalled(api::events::ToolCalled {
            channel: host,
            message,
            author: api::types::Author::Host,
            action: find,
        }));
    r.show.emit(host_called(ToolAction::CreateGroup {
        members: vec![person("ann"), person("cy")],
        channel: Some(group),
        purpose: None,
    }));
    r.session.offers_for_test();
    let listed = r.out.take();
    assert!(
        listed.contains("1 talk to ann") && listed.contains("2 open group: ann, cy"),
        "{listed}"
    );
    assert!(r.run("/go 3").await.contains("no such choice"));
    assert!(r.run("/go 2").await.contains("group with ann, cy"));
    assert!(r.session.prompt().starts_with("[ann, cy]"));
    assert!(r.run("/go 1").await.contains("talking to ann"));
    assert!(r.session.prompt().starts_with("[ann]"));

    assert!(r.run("/name").await.contains("usage"));
    assert!(
        r.run("/name Dr. Who")
            .await
            .contains("you are called Dr. Who now")
    );
    assert!(
        r.session.prompt().contains("Dr. Who>"),
        "{}",
        r.session.prompt()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_persons_suggestion_is_a_numbered_choice_and_only_going_makes_the_group() {
    use api::port::Sink;
    use api::types::PersonId;
    use api::views::ToolAction;
    let mut r = rig().await;
    let person = |id: &str| api::types::PersonRef {
        id: PersonId(id.into()),
        name: id.into(),
        pronoun: None,
    };
    // The suggestion is a stored line of ann's message, as it is in a real turn.
    let offer = ToolAction::CreateGroup {
        members: vec![person("ann"), person("cy")],
        channel: None,
        purpose: Some("plan a party".into()),
    };
    let direct = r.engine.open_direct(PersonId("ann".into())).await.unwrap();
    let message = {
        let w = world::World::open(Home::new(&r._home.0).save_path("default")).unwrap();
        let calls = serde_json::to_value([&offer]).unwrap();
        let id = world::message::append(
            &w,
            world::ChannelId(direct.0.parse().unwrap()),
            &world::Actor::Person(world::PersonId("ann".into())),
            "shall we?",
            Some(&calls),
            None,
            1,
        )
        .unwrap();
        api::types::MessageId(id.to_string())
    };
    let called = |action| {
        api::port::Event::ToolCalled(api::events::ToolCalled {
            channel: direct.clone(),
            message: message.clone(),
            author: api::types::Author::Person {
                id: PersonId("ann".into()),
            },
            action,
        })
    };
    r.show.emit(called(offer.clone()));
    r.session.offers_for_test();
    let listed = r.out.take();
    assert!(listed.contains("1 start a group: ann, cy"), "{listed}");
    // Nothing was made or entered by the suggestion itself.
    assert!(!r.session.prompt().starts_with("[ann, cy]"));
    assert!(r.run("/go 1").await.contains("group with ann, cy"));
    assert!(r.session.prompt().starts_with("[ann, cy]"));
    // The purpose is the group's topic.
    let w = world::World::open(Home::new(&r._home.0).save_path("default")).unwrap();
    let topics: Vec<_> = world::channel::list(&w)
        .unwrap()
        .into_iter()
        .filter(|c| c.kind == world::ChannelKind::Group)
        .map(|c| c.topic)
        .collect();
    assert_eq!(topics, [Some("plan a party".to_string())]);
    drop(w);
    // Taking it again opens the same group instead of making another.
    r.run("/talk ann").await;
    assert!(r.run("/go 1").await.contains("group with ann, cy"));
    let w = world::World::open(Home::new(&r._home.0).save_path("default")).unwrap();
    let made = world::channel::list(&w)
        .unwrap()
        .iter()
        .filter(|c| c.kind == world::ChannelKind::Group)
        .count();
    assert_eq!(made, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hosts_channel_list_becomes_choices_that_switch_to_existing_conversations() {
    let mut r = rig_with(WORDING, true).await;
    r.script.0.lock().unwrap().extend([
        Reply::Say("Hello."),
        Reply::Say("Present."),
        Reply::Say("Here too."),
    ]);
    r.run("/talk ann").await;
    r.run("hi").await;
    r.run("/group ann cy").await;
    // Speaking to everyone settles who answers without a model call.
    r.run("good morning everyone").await;

    r.run("/host").await;
    r.script.0.lock().unwrap().extend([
        Reply::Tool("list_channels", json!({})),
        Reply::Say("Two are open. Choose one from the list."),
    ]);
    let out = r.run("how do I get back to a conversation?").await;
    assert!(
        out.contains("1 open group: ann, cy") && out.contains("2 talk to ann"),
        "{out}"
    );
    assert!(out.contains("/go <n> takes you there"), "{out}");
    assert!(r.run("/go 2").await.contains("talking to ann"));
    assert!(r.session.prompt().starts_with("[ann]"));
    assert!(r.run("/go 1").await.contains("group with ann, cy"));
    assert!(r.session.prompt().starts_with("[ann, cy]"));
}

/// Every command the host's terminal guide names. The pack's own guide test keeps the same
/// list; the parser must know them all.
const GUIDE_COMMANDS: [&str; 15] = [
    "talk", "group", "host", "go", "resend", "sources", "name", "memories", "forget", "edit",
    "restore", "tier", "save", "wrapup", "help",
];

#[test]
fn the_terminal_guide_names_only_commands_the_parser_knows() {
    use agent::assemble::{Ctx, guide};
    use agent::text::{Names as Text, Wording};
    let wording = Wording::new(&Default::default());
    let names = Text::new("<U>", None, &wording);
    let ctx = Ctx {
        wording: &wording,
        names: &names,
        scene: ("(", ")"),
    };
    let text = guide(&ctx, agent::Frontend::Terminal);
    let mut named: Vec<&str> = text
        .split_whitespace()
        .filter_map(|w| w.strip_prefix('/'))
        .map(|w| w.trim_end_matches(|c: char| !c.is_ascii_lowercase()))
        .filter(|w| !w.is_empty())
        .collect();
    named.sort_unstable();
    named.dedup();
    let mut expected = GUIDE_COMMANDS.to_vec();
    expected.sort_unstable();
    assert_eq!(named, expected);
    for c in GUIDE_COMMANDS {
        let unknown = matches!(parse(&format!("/{c}")), Command::Unknown(_));
        assert!(!unknown, "/{c} is in the guide but not a command");
    }
    // The app's guide carries none of them.
    assert!(!guide(&ctx, agent::Frontend::App).contains("/talk"));
}

fn activity(who: &str, state: api::events::ActivityState) -> api::port::Event {
    api::port::Event::TurnActivity(api::events::TurnActivity {
        channel: api::types::ChannelId("1".into()),
        author: api::types::Author::Person {
            id: api::types::PersonId(who.into()),
        },
        state,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_typing_line_names_whoever_generates_once_per_turn() {
    use api::events::ActivityState::{Done, Thinking, Tool, Writing};
    use api::port::Sink;
    let r = rig().await;
    // A group: ann answers (thinking, a lookup, thinking, writing), then bo.
    for (who, state) in [
        ("ann", Thinking),
        (
            "ann",
            Tool {
                name: "search".into(),
            },
        ),
        ("ann", Thinking),
        ("ann", Writing),
        ("ann", Done),
        ("bo", Thinking),
        ("bo", Writing),
        ("bo", Done),
    ] {
        r.show.emit(activity(who, state));
    }
    assert_eq!(
        r.out.take(),
        "ann is typing...\nann is looking things up...\nbo is typing...\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_label_lists_three_names_and_counts_the_rest() {
    let r = rig().await;
    let names = |n: usize| (0..n).map(|i| format!("p{i}")).collect::<Vec<_>>();
    let label = |n| r.names.group_label(&names(n));
    assert_eq!(label(2), "p0, p1");
    assert_eq!(label(3), "p0, p1, p2");
    assert_eq!(label(5), "p0, p1, p2 +2");
}

#[tokio::test(flavor = "multi_thread")]
async fn wrapup_ends_the_conversation_and_a_hung_turn_can_be_abandoned() {
    let mut r = rig().await;
    r.run("/talk ann").await;
    let done = r.run("/wrapup").await;
    assert!(
        done.contains("wrapped up; no memories were written"),
        "{done}"
    );

    // A turn whose endpoint never answers is background work; cancelling ends it.
    r.script.0.lock().unwrap().push_back(Reply::Hang);
    let channel = r
        .engine
        .open_direct(api::types::PersonId("ann".into()))
        .await
        .unwrap();
    r.engine
        .send_message(channel, "hello?".into(), None)
        .await
        .unwrap();
    let waiting = tokio::time::timeout(std::time::Duration::from_millis(300), r.bridge.idle());
    assert!(waiting.await.is_err(), "the turn should still be running");
    r.bridge.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), r.bridge.idle())
        .await
        .expect("cancelled work ends");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_start_lines_say_how_to_send_in_the_corpus_wording() {
    let plain = rig().await;
    let [typing, start] = super::intro_lines(&plain.names);
    assert_eq!(typing, "Enter: new line · Option/Alt-Enter: send");
    assert!(start.contains("/help"), "{start}");
    assert_eq!(
        plain.names.text("cli.typing.more"),
        "Option/Alt-Enter: send"
    );

    let worded = rig_wording(
        r#"{"user.you": "Doc", "cli.typing": "line / send", "cli.typing.more": "send key"}"#,
    )
    .await;
    assert_eq!(super::intro_lines(&worded.names)[0], "line / send");
    assert_eq!(worded.names.text("cli.typing.more"), "send key");
}

#[tokio::test(flavor = "multi_thread")]
async fn lang_switches_the_terminal_wording_at_once_and_keeps_the_choice_in_the_home() {
    let mut r = rig_wording(
        r#"{"user.you": "Doc", "host.name": "Host", "cli.list.count.persona": "{n} others", "cli.lang.set": "now {language}"}"#,
    )
    .await;
    assert!(r.run("/list").await.contains(" others"));
    let set = r.run("/lang en").await;
    assert!(set.contains("interface language is now English"), "{set}");
    // The corpus's phrasing is gone; the built-in English is back, with no restart.
    let listed = r.run("/list").await;
    assert!(listed.contains("persons with a persona"), "{listed}");
    assert!(r.run("/lang").await.contains("interface language: English"));
    assert_eq!(
        Home::new(&r._home.0).interface_language(),
        Some(api::views::InterfaceLanguage::English)
    );
    assert!(r.run("/lang corpus").await.contains("now "));
    assert!(r.run("/list").await.contains(" others"));
    assert!(r.run("/lang klingon").await.contains("usage: /lang"));
}

#[tokio::test]
async fn the_first_run_questions_are_optional_and_asked_once_per_home() {
    let home = TempHome::new();
    let h = Home::new(&home.0);
    let engine = Engine::open(h.clone(), None, api::Ports::default()).unwrap();
    engine.new_save("main".into()).await.unwrap();
    engine.switch_save("main".into()).await.unwrap();
    let names = Names::load(&engine).await.unwrap();
    assert!(super::setup::needs_first_run(&engine, &h).await);

    let mut ask = Script::new(&["2", "Rowan", "03-14", "Likes tea"], &[]);
    super::setup::first_run(&engine, &names, &mut ask).await;
    let s = engine.settings().await.unwrap();
    assert_eq!(s.user_name.as_deref(), Some("Rowan"));
    assert_eq!(s.birthday.map(|d| (d.month, d.day)), Some((3, 14)));
    assert_eq!(s.bio.as_deref(), Some("Likes tea"));
    assert_eq!(
        h.interface_language(),
        Some(api::views::InterfaceLanguage::English)
    );
    assert!(!super::setup::needs_first_run(&engine, &h).await);

    // Skipping everything still records the language, so the questions do not return.
    let other = TempHome::new();
    let oh = Home::new(&other.0);
    let e2 = Engine::open(oh.clone(), None, api::Ports::default()).unwrap();
    e2.new_save("main".into()).await.unwrap();
    e2.switch_save("main".into()).await.unwrap();
    let n2 = Names::load(&e2).await.unwrap();
    super::setup::first_run(&e2, &n2, &mut Script::new(&["", "", "", ""], &[])).await;
    assert_eq!(e2.settings().await.unwrap().user_name, None);
    assert_eq!(
        oh.interface_language(),
        Some(api::views::InterfaceLanguage::Corpus)
    );
    assert!(!super::setup::needs_first_run(&e2, &oh).await);
}

/// Two memories in the world, written the way a wrap-up would.
async fn plant_memories(r: &Rig) {
    r.bridge.agent().await.unwrap().with_world(|w| {
        for text in ["Ann likes jasmine tea", "Bo works at the forge"] {
            w.execute(
                "INSERT INTO fact(author, audience_kind, kind, text, created_at)
                     VALUES ('user', 'world', 'fact', ?1, 0)",
                [text],
            )
            .unwrap();
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn memories_are_numbered_and_can_be_forgotten_restored_and_edited() {
    let mut r = rig().await;
    assert!(r.run("/forget 1").await.contains("/memories first"));
    plant_memories(&r).await;
    let listed = r.run("/memories").await;
    assert!(
        listed.contains("1. [fact] Ann likes jasmine tea")
            && listed.contains("2. [fact] Bo works at the forge"),
        "{listed}"
    );
    assert!(r.run("/forget 9").await.contains("no such memory"));
    assert!(r.run("/forget 1").await.contains("deleted"));
    // Numbers are stale once something changed.
    assert!(r.run("/forget 1").await.contains("/memories first"));

    let listed = r.run("/memories").await;
    assert!(
        listed.contains("1. [fact] Bo works at the forge")
            && listed.contains("2. [fact] Ann likes jasmine tea  (deleted)"),
        "{listed}"
    );
    assert!(r.run("/edit 2 nope").await.contains("/restore it first"));
    assert!(r.run("/restore 1").await.contains("not deleted"));
    assert!(r.run("/restore 2").await.contains("restored"));
    let again = r.run("/memories").await;
    let bo = again
        .lines()
        .find(|l| l.contains("Bo works"))
        .and_then(|l| l.split('.').next())
        .unwrap()
        .to_owned();
    let edit = format!("/edit {bo} Bo moved to the tea house");
    assert!(r.run(&edit).await.contains("rewritten"));
    let listed = r.run("/memories").await;
    assert!(
        listed.contains("[fact] Bo moved to the tea house")
            && listed.contains("Bo works at the forge  (deleted)"),
        "{listed}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tier_and_quiet_hours_show_and_set() {
    let mut r = rig().await;
    assert!(r.run("/tier").await.contains("tier middle"));
    assert!(r.run("/tier extra-high").await.contains("tier extra_high"));
    assert!(
        r.run("/tier nonsense")
            .await
            .contains("levels: low, middle")
    );
    assert!(
        r.run("/quiet 23:00-07:00")
            .await
            .contains("on, 23:00 to 07:00")
    );
    assert!(r.run("/quiet off").await.contains("quiet hours off"));
    assert!(r.run("/quiet").await.contains("quiet hours off"));
    assert!(r.run("/quiet on").await.contains("quiet hours on"));
    assert!(r.run("/quiet 25:00-07:00").await.contains("24-hour"));
    assert!(r.run("/quiet noon").await.contains("usage"));
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_list_switch_and_ask_before_deleting() {
    let mut r = rig().await;
    plant_memories(&r).await;
    let listed = r.run("/save").await;
    assert!(listed.starts_with("* default"), "{listed}");
    assert!(r.run("/save new second").await.contains("created second"));
    assert!(r.run("/save new SECOND").await.contains("already exists"));
    assert!(r.run("/save new a/b").await.contains("not a usable name"));

    r.run("/talk ann").await;
    assert!(
        r.run("/save switch second")
            .await
            .contains("now playing second")
    );
    // The new world is empty, and the old channel is not there.
    assert!(r.run("/memories").await.contains("no memories"));
    assert!(r.run("hello").await.contains("no channel yet"));
    assert!(r.session.prompt().contains("[-]"));
    let both = r.run("/save").await;
    assert!(
        both.contains("* second") && both.contains("  default"),
        "{both}"
    );

    // The save in use is protected; another asks first, and a no (or anything else) keeps it.
    assert!(r.run("/save delete second").await.contains("save in use"));
    assert!(r.run("/save delete default").await.contains("(y/n)"));
    assert!(r.run("n").await.contains("cancelled"));
    assert!(r.run("/save").await.contains("default"));
    r.run("/save delete default").await;
    assert!(r.run("y").await.contains("deleted default"));
    assert!(!r.run("/save").await.contains("default"));
    assert!(r.run("/save switch default").await.contains("no such save"));
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_export_and_import_through_a_file() {
    let mut r = rig().await;
    plant_memories(&r).await;
    let file = r._home.0.join("out.sqlite");
    let cmd = format!("/save export default \"{}\"", file.display());
    assert!(r.run(&cmd).await.contains("wrote"));
    // A second export to the same place asks before replacing.
    assert!(r.run(&cmd).await.contains("replace it? (y/n)"));
    assert!(r.run("no").await.contains("cancelled"));
    r.run(&cmd).await;
    assert!(r.run("y").await.contains("wrote"));

    let import = format!("/save import \"{}\" copy", file.display());
    assert!(r.run(&import).await.contains("imported as copy"));
    assert!(r.run(&import).await.contains("already exists"));
    r.run("/save switch copy").await;
    let listed = r.run("/memories").await;
    assert!(listed.contains("Ann likes jasmine tea"), "{listed}");

    std::fs::write(r._home.0.join("junk.sqlite"), b"nothing").unwrap();
    let junk = format!(
        "/save import \"{}\"",
        r._home.0.join("junk.sqlite").display()
    );
    assert!(r.run(&junk).await.contains("not a world database"));
}

#[tokio::test(flavor = "multi_thread")]
async fn help_is_grouped_and_takes_the_corpus_wording() {
    let mut r = rig().await;
    let help = r.run("/help").await;
    let at = |needle: &str| {
        help.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {help}"))
    };
    let order = [
        "Conversation",
        "Channels",
        "Memories",
        "Saves",
        "Settings",
        "Other",
    ];
    assert!(order.windows(2).all(|w| at(w[0]) < at(w[1])), "{help}");
    for cmd in [
        "/save switch",
        "/forget <n>",
        "/tier [level]",
        "/quiet",
        "/wrapup",
        "Enter: new line",
    ] {
        assert!(help.contains(cmd), "{cmd}");
    }
    assert!(!help.contains("Ctrl-J"));

    // The default names the host through the corpus's word for it.
    assert!(!help.contains("{host}") && help.contains("whether Hostess is available"));
    let mut worded = rig_wording(
        r#"{"host.name": "Hostess", "cli.help.memories": "MEMORY-GROUP for {host}", "cli.typing": "SEND-KEY"}"#,
    )
    .await;
    let help = worded.run("/help").await;
    assert!(
        help.contains("MEMORY-GROUP for Hostess") && help.contains("SEND-KEY"),
        "{help}"
    );
    assert!(help.contains("whether Hostess is available"), "{help}");
}

#[tokio::test(flavor = "multi_thread")]
async fn arrivals_run_the_tick_without_a_model_call_when_nothing_is_due() {
    let mut r = rig().await;
    // Nothing is idle or due: no endpoint is asked (the script is empty and would panic).
    r.session.arrivals(true).await;
    r.session.arrivals(false).await;
    assert!(r.out.take().is_empty() || !r.out.take().contains("error"));
}

#[tokio::test]
async fn tab_completes_persons_with_a_persona_and_follows_the_saves() {
    let mut r = rig().await;
    let names = |r: &Rig, line: &str| -> Vec<String> {
        r.completions
            .complete(line, line.len())
            .1
            .into_iter()
            .map(|p| p.display)
            .collect()
    };
    // Ann and Cy have personas, Bo has none; the alias reaches Ann.
    let all = names(&r, "/talk ");
    assert!(all.iter().any(|n| n == "Annie"), "{all:?}");
    assert!(
        !all.iter().any(|n| n.to_lowercase().starts_with("bo")),
        "{all:?}"
    );
    assert_eq!(names(&r, "/talk Ann").len(), 2, "display name and alias");
    // Any alias kind completes, but only for talkable persons.
    assert_eq!(names(&r, "/talk Lady"), ["Lady A"]);
    // A new save is offered at once, and one that was deleted no longer is.
    assert_eq!(names(&r, "/save switch sec"), Vec::<String>::new());
    r.run("/save new second").await;
    assert_eq!(names(&r, "/save switch sec"), ["second"]);
    // Switching keeps the lists current: the save just left is offered as well.
    r.run("/save switch second").await;
    assert_eq!(names(&r, "/save switch def"), ["default"]);
    assert_eq!(names(&r, "/talk Lady"), ["Lady A"]);
    r.run("/save switch default").await;
    r.run("/save delete second").await;
    r.run("y").await;
    assert_eq!(names(&r, "/save switch sec"), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn resend_after_a_failed_turn_takes_back_the_unanswered_message() {
    let mut r = rig().await;
    r.run("/talk ann").await;
    r.script
        .0
        .lock()
        .unwrap()
        .push_back(Reply::Fail("overloaded"));
    assert!(r.run("how is the tea?").await.contains("overloaded"));
    let out = r.run("/resend").await;
    assert!(out.contains("took back 1 message;"), "{out}");
    assert_eq!(r.session.take_prefill().as_deref(), Some("how is the tea?"));
    assert!(!r.run("/history 10").await.contains("how is the tea?"));
}

/// Passwords the session will be asked for, in order; each prompt is recorded.
fn answers(r: &mut Rig, typed: &[&str]) -> Arc<std::sync::Mutex<Vec<String>>> {
    let queue: std::collections::VecDeque<String> = typed.iter().map(|s| (*s).to_owned()).collect();
    let queue = std::sync::Mutex::new(queue);
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = asked.clone();
    r.session.set_secret(move |prompt| {
        log.lock().unwrap().push(prompt.to_owned());
        queue.lock().unwrap().pop_front()
    });
    asked
}

#[tokio::test(flavor = "multi_thread")]
async fn password_turns_encryption_on_changes_it_and_turns_it_off() {
    let mut r = rig().await;
    let file = r._home.0.join("saves/default.sqlite");
    assert!(r.run("/password").await.contains("not encrypted"));
    assert!(r.run("/password off").await.contains("not encrypted"));

    // The warning comes before the question, and a no changes nothing.
    let asked = answers(&mut r, &["one", "one"]);
    let ask = r.run("/password on").await;
    assert!(
        ask.contains("cannot be recovered") && ask.contains("(y/n)"),
        "{ask}"
    );
    assert!(r.run("n").await.contains("cancelled"));
    assert!(!world::cipher::is_encrypted(&file));
    r.run("/password on").await;
    assert!(r.run("y").await.contains("is encrypted now"));
    assert_eq!(*asked.lock().unwrap(), ["new password: ", "again: "]);
    assert!(world::cipher::is_encrypted(&file));
    assert!(r.run("/save").await.contains("encrypted"));
    assert!(r.run("/password").await.contains("is encrypted"));
    assert!(r.run("/password on").await.contains("already encrypted"));
    // Play goes on over the encrypted file.
    assert!(r.run("/name Dr. Doc").await.contains("Dr. Doc"));

    // Two different new passwords change nothing; a wrong current one is refused.
    answers(&mut r, &["one", "two", "three"]);
    assert!(r.run("/password change").await.contains("did not match"));
    answers(&mut r, &["bad", "two", "two"]);
    assert!(r.run("/password change").await.contains("wrong password"));
    let asked = answers(&mut r, &["one", "two", "two"]);
    assert!(r.run("/password change").await.contains("password changed"));
    assert_eq!(
        *asked.lock().unwrap(),
        ["current password: ", "new password: ", "again: "]
    );
    assert!(world::check_file(&file, Some("one")).is_err());
    world::check_file(&file, Some("two")).unwrap();

    answers(&mut r, &["nope"]);
    r.run("/password off").await;
    assert!(r.run("y").await.contains("wrong password"));
    answers(&mut r, &["two"]);
    r.run("/password off").await;
    assert!(r.run("y").await.contains("is not encrypted now"));
    assert!(!world::cipher::is_encrypted(&file));
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_to_an_encrypted_save_asks_for_its_password() {
    let mut r = rig().await;
    r.run("/save new vault").await;
    let vault = r._home.0.join("saves/vault.sqlite");
    world::cipher::encrypt(&vault, "key").unwrap();
    let listed = r.run("/save").await;
    assert!(
        listed
            .lines()
            .any(|l| l.contains("vault") && l.contains("encrypted")),
        "{listed}"
    );

    // Cancelling, then a wrong password three times, leave the current save as it was.
    let asked = answers(&mut r, &[]);
    assert!(r.run("/save switch vault").await.contains("cancelled"));
    answers(&mut r, &["a", "b", "c"]);
    let out = r.run("/save switch vault").await;
    assert_eq!(out.matches("wrong password").count(), 3, "{out}");
    assert!(r.run("/save").await.contains("* default"));
    assert_eq!(asked.lock().unwrap()[0], "password for vault: ");

    answers(&mut r, &["bad", "key"]);
    let out = r.run("/save switch vault").await;
    assert!(
        out.contains("wrong password") && out.contains("now playing vault"),
        "{out}"
    );
    assert!(r.run("/save").await.contains("* vault"));

    // Copying a save keeps its password; importing an encrypted file asks for it.
    assert!(r.run("/save copy vault twin").await.contains("copied"));
    assert!(world::cipher::is_encrypted(
        r._home.0.join("saves/twin.sqlite")
    ));
    let out = r._home.0.join("out.sqlite");
    r.run(&format!("/save export vault \"{}\"", out.display()))
        .await;
    answers(&mut r, &["wrong", "key"]);
    let imported = r
        .run(&format!("/save import \"{}\" copy2", out.display()))
        .await;
    assert!(
        imported.contains("wrong password") && imported.contains("imported as copy2"),
        "{imported}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bio_shows_sets_clears_and_is_bounded() {
    let mut r = rig().await;
    assert!(r.run("/bio").await.contains("no bio"));
    assert!(
        r.run("/bio Runs the night shift.")
            .await
            .contains("bio saved")
    );
    assert!(r.run("/bio").await.contains("bio: Runs the night shift."));
    let long = "x".repeat(world::settings::BIO_MAX + 1);
    assert!(
        r.run(&format!("/bio {long}"))
            .await
            .contains("at most 200 characters")
    );
    assert!(r.run("/bio clear").await.contains("bio cleared"));
    assert!(r.run("/bio").await.contains("no bio"));
}

// ---- first run: the save picker and the setup steps ----

#[tokio::test(flavor = "multi_thread")]
async fn without_a_save_the_picker_leads_and_a_choice_opens_and_records_one() {
    let mut r = rig_full(WORDING, false, false, None).await;
    assert!(r.session.picking());
    r.session.show_picker().await;
    let shown = r.out.take();
    assert!(
        shown.contains("(none yet)") && shown.contains("\"new <name>\""),
        "{shown}"
    );

    // Nothing but a choice makes sense yet.
    assert!(r.run("hello").await.contains("No save is open"));
    assert!(r.run("/talk ann").await.contains("No save is open"));
    assert!(r.run("new a/b").await.contains("not a usable name"));
    assert!(r.run("3").await.contains("no save with that number"));
    assert!(r.session.picking());

    let opened = r.run("new first").await;
    assert!(opened.contains("now playing first"), "{opened}");
    assert!(!r.session.picking());
    let home = Home::new(&r._home.0);
    assert_eq!(home.current_save().as_deref(), Some("first"));
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_clears_the_record_and_returns_to_a_numbered_list_both_clients_read() {
    let mut r = rig().await;
    r.run("/save new second").await;
    let home = Home::new(&r._home.0);
    assert_eq!(home.current_save().as_deref(), Some("default"));

    let out = r.run("/save logout").await;
    assert!(
        out.contains("logged out") && out.contains("Saves in"),
        "{out}"
    );
    assert!(
        out.contains(" 1  ") && out.contains("default") && out.contains("second"),
        "{out}"
    );
    assert!(r.session.picking());
    assert_eq!(home.current_save(), None);
    // The app, starting now, would show its picker too.
    assert_eq!(home.start_save(), api::home::Start::Pick { missing: None });

    // Pick by number (the list is most recently played first) or by name.
    let names: Vec<String> = r
        .engine
        .saves()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    let n = names.iter().position(|s| s == "second").unwrap() + 1;
    let out = r.run(&n.to_string()).await;
    assert!(out.contains("now playing second"), "{out}");
    assert!(!r.session.picking());
    assert_eq!(home.current_save().as_deref(), Some("second"));
    // The shorthand does the same.
    assert!(r.run("/logout").await.contains("logged out"));
    assert!(r.run("quit").await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recorded_save_that_is_gone_is_named_in_the_picker() {
    let mut r = rig_full(WORDING, false, false, Some("vanished")).await;
    assert!(r.session.picking());
    r.session.show_picker().await;
    let shown = r.out.take();
    assert!(
        shown.contains("\"vanished\", is not there any more"),
        "{shown}"
    );
}

/// Answers from a list, and keeps what was said.
struct Script {
    answers: VecDeque<Option<String>>,
    secrets: VecDeque<Option<String>>,
    said: Vec<String>,
}

impl Script {
    fn new(answers: &[&str], secrets: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| Some((*a).to_owned())).collect(),
            secrets: secrets.iter().map(|a| Some((*a).to_owned())).collect(),
            said: Vec::new(),
        }
    }
}

impl super::setup::Ask for Script {
    fn line(&mut self, prompt: &str) -> Option<String> {
        self.said.push(prompt.to_owned());
        self.answers.pop_front().flatten()
    }
    fn secret(&mut self, _: &str) -> Option<String> {
        self.secrets.pop_front().flatten()
    }
    fn say(&mut self, text: &str) {
        self.said.push(text.to_owned());
    }
}

#[test]
fn the_corpus_step_explains_a_bad_path_and_remembers_a_good_one() {
    let home = TempHome::new();
    let h = Home::new(&home.0);
    h.create().unwrap();
    let junk = home.0.join("junk.folio");
    std::fs::write(&junk, b"not a database").unwrap();
    let good = corpus(WORDING, false);
    let mut ask = Script::new(
        &[
            "/no/such.folio",
            &junk.display().to_string(),
            &format!("\"{}\"", good.path().display()),
        ],
        &[],
    );
    let chosen = super::setup::corpus(&h, &mut ask).unwrap();
    assert_eq!(chosen, Some(good.path().canonicalize().unwrap()));
    let said = ask.said.join("\n");
    assert!(said.contains("there is no file there"), "{said}");
    assert!(
        said.contains("not a corpus this program can read"),
        "{said}"
    );
    assert_eq!(h.chosen_corpus(), chosen);

    // Empty answer: stop, nothing remembered.
    let other = TempHome::new();
    let oh = Home::new(&other.0);
    oh.create().unwrap();
    assert_eq!(
        super::setup::corpus(&oh, &mut Script::new(&[""], &[])).unwrap(),
        None
    );
    assert_eq!(oh.chosen_corpus(), None);
}

#[tokio::test]
async fn the_endpoints_wizard_writes_a_valid_file_and_the_key_goes_to_the_fake_keychain() {
    let home = TempHome::new();
    let h = Home::new(&home.0);
    // The library's default ports keep keys in memory: the real keychain is not touched.
    let engine = Engine::open(h.clone(), None, api::Ports::default()).unwrap();
    assert_eq!(
        super::setup::gap(&engine).await,
        super::setup::Gap::Endpoint
    );

    let mut ask = Script::new(
        &[
            "y",
            "",
            "mine",
            "not a url",
            "",
            "model-a",
            "",
            "mine",
            "https://api.example.invalid/v1",
            "chat",
            "model-a",
            "model-h",
        ],
        &["sk-secret", "sk-secret"],
    );
    // First pass: "not a url" is refused with a plain reason, then the form runs again.
    let names = Names::load(&engine).await.unwrap();
    assert!(super::setup::endpoints(&engine, &names, super::setup::Gap::Endpoint, &mut ask).await);
    let said = ask.said.join("\n");
    assert!(said.contains("must start with http"), "{said}");
    let text = std::fs::read_to_string(h.endpoints()).unwrap();
    assert!(!text.contains("sk-secret"));
    let parsed = api::endpoints::Endpoints::from_toml(&text).unwrap();
    parsed.validate().unwrap();
    let roles = parsed.roles.unwrap();
    assert_eq!(
        (
            roles.chat.model.as_str(),
            roles.host.unwrap().model.as_str()
        ),
        ("model-a", "model-h")
    );
    assert!(engine.endpoints().await.unwrap().profiles[0].has_key);
    assert_eq!(super::setup::gap(&engine).await, super::setup::Gap::None);

    // Declining changes nothing.
    let fresh = TempHome::new();
    let e2 = Engine::open(Home::new(&fresh.0), None, api::Ports::default()).unwrap();
    let n2 = Names::load(&e2).await.unwrap();
    assert!(
        !super::setup::endpoints(
            &e2,
            &n2,
            super::setup::Gap::Endpoint,
            &mut Script::new(&["n"], &[])
        )
        .await
    );
    assert!(!Home::new(&fresh.0).endpoints().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn history_marks_pictures_and_storage_clears_only_caches() {
    let mut r = rig().await;
    r.run("/talk ann").await;
    let ch = r
        .engine
        .open_direct(api::types::PersonId("ann".into()))
        .await
        .unwrap();
    {
        let ch = world::ChannelId(ch.0.parse().unwrap());
        let w = world::World::open_with(r.engine.home().world().unwrap(), None).unwrap();
        let hash = world::picture::put(&w, "image/png", b"\x89PNG\r\n\x1a\nx").unwrap();
        for (text, meta) in [
            ("with a picture", serde_json::json!({ "picture": hash })),
            ("without one", serde_json::json!({ "picture_lost": true })),
        ] {
            world::message::append(&w, ch, &world::Actor::User, text, None, Some(&meta), 5)
                .unwrap();
        }
    }
    let shown = r.run("/history 5").await;
    assert!(shown.contains("[picture]"), "{shown}");
    assert!(shown.contains("[picture unavailable]"), "{shown}");

    let home = r.engine.home().clone();
    std::fs::create_dir_all(home.tts()).unwrap();
    std::fs::write(home.tts().join("a.mp3"), [0u8; 2048]).unwrap();
    let usage = r.run("/storage").await;
    assert!(usage.contains("audio") && usage.contains("2 KB"), "{usage}");
    assert!(usage.contains("pictures in this save: 1"), "{usage}");

    // A no keeps the cache; a yes empties it and nothing else.
    assert!(r.run("/storage clear audio").await.contains("(y/n)"));
    assert!(r.run("n").await.contains("cancelled"));
    assert!(home.tts().join("a.mp3").exists());
    r.run("/storage clear audio").await;
    assert!(r.run("y").await.contains("cleared"));
    assert!(!home.tts().join("a.mp3").exists());
    assert!(home.world().unwrap().is_file());
    assert!(r.run("/storage clear saves").await.contains("usage:"));
}

async fn unread_of(r: &Rig, who: &str) -> u32 {
    let list = r.engine.channels().await.unwrap();
    list.iter()
        .find(|c| c.participants.iter().any(|p| p.id.0 == who))
        .map_or(0, |c| c.unread)
}

#[tokio::test(flavor = "multi_thread")]
async fn what_the_terminal_prints_counts_as_read_in_the_app() {
    let mut r = rig().await;
    r.run("/talk ann").await;
    r.script
        .0
        .lock()
        .unwrap()
        .push_back(Reply::Say("Hello, Doc."));
    assert!(r.run("hi").await.contains("Hello, Doc."));
    assert_eq!(unread_of(&r, "ann").await, 0);

    // A message written while nobody was looking is unread until the channel is entered,
    // and entering it prints the message.
    let ch = r
        .engine
        .open_direct(api::types::PersonId("ann".into()))
        .await
        .unwrap();
    {
        // Another connection to the open save, as the app would be.
        let home = Home::new(&r._home.0);
        let w = world::World::open(home.world().unwrap()).unwrap();
        let who = world::Actor::Person("ann".into());
        let ch = world::ChannelId(ch.0.parse().unwrap());
        world::message::append(&w, ch, &who, "You were away.", None, None, 9_000).unwrap();
    }
    assert_eq!(unread_of(&r, "ann").await, 1);
    r.run("/host").await;
    let out = r.run("/talk ann").await;
    assert!(
        out.contains("1 unread:") && out.contains("You were away."),
        "{out}"
    );
    assert_eq!(unread_of(&r, "ann").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn conversations_are_listed_opened_archived_and_titled() {
    let mut r = rig().await;
    assert!(r.run("/chats").await.contains("no conversations yet"));
    r.run("/talk ann").await;
    r.script.0.lock().unwrap().push_back(Reply::Say("Hi."));
    r.run("hello").await;
    let listed = r.run("/chats").await;
    assert!(
        listed.contains("  1  ann") && listed.contains("direct"),
        "{listed}"
    );
    assert!(r.run("/chat 9").await.contains("no such conversation"));
    r.run("/talk cy").await;
    assert!(r.run("/chat 1").await.contains("talking to ann"));

    assert!(r.run("/rename Tea hour").await.contains("renamed"));
    assert!(r.run("/chats").await.contains("Tea hour"));
    assert!(r.run("/rename").await.contains("derived"));
    assert!(r.run("/archive").await.contains("archived"));
    assert!(!r.run("/chats").await.contains("ann"));
    assert!(r.run("/chats archived").await.contains("ann"));
    // Opening an archived conversation says so and names the way out; it stays archived.
    r.run("/talk cy").await;
    let opened = r.run("/chats archived").await;
    assert!(opened.contains("ann"), "{opened}");
    let opened = r.run("/chat 1").await;
    assert!(
        opened.contains("talking to ann") && opened.contains("archived; /unarchive"),
        "{opened}"
    );
    assert!(r.run("/chats archived").await.contains("ann"));
    assert!(r.run("/unarchive").await.contains("main list"));
    assert!(r.run("/read").await.contains("marked read: 1 conversation"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_can_be_listed_and_deleted_after_asking() {
    let mut r = rig().await;
    r.run("/group ann cy").await;
    assert!(r.run("/members").await.contains("ann, cy"));
    assert!(r.run("/leave").await.contains("(y/n)"));
    assert!(r.run("n").await.contains("cancelled"));
    r.run("/leave").await;
    assert!(r.run("y").await.contains("group deleted"));
    assert!(r.session.prompt().starts_with("[-]"));
}

#[tokio::test(flavor = "multi_thread")]
async fn people_have_a_profile_a_mode_and_requests_are_listed() {
    let mut r = rig().await;
    let who = r.run("/who bo").await;
    assert!(who.contains("bo") && who.contains("no persona"), "{who}");
    assert!(
        r.run("/mode ann enabled")
            .await
            .contains("ann is enabled now")
    );
    assert!(r.run("/who ann").await.contains("mode: enabled"));
    assert!(r.run("/mode ann frozen").await.contains("frozen now"));
    r.run("/group ann cy").await;
    assert!(r.run("/mode disabled").await.contains("disabled now"));
    assert!(
        r.run("/mode nobody frozen")
            .await
            .contains("nobody matches")
    );
    assert!(r.run("/requests").await.contains("no requests yet"));
    assert!(r.run("/waiting").await.contains("nobody is waiting"));
    assert!(r.run("/birthday").await.contains("no birthday"));
    assert!(r.run("/birthday 03-14").await.contains("saved"));
    assert!(r.run("/birthday").await.contains("03-14"));
    assert!(r.run("/birthday 13-40").await.contains("not a date"));
    assert!(r.run("/birthday clear").await.contains("cleared"));
    let clock = r.run("/clock").await;
    assert!(clock.trim().len() >= 16, "{clock:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_endpoint_the_session_is_a_library_and_never_calls_a_model() {
    let mut r = rig_setup(WORDING, false, true, None, false).await;
    r.session.set_no_model(true);
    // The notice is one line; the prompt names the mode.
    let notice = r.session.library_notice(false).unwrap();
    assert!(
        notice.contains("library mode") && !notice.contains('\n'),
        "{notice}"
    );
    assert!(r.session.prompt().starts_with("[library] Doc"));

    // The roster lists everyone, with no word about the host speaking.
    let listed = r.run("/list").await;
    assert!(listed.contains("ann") && listed.contains("bo "), "{listed}");
    assert!(r.run("/who ann").await.contains("ann"));

    // Search needs no conversation; results are numbered and open with /read.
    let found = r.run("/search jasmine tea").await;
    assert!(
        found.contains("1. Tea page") && found.contains("/read <n>"),
        "{found}"
    );
    let read = r.run("/read 1").await;
    assert!(read.contains("Tea page (u1)"), "{read}");
    assert!(
        read.contains("Ann said the jasmine tea needs cooler water"),
        "{read}"
    );
    assert!(r.run("/read u2").await.contains("Bo hammered the blade"));
    assert!(r.run("/read nope").await.contains("no unit \"nope\""));
    assert!(r.run("/read 9").await.contains("no such result"));
    assert!(r.run("/memories").await.contains("no memories"));
    assert!(r.run("/save").await.contains("default"));

    // Talking, asking and the like say what they need; the script would panic on a call.
    for line in ["hello", "/ask ann tea?", "/wrapup"] {
        let out = r.run(line).await;
        assert!(out.contains("needs a language model"), "{line}: {out}");
    }
    // Nothing to go back to without an endpoint.
    assert!(r.run("/library off").await.contains("stays on"));
    assert!(r.run("/stats").await.contains("no model calls yet"));
}

#[tokio::test(flavor = "multi_thread")]
async fn library_mode_can_be_chosen_and_left_when_a_model_is_set_up() {
    let mut r = rig().await;
    assert!(r.session.library_notice(false).is_none());
    assert!(r.run("/talk ann").await.contains("talking to ann"));
    assert!(r.run("/library").await.contains("nothing here calls"));
    assert!(r.session.prompt().contains("[ann]"));
    let out = r.run("hello").await;
    assert!(out.contains("library mode is on"), "{out}");
    // Opening a unit works in a conversation too; the script is still untouched.
    assert!(r.run("/read u1").await.contains("jasmine"));
    assert!(
        r.run("/library off")
            .await
            .contains("back to conversations")
    );
    r.script
        .0
        .lock()
        .unwrap()
        .push_back(Reply::Say("Hello again."));
    assert!(r.run("hello").await.contains("Hello again."));
}

#[tokio::test(flavor = "multi_thread")]
async fn events_are_listed_and_switched() {
    let mut r = rig().await;
    let out = r.run("/events").await;
    // The rig's register has them off until the user asks.
    assert!(
        out.contains("ship events off") && out.contains("nothing has happened"),
        "{out}"
    );
    assert!(r.run("/events 2").await.contains("2 a day"));
    assert!(r.run("/events off").await.contains("ship events off"));
    assert!(r.run("/events").await.contains("ship events off"));
    assert!(r.run("/events on").await.contains("1 a day"));
    assert!(r.run("/events 3").await.contains("usage"));
    assert!(r.run("/events many").await.contains("usage"));
}
