//! A scripted endpoint and a small corpus with personas, shared by the agent tests.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use agent::endpoint::StaticKeys;
use agent::{Agent, Clock, Config, Event, Parts, Sink};
use api::endpoints::{Endpoints, WireApi};
use async_trait::async_trait;
use folio::fixture::{Builder, TempFolio, UnitSpec};
use futures::StreamExt;
use llm::transport::{HttpRequest, HttpResponse, StreamResponse};
use llm::{Llm, Retry, Transport};
use serde_json::{Value, json};
use world::clock::Now;
use world::{ChannelId, World};

/// The three protocols every prefix test runs on.
pub const WIRES: [WireApi; 3] = [WireApi::Chat, WireApi::Responses, WireApi::Messages];

/// One scripted reply.
#[derive(Debug, Clone)]
pub enum Reply {
    Say(String),
    Call(String, Value),
    /// A recorded SSE body, replayed as is.
    Raw(String),
    Fail(String),
}

pub fn say(t: &str) -> Reply {
    Reply::Say(t.into())
}

pub fn call(name: &str, args: Value) -> Reply {
    Reply::Call(name.into(), args)
}

/// Records every body and answers from the script, in the protocol the request was
/// rendered for. Usage is simulated the way a prefix cache behaves: the bytes a body
/// shares with the previous one under the same cache key count as cached input, four
/// bytes to a token.
#[derive(Default)]
pub struct Fake {
    script: Mutex<VecDeque<Reply>>,
    pub bodies: Mutex<Vec<Vec<u8>>>,
    /// Calls so far, to number the signatures of reasoning blocks.
    n: AtomicI64,
}

pub fn tokens(bytes: usize) -> u64 {
    bytes.div_ceil(4) as u64
}

fn common(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn cache_key(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    v["prompt_cache_key"].as_str().map(str::to_owned)
}

impl Fake {
    pub fn push(&self, replies: impl IntoIterator<Item = Reply>) {
        self.script.lock().unwrap().extend(replies);
    }

    pub fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().unwrap().clone()
    }

    pub fn json(&self) -> Vec<Value> {
        self.bodies()
            .iter()
            .map(|b| serde_json::from_slice(b).unwrap())
            .collect()
    }

    pub fn left(&self) -> usize {
        self.script.lock().unwrap().len()
    }

    fn usage(&self, body: &[u8], output: usize) -> (u64, u64, u64) {
        let bodies = self.bodies.lock().unwrap();
        let key = cache_key(body);
        let prev = bodies
            .iter()
            .rev()
            .find(|b| cache_key(b) == key && b.as_slice() != body);
        let cached = prev.map(|p| tokens(common(p, body))).unwrap_or(0);
        let input = tokens(body.len());
        (input, cached.min(input), tokens(output) + 3)
    }
}

pub fn sse(events: &[Value], done: bool) -> String {
    let mut s = String::new();
    for e in events {
        s.push_str("data: ");
        s.push_str(&e.to_string());
        s.push_str("\n\n");
    }
    if done {
        s.push_str("data: [DONE]\n\n");
    }
    s
}

fn halves(t: &str) -> [&str; 2] {
    let mid = t
        .char_indices()
        .nth(t.chars().count() / 2)
        .map_or(0, |(i, _)| i);
    [&t[..mid], &t[mid..]]
}

