//! No network: fixtures per protocol through the in-memory transport.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use llm::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn endpoint(protocol: Protocol) -> Endpoint {
    Endpoint {
        protocol,
        base_url: "https://example.test/v1/".into(),
        model: "model-a".into(),
        key: Secret::new("sk-secret-value"),
        caps: Caps {
            forced_tool: true,
            ..Caps::default()
        },
    }
}

const ALL: [Protocol; 3] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
];

fn base_request() -> Request {
    Request {
        system: vec![
            SystemBlock {
                text: "rules".into(),
                cache_mark: true,
            },
            SystemBlock {
                text: "persona".into(),
                cache_mark: true,
            },
            SystemBlock {
                text: "scene".into(),
                cache_mark: false,
            },
        ],
        messages: vec![Message::user("hello")],
        tools: vec![ToolSpec {
            name: "search".into(),
            description: "look something up".into(),
            schema: json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }],
        tool_choice: ToolChoice::Auto,
        effort: Some(Effort::High),
        max_output: Some(4000),
        cache_key: Some("segment-1".into()),
        output_schema: None,
    }
}

fn body(r: &Rendered) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

fn log_key(p: Protocol) -> &'static str {
    if p == Protocol::OpenAiResponses {
        "input"
    } else {
        "messages"
    }
}

fn fixture_reply(p: Protocol) -> Response {
    let name = match p {
        Protocol::OpenAiChat => "chat.json",
        Protocol::OpenAiResponses => "responses.json",
        _ => "messages.json",
    };
    decode::decode_response(p, &serde_json::from_slice(&fixture(name)).unwrap()).unwrap()
}

#[test]
fn render_is_deterministic_with_sorted_keys() {
    for p in ALL {
        let ep = endpoint(p);
        let a = render(&ep, &base_request()).unwrap();
        let b = render(&ep, &base_request()).unwrap();
        assert_eq!(a, b);
        let mut sorted = body(&a);
        sorted.sort_all_objects();
        assert_eq!(serde_json::to_vec(&sorted).unwrap(), a.body, "{p:?}");
    }
}

#[test]
fn headers_and_urls_never_carry_the_secret() {
    for (p, path) in [
        (Protocol::OpenAiChat, "chat/completions"),
        (Protocol::OpenAiResponses, "responses"),
        (Protocol::AnthropicMessages, "messages"),
    ] {
        let ep = endpoint(p);
        let r = render(&ep, &base_request()).unwrap();
        assert_eq!(r.url, format!("https://example.test/v1/{path}"));
        assert!(!format!("{r:?}{ep:?}").contains("sk-secret-value"));
        let auth = auth_headers(&ep);
        assert_eq!(auth.len(), 1);
        let (k, v) = &auth[0];
        if p == Protocol::AnthropicMessages {
            assert_eq!((k.as_str(), v.as_str()), ("x-api-key", "sk-secret-value"));
            assert!(
                r.headers
                    .contains(&("anthropic-version".into(), "2023-06-01".into()))
            );
        } else {
            assert_eq!(v, "Bearer sk-secret-value");
        }
    }
    let mut keyless = endpoint(Protocol::OpenAiChat);
    keyless.key = Secret::default();
    assert!(auth_headers(&keyless).is_empty());
}

/// The cache invariant: one more user message and one assistant reply (with its native
/// blocks) append to the log and change nothing before it.
#[test]
fn appending_a_turn_leaves_the_prefix_byte_identical() {
    for p in ALL {
        let ep = endpoint(p);
        let before = base_request();
        let mut after = before.clone();
        let reply = fixture_reply(p);
        if p != Protocol::OpenAiChat {
            assert!(
                reply.parts.iter().any(|x| matches!(x, Part::Native(_))),
                "{p:?}"
            );
        }
        let calls: Vec<String> = reply.tool_calls().map(|c| c.0.to_owned()).collect();
        after.messages.push(reply.into_message());
        after.messages.push(Message {
            role: Role::Tool,
            parts: calls
                .iter()
                .map(|id| Part::ToolResult {
                    call_id: id.clone(),
                    content: "found".into(),
                    is_error: false,
                })
                .collect(),
        });
        after.messages.push(Message::user("one more"));

        let (a, b) = (
            body(&render(&ep, &before).unwrap()),
            body(&render(&ep, &after).unwrap()),
        );
        let (mut a, mut b) = (a, b);
        let key = log_key(p);
        let (la, lb) = (a[key].take(), b[key].take());
        assert_eq!(a, b, "{p:?}: every field but the log is unchanged");
        let (la, lb) = (la.as_array().unwrap(), lb.as_array().unwrap());
        assert!(lb.len() > la.len());
        for (x, y) in la.iter().zip(lb) {
            assert_eq!(x.to_string(), y.to_string(), "{p:?}");
        }
    }
}

