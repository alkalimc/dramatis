//! Manual smoke test against a real endpoint. Never run in CI (`#[ignore]`): it makes
//! three real model calls and spends real quota.
//!
//! ```sh
//! DRAMATIS_SMOKE_FOLIO=/path/to/corpus.folio \
//! DRAMATIS_SMOKE_ENDPOINTS=$HOME/.dramatis/endpoints.toml \
//! cargo test -p agent --test smoke -- --ignored --nocapture
//! ```
//!
//! The chat role's key is read from the system keychain (service `dramatis`, account
//! `profile.<name>`). `DRAMATIS_SMOKE_PERSON` picks who to talk to (default: the first
//! person with a persona) and `DRAMATIS_SMOKE_SAY` what the user says, one message per
//! line. The world database is a throwaway in memory. It prints one meter row per call;
//! from the second call on the cached input must be non-zero. With `DRAMATIS_SMOKE_USAGE`
//! set it also prints every `usage` object the endpoint returns, so a cache write and a
//! plain uncached input can be told apart. After the wrap-up it prints the facts written to
//! the world. The cache check is skipped when the chat role is billed per request.

use std::sync::Arc;

use agent::{Agent, Config, NoSink, Parts, SystemClock};
use futures::StreamExt;
use llm::transport::{Http, HttpRequest, HttpResponse, StreamResponse, Transport};

/// Real HTTP that echoes the `usage` part of every reply and, with
/// `DRAMATIS_SMOKE_DUMP=<dir>`, writes each request body there as `<n>.json` and its
/// response body next to it as `<n>.response.json`.
struct ShowUsage(Http);

/// Writes the request body and returns where its response goes.
fn dump(req: &HttpRequest) -> Option<std::path::PathBuf> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::path::PathBuf::from(std::env::var_os("DRAMATIS_SMOKE_DUMP")?);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{n}.json")), &req.body);
    Some(dir.join(format!("{n}.response.json")))
}

fn save(path: &Option<std::path::PathBuf>, bytes: &[u8]) {
    use std::io::Write;
    if let Some(path) = path
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = f.write_all(bytes);
    }
}

fn show(bytes: &[u8]) {
    for line in String::from_utf8_lossy(bytes).lines() {
        if let Some(i) = line.find("\"usage\"") {
            println!("usage> {}", &line[i..]);
        }
    }
}

#[async_trait::async_trait]
impl Transport for ShowUsage {
    async fn send(&self, req: HttpRequest) -> llm::Result<HttpResponse> {
        let out = dump(&req);
        let r = self.0.send(req).await?;
        show(&r.body);
        save(&out, &r.body);
        Ok(r)
    }
    async fn open(&self, req: HttpRequest) -> llm::Result<StreamResponse> {
        let out = dump(&req);
        let r = self.0.open(req).await?;
        let chunks = r
            .chunks
            .inspect(move |c| {
                if let Ok(b) = c {
                    show(b);
                    save(&out, b);
                }
            })
            .boxed();
        Ok(StreamResponse {
            status: r.status,
            chunks,
            retry_after: None,
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "calls a real endpoint; run by hand"]
async fn two_turns_and_a_wrapup_hit_the_cache() {
    let folio = std::env::var("DRAMATIS_SMOKE_FOLIO").expect("DRAMATIS_SMOKE_FOLIO");
    let endpoints = std::env::var("DRAMATIS_SMOKE_ENDPOINTS").expect("DRAMATIS_SMOKE_ENDPOINTS");
    let endpoints =
        api::endpoints::Endpoints::from_toml(&std::fs::read_to_string(endpoints).unwrap()).unwrap();
    let per_request_chat = {
        let chat = &endpoints.roles.as_ref().expect("no chat role").chat;
        endpoints
            .profile(&chat.profile)
            .and_then(|p| p.models.iter().find(|m| m.id == chat.model))
            .is_some_and(|m| m.per_request())
    };
    let office = std::env::temp_dir().join("dramatis-smoke-office");
    let agent = Agent::new(Parts {
        world: world::World::in_memory().unwrap(),
        index: index::Index::open(&folio, Default::default()).unwrap(),
        config: Config {
            office,
            ..Config::default()
        },
        endpoints,
        keys: None,
        llm: if std::env::var_os("DRAMATIS_SMOKE_USAGE").is_some()
            || std::env::var_os("DRAMATIS_SMOKE_DUMP").is_some()
        {
            llm::Llm::new(
                Arc::new(ShowUsage(Http::new().unwrap())),
                Default::default(),
            )
        } else {
            llm::Llm::http().unwrap()
        },
        sink: Arc::new(NoSink),
        clock: Arc::new(SystemClock { offset_min: 0 }),
    })
    .unwrap();
    let person = std::env::var("DRAMATIS_SMOKE_PERSON").unwrap_or_else(|_| {
        agent
            .personas()
            .persons()
            .next()
            .expect("the corpus has no persona")
            .to_owned()
    });
    let ch = agent.open_direct(&world::PersonId(person.clone())).unwrap();
    let say = std::env::var("DRAMATIS_SMOKE_SAY")
        .unwrap_or_else(|_| "Hello. How are you today?\nWhat did you do this morning?".into());
    for text in say.lines().filter(|l| !l.trim().is_empty()) {
        agent.send(ch, text, None).await.unwrap();
    }
    agent.wrapup(ch).await.unwrap();
    for m in agent.history(ch, None, 50).unwrap() {
        println!("{:?}: {}", m.author, m.text);
    }
    let facts: Vec<(String, String, String, String)> = agent.with_world(|w| {
        let mut s = w
            .prepare(
                "SELECT kind, text, audience_kind, COALESCE(audience_ref, '') FROM fact \
                 WHERE retracted = 0 ORDER BY id",
            )
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    });
    println!("facts written: {}", facts.len());
    for (kind, text, audience, aref) in &facts {
        println!("fact> {kind} | {text} | {audience} {aref}");
    }
    let rows: Vec<(String, i64, i64, i64)> = agent.with_world(|w| {
        let mut s = w
            .prepare("SELECT shape, uncached, cached, output FROM meter ORDER BY id")
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    });
    println!("shape uncached cached output");
    for r in &rows {
        println!("{} {} {} {}", r.0, r.1, r.2, r.3);
    }
    assert!(rows.len() >= 2, "{rows:?}");
    // A request-billed chat role reports no cache, so there is nothing to assert.
    assert!(
        per_request_chat || rows[1..].iter().all(|r| r.2 > 0),
        "a later call missed the cache: {rows:?}"
    );
}
