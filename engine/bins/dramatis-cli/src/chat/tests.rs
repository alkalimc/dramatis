//! The session end to end over a scripted endpoint: no network, no real key.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

use agent::endpoint::StaticKeys;
use agent::{Agent, Config, Parts, SystemClock};
use async_trait::async_trait;
use folio::fixture::{Builder, TempFolio, UnitSpec};
use futures::StreamExt;
use llm::transport::{HttpRequest, HttpResponse, StreamResponse};
use llm::{Llm, Retry, Transport};
use serde_json::json;

use super::command::parse;
use super::render::{Marker, Style};
use super::session::Session;
use super::show::{Show, Term};

enum Reply {
    Say(&'static str),
    Fail(&'static str),
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
            Reply::Fail(message) => sse(&[json!({"error": {"message": message}})]),
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

fn corpus() -> TempFolio {
    let mut b = Builder::new()
        .manifest("user_placeholder", r#""<U>""#)
        .manifest("wording", r#"{"user.you": "Doc", "host.name": "Hostess"}"#)
        .manifest("scene_marker", r#"["(", ")"]"#);
    b.person("ann", None).person("bo", None);
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
    for (slot, body) in [
        ("system", "You are ann."),
        ("tone", "Brief."),
        ("capability", "Knows tea."),
    ] {
        b.conn()
            .execute(
                "INSERT INTO prompts(subject, slot, body) VALUES ('ann', ?1, ?2)",
                [slot, body],
            )
            .unwrap();
    }
    b.finish()
}

struct Rig {
    session: Session,
    out: Buf,
    script: Arc<Scripted>,
    _folio: TempFolio,
}

fn rig() -> Rig {
    let folio = corpus();
    let index = index::Index::open(folio.path(), index::params::Retrieve::default()).unwrap();
    let marker = Marker::new(&index.folio().manifest().scene_marker);
    let out = Buf::default();
    let term = Arc::new(Term::new(Box::new(out.clone()), Style::PLAIN));
    let show = Arc::new(Show::new(term.clone(), marker));
    let script = Arc::new(Scripted::default());
    let endpoints = api::endpoints::Endpoints::from_toml(
        "[[profile]]\nname = 'p'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = 'chat'\n\
         [[profile.model]]\nid = 'model-a'\n\
         [roles]\nchat = { profile = 'p', model = 'model-a' }\n",
    )
    .unwrap();
    let agent = Arc::new(
        Agent::new(Parts {
            world: world::World::in_memory().unwrap(),
            index,
            config: Config::default(),
            endpoints,
            keys: Some(Arc::new(StaticKeys(vec![(
                "p".into(),
                llm::Secret::new("sk-test"),
            )]))),
            llm: Llm::new(script.clone(), Retry::none()),
            sink: show.clone(),
            clock: Arc::new(SystemClock { offset_min: 0 }),
        })
        .unwrap(),
    );
    show.bind(&agent);
    Rig {
        session: Session::new(agent, term, show),
        out,
        script,
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
    let mut r = rig();
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
    assert!(first.contains("sources: Tea page"), "{first:?}");
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

    let history = r.run("/history 2").await;
    assert!(history.contains("ann: Better now."), "{history:?}");
    let stats = r.run("/stats").await;
    assert!(
        stats.contains("direct") && stats.contains("model-a") && stats.contains("total"),
        "{stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn search_runs_retrieval_as_the_current_speaker_without_a_model_call() {
    let mut r = rig();
    r.run("/talk ann").await;
    let found = r.run("/search jasmine tea").await;
    assert!(
        found.contains("as ann") && found.contains("Tea page"),
        "{found}"
    );
    assert!(r.run("/stats").await.contains("no model calls yet"));
    assert!(r.run("/memories").await.contains("no memories"));
    let group = r.run("/group ann bo").await;
    assert!(group.contains("has no persona"), "{group}");
    assert!(r.run("/host").await.contains("no host"));
}
