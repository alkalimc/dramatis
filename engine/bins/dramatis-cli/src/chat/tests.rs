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

fn corpus(host: bool) -> TempFolio {
    let mut b = Builder::new()
        .manifest("user_placeholder", r#""<U>""#)
        .manifest("wording", r#"{"user.you": "Doc", "host.name": "Hostess"}"#)
        .manifest("scene_marker", r#"["(", ")"]"#);
    b.person("ann", None).person("bo", None).person("cy", None);
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
    show: Arc<Show>,
    engine: Arc<Engine>,
    names: Arc<Names>,
    bridge: Arc<runtime::AgentBridge>,
    out: Buf,
    script: Arc<Scripted>,
    _home: TempHome,
    _folio: TempFolio,
}

async fn rig() -> Rig {
    rig_with(false).await
}

async fn rig_with(host: bool) -> Rig {
    let folio = corpus(host);
    let home = TempHome::new();
    std::fs::write(
        home.0.join("endpoints.toml"),
        "[[profile]]\nname = 'p'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = 'chat'\n\
         [[profile.model]]\nid = 'model-a'\n\
         [roles]\nchat = { profile = 'p', model = 'model-a' }\n",
    )
    .unwrap();
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
        },
    )
    .unwrap();
    let engine = runtime.engine;
    let names = Arc::new(Names::load(&engine).await.unwrap());
    let scene = engine.wording().await.unwrap().scene_marker;
    show.bind(names.clone(), Marker::new(&(scene.open, scene.close)));
    let bridge = runtime.bridge.unwrap();
    Rig {
        session: Session::new(
            engine.clone(),
            bridge.clone(),
            term,
            show.clone(),
            names.clone(),
        )
        .await,
        show,
        engine,
        names,
        bridge,
        out,
        script,
        _home: home,
        _folio: folio,
    }
}

impl Rig {
    async fn run(&mut self, line: &str) -> String {
        self.session.handle(parse(line)).await;
        self.out.take()
    }
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
        unfolded.contains("sources (1):\n  Tea page"),
        "{unfolded:?}"
    );
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
    };
    let group = r
        .engine
        .create_group(vec![PersonId("ann".into()), PersonId("cy".into())], None)
        .await
        .unwrap();
    assert!(r.run("/go 1").await.contains("offered nothing"));

    // The host's tool calls become a numbered list after its reply.
    r.show.emit(host_called(ToolAction::FindPeople {
        topic: "tea".into(),
        persons: vec![person("ann")],
    }));
    r.show.emit(host_called(ToolAction::CreateGroup {
        members: vec![person("ann"), person("cy")],
        channel: Some(group),
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
async fn the_hosts_channel_list_becomes_choices_that_switch_to_existing_conversations() {
    let mut r = rig_with(true).await;
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
/// Guide commands the parser gains only when the saves and memory-editing commands land;
/// drop each name from here as it does.
const NOT_PARSED_YET: [&str; 5] = ["forget", "edit", "restore", "tier", "save"];

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
    for c in GUIDE_COMMANDS
        .iter()
        .filter(|c| !NOT_PARSED_YET.contains(c))
    {
        let unknown = matches!(parse(&format!("/{c}")), Command::Invalid(m) if m.starts_with("unknown command"));
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