fn render(wire: WireApi, reply: &Reply, usage: (u64, u64, u64), n: i64) -> String {
    let (input, cached, output) = usage;
    match (wire, reply) {
        (_, Reply::Raw(s)) => s.clone(),
        (WireApi::Messages, Reply::Fail(d)) => sse(
            &[json!({"type": "error", "error": {"type": "overloaded_error", "message": d}})],
            false,
        ),
        (WireApi::Responses, Reply::Fail(d)) => sse(
            &[json!({"type": "response.failed", "response": {"error": {"message": d}}})],
            false,
        ),
        (WireApi::Chat, Reply::Fail(d)) => sse(&[json!({"error": {"message": d}})], false),
        (WireApi::Messages, r) => {
            // Thinking comes first and carries a signature; the cache is written for what
            // the prefix has not read.
            let fresh = 3.min(input - cached);
            let written = input - cached - fresh;
            let mut ev = vec![
                json!({"type": "message_start", "message": {"id": "msg", "role": "assistant",
                    "content": [], "usage": {"input_tokens": fresh,
                    "cache_read_input_tokens": cached, "cache_creation_input_tokens": written,
                    "output_tokens": 1}}}),
                json!({"type": "content_block_start", "index": 0,
                    "content_block": {"type": "thinking", "thinking": ""}}),
                json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "thinking_delta", "thinking": "hm"}}),
                json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "signature_delta", "signature": format!("sig-{n}")}}),
                json!({"type": "content_block_stop", "index": 0}),
            ];
            let stop = match r {
                Reply::Say(t) => {
                    ev.push(json!({"type": "content_block_start", "index": 1,
                        "content_block": {"type": "text", "text": ""}}));
                    for part in halves(t) {
                        ev.push(json!({"type": "content_block_delta", "index": 1,
                            "delta": {"type": "text_delta", "text": part}}));
                    }
                    ev.push(json!({"type": "content_block_stop", "index": 1}));
                    "end_turn"
                }
                Reply::Call(name, args) => {
                    let args = args.to_string();
                    let [a, b] = halves(&args);
                    ev.push(json!({"type": "content_block_start", "index": 1,
                        "content_block": {"type": "tool_use", "id": format!("call_{name}"),
                                          "name": name, "input": {}}}));
                    for part in [a, b] {
                        ev.push(json!({"type": "content_block_delta", "index": 1,
                            "delta": {"type": "input_json_delta", "partial_json": part}}));
                    }
                    ev.push(json!({"type": "content_block_stop", "index": 1}));
                    "tool_use"
                }
                _ => unreachable!(),
            };
            ev.push(
                json!({"type": "message_delta", "delta": {"stop_reason": stop},
                "usage": {"output_tokens": output}}),
            );
            ev.push(json!({"type": "message_stop"}));
            let mut s = String::new();
            for e in &ev {
                s.push_str(&format!(
                    "event: {}\ndata: {e}\n\n",
                    e["type"].as_str().unwrap()
                ));
            }
            s
        }
        (WireApi::Responses, r) => {
            let mut ev = vec![json!({"type": "response.output_item.done", "item":
                {"type": "reasoning", "id": format!("rs_{n}"), "summary": [],
                 "encrypted_content": format!("enc-{n}")}})];
            match r {
                Reply::Say(t) => {
                    for part in halves(t) {
                        ev.push(json!({"type": "response.output_text.delta", "delta": part}));
                    }
                    ev.push(json!({"type": "response.output_item.done", "item":
                        {"type": "message", "role": "assistant",
                         "content": [{"type": "output_text", "text": t}]}}));
                }
                Reply::Call(name, args) => ev.push(json!({
                    "type": "response.output_item.done",
                    "item": {"type": "function_call", "call_id": format!("call_{name}"),
                             "name": name, "arguments": args.to_string()},
                })),
                _ => unreachable!(),
            }
            ev.push(json!({"type": "response.completed", "response": {"usage": {
                "input_tokens": input, "input_tokens_details": {"cached_tokens": cached},
                "output_tokens": output, "output_tokens_details": {"reasoning_tokens": 2},
            }}}));
            sse(&ev, false)
        }
        (WireApi::Chat, r) => {
            let mut ev = Vec::new();
            let chunk = |delta: Value| json!({"choices": [{"index": 0, "delta": delta}]});
            match r {
                Reply::Say(t) => {
                    for part in halves(t) {
                        ev.push(chunk(json!({"content": part})));
                    }
                }
                Reply::Call(name, args) => {
                    ev.push(chunk(
                        json!({"tool_calls": [{"index": 0, "id": format!("call_{name}"),
                        "function": {"name": name, "arguments": ""}}]}),
                    ));
                    ev.push(chunk(json!({"tool_calls": [{"index": 0,
                        "function": {"arguments": args.to_string()}}]})));
                }
                _ => unreachable!(),
            }
            ev.push(json!({"choices": [], "usage": {"prompt_tokens": input,
                "completion_tokens": output, "prompt_tokens_details": {"cached_tokens": cached}}}));
            sse(&ev, true)
        }
    }
}

#[async_trait]
impl Transport for Fake {
    async fn send(&self, req: HttpRequest) -> llm::Result<HttpResponse> {
        assert!(
            req.url.ends_with("/models"),
            "only the model list is a plain call"
        );
        Ok(HttpResponse {
            status: 200,
            body: json!({"data": [{"id": "model-a"}]})
                .to_string()
                .into_bytes(),
        })
    }