#[test]
fn messages_cache_policy_and_effort() {
    let ep = endpoint(Protocol::AnthropicMessages);
    let mut req = base_request();
    req.output_schema = Some(json!({"type": "object"}));
    let b = body(&render(&ep, &req).unwrap());
    assert_eq!(b["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(b["thinking"], json!({"type": "adaptive"}));
    assert_eq!(b["output_config"]["effort"], "high");
    assert_eq!(
        b["output_config"]["format"],
        json!({"type": "json_schema", "schema": {"type": "object"}})
    );
    assert_eq!(b["max_tokens"], 4000);
    let sys = b["system"].as_array().unwrap();
    assert!(sys[0].get("cache_control").is_some() && sys[1].get("cache_control").is_some());
    assert!(sys[2].get("cache_control").is_none());
    assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(b["tool_choice"], json!({"type": "auto"}));
    assert!(b.get("prompt_cache_key").is_none() && b.get("store").is_none());

    let mut tight = endpoint(Protocol::AnthropicMessages);
    tight.caps.cache_breakpoints = Some(2);
    assert!(matches!(render(&tight, &req), Err(Error::Invalid(_))));
    tight.caps.cache_breakpoints = Some(0);
    let b = body(&render(&tight, &req).unwrap());
    assert!(b.get("cache_control").is_none());
    assert!(b["system"][0].get("cache_control").is_none());
}

#[test]
fn openai_cache_effort_and_structured_output() {
    let mut req = base_request();
    req.output_schema = Some(json!({"type": "object"}));
    let b = body(&render(&endpoint(Protocol::OpenAiResponses), &req).unwrap());
    assert_eq!(b["prompt_cache_key"], "segment-1");
    assert_eq!(b["store"], false);
    assert_eq!(b["reasoning"]["effort"], "high");
    assert_eq!(b["text"]["format"]["type"], "json_schema");
    assert_eq!(b["max_output_tokens"], 4000);
    assert!(b.get("thinking").is_none() && b.get("cache_control").is_none());
    assert_eq!(b["input"][0], json!({"role": "system", "content": "rules"}));

    req.effort = Some(Effort::Max);
    let b = body(&render(&endpoint(Protocol::OpenAiChat), &req).unwrap());
    assert!(b.get("prompt_cache_key").is_none() && b.get("store").is_none());
    assert_eq!(b["reasoning_effort"], "high");
    assert_eq!(b["response_format"]["type"], "json_schema");
    assert_eq!(b["max_completion_tokens"], 4000);
    let b = body(&render_stream(&endpoint(Protocol::OpenAiChat), &req).unwrap());
    assert_eq!(b["stream"], true);
    assert_eq!(b["stream_options"]["include_usage"], true);
}

#[test]
fn forced_tool_and_foreign_native_are_typed_errors() {
    let mut req = base_request();
    req.tool_choice = ToolChoice::Tool("search".into());
    let b = body(&render(&endpoint(Protocol::OpenAiResponses), &req).unwrap());
    assert_eq!(
        b["tool_choice"],
        json!({"type": "function", "name": "search"})
    );
    // The messages protocol never gets one, whatever the caps claim.
    assert!(matches!(
        render(&endpoint(Protocol::AnthropicMessages), &req),
        Err(Error::ForcedToolNotAllowed)
    ));
    let mut no_force = endpoint(Protocol::OpenAiChat);
    no_force.caps.forced_tool = false;
    assert!(matches!(
        render(&no_force, &req),
        Err(Error::ForcedToolNotAllowed)
    ));

    let thinking = Part::Native(json!({"type": "thinking", "thinking": "x", "signature": "s"}));
    let mut req = base_request();
    req.messages.push(Message::assistant(vec![thinking]));
    assert!(render(&endpoint(Protocol::AnthropicMessages), &req).is_ok());
    for p in [Protocol::OpenAiChat, Protocol::OpenAiResponses] {
        assert!(
            matches!(render(&endpoint(p), &req), Err(Error::ForeignNative { .. })),
            "{p:?}"
        );
    }
}

#[test]
fn whole_replies_normalise_parts_stop_and_usage() {
    let m = fixture_reply(Protocol::AnthropicMessages);
    assert_eq!(m.stop, Stop::ToolUse);
    assert_eq!(
        m.usage,
        Usage {
            uncached_in: 12,
            cached_in: 5000,
            cache_write_in: 300,
            output: 80,
            reasoning: 0
        }
    );
    assert!(matches!(&m.parts[0], Part::Native(v) if v["signature"] == "sig-abc=="));
    assert_eq!(m.text(), "Checking.");
    assert_eq!(
        m.tool_calls().next().unwrap(),
        ("toolu_1", "search", "{\"query\":\"harbour\"}")
    );

    let r = fixture_reply(Protocol::OpenAiResponses);
    assert_eq!(r.stop, Stop::ToolUse);
    assert_eq!(
        r.usage,
        Usage {
            uncached_in: 200,
            cached_in: 800,
            cache_write_in: 0,
            output: 90,
            reasoning: 40
        }
    );
    assert!(matches!(&r.parts[0], Part::Native(v) if v["type"] == "reasoning"));

    let c = fixture_reply(Protocol::OpenAiChat);
    assert_eq!(c.stop, Stop::ToolUse);
    assert_eq!(c.usage, r.usage);
    let hm: Value = serde_json::from_slice(&fixture("chat_hitmiss.json")).unwrap();
    let u = decode::decode_response(Protocol::OpenAiChat, &hm).unwrap();
    assert_eq!(
        (u.usage.uncached_in, u.usage.cached_in, u.stop),
        (300, 700, Stop::EndTurn)
    );

    let refused: Value = serde_json::from_slice(&fixture("messages_refusal.json")).unwrap();
    let r = decode::decode_response(Protocol::AnthropicMessages, &refused).unwrap();
    assert_eq!(r.stop, Stop::Refusal);
    assert!(r.parts.is_empty(), "content of a refusal is not read");
}

async fn collect(p: Protocol, name: &str, chunk: usize) -> Vec<Event> {
    let t = Arc::new(MemoryTransport::new([Ok(Canned::chunked(
        fixture(name),
        chunk,
    ))]));
    let llm = Llm::new(t, Retry::none());
    let s = llm.stream(&endpoint(p), &base_request()).await.unwrap();
    s.map(|e| e.unwrap()).collect().await
}

#[tokio::test]
async fn a_structured_output_request_is_sent_whole_on_every_wire() {
    for (p, name) in [
        (Protocol::OpenAiChat, "chat.json"),
        (Protocol::OpenAiResponses, "responses.json"),
        (Protocol::AnthropicMessages, "messages.json"),
    ] {
        let t = Arc::new(MemoryTransport::new([Ok(Canned::chunked(
            fixture(name),
            1 << 16,
        ))]));
        let llm = Llm::new(t.clone(), Retry::none());
        let mut req = base_request();
        req.output_schema = Some(json!({"type": "object"}));
        let events: Vec<Event> = llm
            .stream(&endpoint(p), &req)
            .await
            .unwrap()
            .map(|e| e.unwrap())
            .collect()
            .await;
        assert_eq!(events, [Event::Done(fixture_reply(p))], "{p:?}");
        let sent: Value = serde_json::from_slice(&t.sent()[0].body).unwrap();
        assert!(sent.get("stream").is_none(), "{p:?}");
    }
}

#[tokio::test]
async fn streams_decode_to_the_same_response_as_whole_replies() {
    for (p, name, whole) in [
        (
            Protocol::AnthropicMessages,
            "messages.sse",
            fixture_reply(Protocol::AnthropicMessages),
        ),
        (
            Protocol::OpenAiResponses,
            "responses.sse",
            fixture_reply(Protocol::OpenAiResponses),
        ),
        (
            Protocol::OpenAiChat,
            "chat.sse",
            fixture_reply(Protocol::OpenAiChat),
        ),
    ] {
        // Odd chunk sizes split events and multi-byte boundaries.
        for chunk in [7, 64, 1 << 16] {
            let events = collect(p, name, chunk).await;
            let Some(Event::Done(done)) = events.last() else {
                panic!("{p:?}: no Done")
            };
            assert_eq!(done, &whole, "{p:?} chunk {chunk}");
            let text: String = events
                .iter()
                .filter_map(|e| {
                    if let Event::TextDelta(t) = e {
                        Some(t.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(text, "Checking.");
            let calls: Vec<_> = events
                .iter()
                .filter(|e| matches!(e, Event::ToolCall { .. }))
                .collect();
            assert_eq!(calls.len(), 1, "{p:?}");
            if p != Protocol::OpenAiChat {
                assert!(
                    events.iter().any(|e| matches!(e, Event::ReasoningDelta(_))),
                    "{p:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_stream_that_stops_early_is_an_error() {
    let cut = String::from_utf8(fixture("messages.sse")).unwrap();
    let cut = &cut[..cut.find("event: message_delta").unwrap()];
    let t = Arc::new(MemoryTransport::new([Ok(Canned::ok(cut))]));
    let llm = Llm::new(t, Retry::none());
    let s = llm
        .stream(&endpoint(Protocol::AnthropicMessages), &base_request())
        .await
        .unwrap();
    let all: Vec<_> = s.collect().await;
    assert!(matches!(all.last(), Some(Err(Error::Decode(_)))));
}

#[tokio::test]
async fn complete_sends_the_rendered_request_with_auth() {
    let t = Arc::new(MemoryTransport::new([Ok(Canned::ok(fixture(
        "messages.json",
    )))]));
    let llm = Llm::new(t.clone(), Retry::none());
    let ep = endpoint(Protocol::AnthropicMessages);
    let r = llm.complete(&ep, &base_request()).await.unwrap();
    assert_eq!(r.stop, Stop::ToolUse);
    let sent = t.sent();
    assert_eq!(sent[0].body, render(&ep, &base_request()).unwrap().body);
    assert!(
        sent[0]
            .headers
            .contains(&("x-api-key".into(), "sk-secret-value".into()))
    );
    assert!(!format!("{:?}", sent[0]).contains("sk-secret-value"));
}

#[tokio::test]
async fn errors_map_and_only_transient_ones_retry() {
    let ep = endpoint(Protocol::OpenAiResponses);
    let retry = Retry {
        attempts: 3,
        base: Duration::from_millis(1),
    };
    let overloaded = Canned::status(
        529,
        r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
    );
    let limited = Canned::status(429, r#"{"error":{"message":"slow down"}}"#);
    let t = Arc::new(MemoryTransport::new([
        Ok(limited.clone()),
        Err(Error::Transport("reset".into())),
        Ok(Canned::ok(fixture("responses.json"))),
    ]));
    let r = Llm::new(t.clone(), retry)
        .complete(&ep, &base_request())
        .await
        .unwrap();
    assert_eq!(r.stop, Stop::ToolUse);
    assert_eq!(t.sent().len(), 3);

    let t = Arc::new(MemoryTransport::new([
        Ok(limited.clone()),
        Ok(limited),
        Ok(overloaded),
    ]));
    let e = Llm::new(t.clone(), retry)
        .complete(&ep, &base_request())
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "HTTP 529: busy");
    assert_eq!(t.sent().len(), 3, "bounded");

    let bad = Canned::status(400, r#"{"error":{"message":"bad schema"}}"#);
    let t = Arc::new(MemoryTransport::new([Ok(bad)]));
    let e = Llm::new(t.clone(), retry)
        .complete(&ep, &base_request())
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "HTTP 400: bad schema");
    assert_eq!(t.sent().len(), 1, "a client error is final");

    let t = Arc::new(MemoryTransport::new([Ok(Canned::status(401, "nope"))]));
    let e = Llm::new(t, retry)
        .stream(&ep, &base_request())
        .await
        .err()
        .unwrap();
    assert!(matches!(e, Error::Http { status: 401, .. }));
}

#[tokio::test]
async fn a_long_rate_limit_is_final_and_typed() {
    let ep = endpoint(Protocol::OpenAiResponses);
    let retry = Retry {
        attempts: 3,
        base: Duration::from_millis(1),
    };
    let body = r#"{"error":{"message":"all slots full"}}"#;
    let t = Arc::new(MemoryTransport::new([Ok(
        Canned::status(429, body).retry_after(3600)
    )]));
    let e = Llm::new(t.clone(), retry)
        .stream(&ep, &base_request())
        .await
        .err()
        .unwrap();
    assert!(matches!(
        &e,
        Error::RateLimited {
            retry_after: Some(3600),
            ..
        }
    ));
    assert_eq!(
        e.to_string(),
        "HTTP 429 (retry after 3600 s): all slots full"
    );
    assert_eq!(t.sent().len(), 1, "no waiting out an hour inside a call");

    // Without a hint the limit is retried a few times, then reported as such.
    let t = Arc::new(MemoryTransport::new([
        Ok(Canned::status(429, body)),
        Ok(Canned::status(429, body)),
        Ok(Canned::status(429, body)),
    ]));
    let e = Llm::new(t.clone(), retry)
        .complete(&ep, &base_request())
        .await
        .unwrap_err();
    assert!(matches!(
        e,
        Error::RateLimited {
            retry_after: None,
            ..
        }
    ));
    assert_eq!(t.sent().len(), 3);
}

#[tokio::test]
async fn models_speech_and_embeddings() {
    let ep = endpoint(Protocol::AnthropicMessages);
    let list = r#"{"data":[{"id":"claude-a"},{"id":"claude-b"}]}"#;
    let t = Arc::new(MemoryTransport::new([
        Ok(Canned::ok(list)),
        Ok(Canned::status(
            401,
            r#"{"error":{"message":"invalid key"}}"#,
        )),
    ]));
    let llm = Llm::new(t.clone(), Retry::none());
    assert_eq!(llm.models(&ep).await.unwrap(), ["claude-a", "claude-b"]);
    assert_eq!(
        llm.models(&ep).await.unwrap_err().to_string(),
        "HTTP 401: invalid key"
    );
    assert!(
        t.sent()[0]
            .url
            .starts_with("https://example.test/v1/models")
    );
    assert!(matches!(
        llm.speech(&ep, "hi", "v").await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        llm.embed(&ep, &["a".into()]).await,
        Err(Error::Unsupported(_))
    ));

    let oa = endpoint(Protocol::OpenAiChat);
    let emb = r#"{"data":[{"index":1,"embedding":[0.5,1.5]},{"index":0,"embedding":[1.0,2.0]}]}"#;
    let t = Arc::new(MemoryTransport::new([
        Ok(Canned::ok([1u8, 2, 3])),
        Ok(Canned::ok(emb)),
    ]));
    let llm = Llm::new(t.clone(), Retry::none());
    assert_eq!(llm.speech(&oa, "hello", "alloy").await.unwrap(), [1, 2, 3]);
    let v = llm.embed(&oa, &["a".into(), "b".into()]).await.unwrap();
    assert_eq!(v, [vec![1.0, 2.0], vec![0.5, 1.5]]);
    let sent = t.sent();
    assert!(sent[0].url.ends_with("/audio/speech") && sent[1].url.ends_with("/embeddings"));
    let b: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(
        (b["input"].as_str(), b["voice"].as_str()),
        (Some("hello"), Some("alloy"))
    );
}
