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
//! person with a persona). The world database is a throwaway in memory. It prints one
//! meter row per call; from the second call on the cached input must be non-zero.

use std::sync::Arc;

use agent::endpoint::{Http, Keychain};
use agent::{Agent, Config, NoSink, Parts, SystemClock};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "calls a real endpoint; run by hand"]
async fn two_turns_and_a_wrapup_hit_the_cache() {
    let folio = std::env::var("DRAMATIS_SMOKE_FOLIO").expect("DRAMATIS_SMOKE_FOLIO");
    let endpoints = std::env::var("DRAMATIS_SMOKE_ENDPOINTS").expect("DRAMATIS_SMOKE_ENDPOINTS");
    let endpoints =
        api::endpoints::Endpoints::from_toml(&std::fs::read_to_string(endpoints).unwrap()).unwrap();
    let office = std::env::temp_dir().join("dramatis-smoke-office");
    let agent = Agent::new(Parts {
        world: world::World::in_memory().unwrap(),
        index: index::Index::open(&folio, Default::default()).unwrap(),
        config: Config {
            office,
            ..Config::default()
        },
        endpoints,
        keys: Arc::new(Keychain),
        transport: Arc::new(Http::default()),
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
    for text in ["Hello. How are you today?", "What did you do this morning?"] {
        agent.send(ch, text, None).await.unwrap();
    }
    agent.wrapup(ch).await.unwrap();
    for m in agent.history(ch, None, 10).unwrap() {
        println!("{:?}: {}", m.author, m.text);
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
    assert!(
        rows[1..].iter().all(|r| r.2 > 0),
        "a later call missed the cache: {rows:?}"
    );
}