    async fn open(&self, req: HttpRequest) -> llm::Result<StreamResponse> {
        let wire = if req.url.ends_with("/chat/completions") {
            WireApi::Chat
        } else if req.url.ends_with("/responses") {
            WireApi::Responses
        } else {
            assert!(req.url.ends_with("/messages"), "{}", req.url);
            WireApi::Messages
        };
        // The key goes in a header, never in the body.
        assert!(!String::from_utf8_lossy(&req.body).contains("sk-test"));
        let reply = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script has no more replies");
        let out = match &reply {
            Reply::Say(t) => t.len(),
            Reply::Call(_, a) => a.to_string().len(),
            _ => 0,
        };
        let usage = self.usage(&req.body, out);
        self.bodies.lock().unwrap().push(req.body);
        let n = self.n.fetch_add(1, Ordering::SeqCst);
        let text = render(wire, &reply, usage, n);
        // Cut mid-event, to exercise events split across chunks.
        let chunks: Vec<llm::Result<Vec<u8>>> =
            text.as_bytes().chunks(97).map(|c| Ok(c.to_vec())).collect();
        Ok(StreamResponse {
            status: 200,
            chunks: futures::stream::iter(chunks).boxed(),
        })
    }
}

#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<Event>>);

impl Sink for Recorder {
    fn emit(&self, e: Event) {
        self.0.lock().unwrap().push(e);
    }
}

impl Recorder {
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

pub struct TestClock(pub AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> Now {
        Now::utc(self.0.load(Ordering::SeqCst))
    }
}

impl TestClock {
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

/// 2026-03-10 12:00 UTC.
pub const T0: i64 = 1_773_144_000_000;
pub const META: &str = "META-LAYER-ONLY-FOR-THE-HOST";
pub const IN_WORLD: &str = "In-world layer of the host.";

/// Three people: `ann` and `bo` have personas, `cy` has none. The placeholder is `<U>`
/// and the corpus's word for "you" is `Doc`.
pub fn corpus() -> TempFolio {
    let mut b = Builder::new()
        .manifest("user_placeholder", r#""<U>""#)
        .manifest(
            "wording",
            r#"{"user.you": "Doc", "host.name": "Hostess", "tool.search": "Search the archive."}"#,
        )
        .manifest("scene_marker", r#"["(", ")"]"#);
    b.person("ann", Some("03-10"))
        .person("bo", None)
        .person("cy", None);
    let units: &[(&str, &str, &str, &[&str])] = &[
        (
            "u1",
            "tea",
            "Dr.<U> poured the jasmine tea for everyone",
            &["ann"],
        ),
        (
            "u2",
            "tea",
            "Ann said the jasmine tea needs cooler water",
            &["ann"],
        ),
        (
            "u3",
            "forge",
            "Bo hammered the blade at the forge until dawn",
            &["bo"],
        ),
        (
            "u4",
            "forge",
            "Bo said the forge blade steel came from the north",
            &["bo"],
        ),
        (
            "u5",
            "tower",
            "Cy kept watch on the tower over the river",
            &["cy"],
        ),
        (
            "u6",
            "lore",
            "The river city keeps its archive in the old tower",
            &[],
        ),
    ];
    for (id, page, text, persons) in units {
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
    let prompt = |subject: &str, slot: &str, body: &str| {
        b.conn()
            .execute(
                "INSERT INTO prompts(subject, slot, body) VALUES (?1, ?2, ?3)",
                [subject, slot, body],
            )
            .unwrap();
    };
    for p in ["ann", "bo"] {
        prompt(
            p,
            "system",
            &format!("You are {p}. You call the user Dr.<U>."),
        );
        prompt(p, "tone", &format!("{p} speaks briefly."));
        prompt(p, "fallback", "Not sure, Dr.<U>.");
        prompt(p, "capability", &format!("{p} knows things."));
    }
    // Half a persona is no persona.
    prompt("cy", "system", "You are cy.");
    prompt("host", "in_world", IN_WORLD);
    prompt("host", "meta", META);
    b.finish()
}

pub fn endpoints(wire: WireApi, forced_tool: bool, window: Option<u32>) -> Endpoints {
    let wire = match wire {
        WireApi::Chat => "chat",
        WireApi::Responses => "responses",
        WireApi::Messages => "messages",
    };
    let window = window.map_or(String::new(), |w| format!("context_window = {w}\n"));
    Endpoints::from_toml(&format!(
        "[[profile]]\nname = 'p'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = '{wire}'\n\
         [[profile.model]]\nid = 'model-a'\nforced_tool = {forced_tool}\n{window}\
         [roles]\nchat = {{ profile = 'p', model = 'model-a' }}\n"
    ))
    .unwrap()
}

pub struct Env {
    pub agent: Arc<Agent>,
    pub fake: Arc<Fake>,
    pub sink: Arc<Recorder>,
    pub clock: Arc<TestClock>,
    pub _folio: TempFolio,
    pub _office: tempdir::Dir,
}

pub mod tempdir {
    pub struct Dir(pub std::path::PathBuf);

    impl Dir {
        pub fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "agent-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Retrieval bands that the tiny corpus can reach: any match is high.
pub fn retrieve() -> index::params::Retrieve {
    let mut r = index::params::Retrieve::default();
    r.confidence.low_top1 = 0.01;
    r.confidence.high_top1 = 0.02;
    r.confidence.low_entropy = 1.0;
    r.confidence.high_entropy = 1.0;
    r
}

pub fn env_with(endpoints: Endpoints, tune: impl FnOnce(&mut Config)) -> Env {
    let folio_file = corpus();
    let index = index::Index::open(folio_file.path(), retrieve()).unwrap();
    let office = tempdir::Dir::new();
    let mut config = Config {
        office: office.0.clone(),
        ..Config::default()
    };
    tune(&mut config);
    let fake = Arc::new(Fake::default());
    let sink = Arc::new(Recorder::default());
    let clock = Arc::new(TestClock(AtomicI64::new(T0)));
    let agent = Agent::new(Parts {
        world: World::in_memory().unwrap(),
        index,
        config,
        endpoints,
        keys: Some(Arc::new(StaticKeys(vec![(
            "p".into(),
            llm::Secret::new("sk-test"),
        )]))),
        llm: Llm::new(fake.clone(), Retry::none()),
        sink: sink.clone(),
        clock: clock.clone(),
    })
    .unwrap();
    Env {
        agent: Arc::new(agent),
        fake,
        sink,
        clock,
        _folio: folio_file,
        _office: office,
    }
}

pub fn env(wire: WireApi) -> Env {
    env_with(endpoints(wire, true, None), |_| {})
}

pub fn p(s: &str) -> world::PersonId {
    world::PersonId::from(s)
}

/// The list a body carries its turns in.
pub fn list(v: &Value) -> &Vec<Value> {
    v.get("input")
        .or_else(|| v.get("messages"))
        .and_then(Value::as_array)
        .unwrap()
}

/// A body without its turns and without `tool_choice`: what must not change between two
/// calls on one log.
pub fn head(v: &Value) -> Value {
    let mut h = v.clone();
    let o = h.as_object_mut().unwrap();
    o.remove("input");
    o.remove("messages");
    o.remove("tool_choice");
    h
}

/// Every consecutive pair of rendered requests on the same log: the later has the same
/// head, and its turns start with all of the earlier one's. Rendering keeps keys sorted,
/// so this is the cached prefix the endpoint sees, compared as the bytes of each turn too.
pub fn assert_prefix_chain(bodies: &[Vec<u8>]) {
    assert!(!bodies.is_empty());
    let json: Vec<Value> = bodies
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    for (i, pair) in json.windows(2).enumerate() {
        assert_eq!(
            head(&pair[0]),
            head(&pair[1]),
            "body {} changed the head",
            i + 1
        );
        let (a, b) = (list(&pair[0]), list(&pair[1]));
        assert!(b.len() > a.len(), "body {} adds nothing", i + 1);
        for (j, (x, y)) in a.iter().zip(b).enumerate() {
            assert_eq!(
                x.to_string(),
                y.to_string(),
                "body {} rewrote turn {j} of body {i}",
                i + 1
            );
        }
    }
}

/// Every byte of a channel's logs, prefixes and entries, every segment.
pub fn log_bytes(agent: &Agent, ch: ChannelId) -> String {
    agent.with_world(|w| {
        let mut stmt = w
            .prepare(
                "SELECT CAST(prefix_a || prefix_b || prefix_c AS BLOB) FROM log_segment WHERE channel = ?1
                 UNION ALL SELECT e.bytes FROM log_entry e JOIN log_segment s ON s.id = e.segment
                 WHERE s.channel = ?1",
            )
            .unwrap();
        let rows = stmt
            .query_map([ch], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(|r| String::from_utf8(r.unwrap()).unwrap())
            .collect::<Vec<_>>();
        rows.join("\n")
    })
}

pub fn segments(agent: &Agent, ch: ChannelId) -> Vec<world::log::Segment> {
    agent.with_world(|w| {
        let mut stmt = w
            .prepare("SELECT id FROM log_segment WHERE channel = ?1 ORDER BY seq")
            .unwrap();
        let ids: Vec<world::SegmentId> = stmt
            .query_map([ch], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        ids.into_iter()
            .map(|id| {
                w.query_row(
                    "SELECT id, channel, seq, prefix_a, prefix_b, prefix_c, shape, opened_at
                     FROM log_segment WHERE id = ?1",
                    [id],
                    |r| {
                        Ok(world::log::Segment {
                            id: r.get(0)?,
                            channel: r.get(1)?,
                            seq: r.get(2)?,
                            prefix_a: r.get(3)?,
                            prefix_b: r.get(4)?,
                            prefix_c: r.get(5)?,
                            shape: r.get(6)?,
                            opened_at: r.get(7)?,
                        })
                    },
                )
                .unwrap()
            })
            .collect()
    })
}
