//! The first screen costs no model call and must come up in under 100 ms: open the
//! corpus, then ask for the status, the waiting panel and the roster.
//!
//! The synthetic corpus is sized like a real one (thousands of people, tens of thousands
//! of units). Point `DRAMATIS_BENCH_FOLIO` at a real corpus to time that one as well; run
//! with `--release --nocapture` for the number that matters.

use std::path::Path;
use std::time::Instant;

use api::home::Home;
use api::{Api, Engine, Ports};
use folio::fixture::{Builder, UnitSpec};

const PEOPLE: usize = 2500;
const UNITS: usize = 30_000;

fn home(tag: &str) -> Home {
    let dir = std::env::temp_dir().join(format!("dramatis-first-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Home::new(dir)
}

async fn first_screen(folio: &Path, tag: &str) -> std::time::Duration {
    let home = home(tag);
    let start = Instant::now();
    let engine = Engine::open(home.clone(), Some(folio.to_path_buf()), Ports::default()).unwrap();
    let status = engine.app_status().await.unwrap();
    let digest = engine.digest().await.unwrap();
    let roster = engine.roster().await.unwrap();
    let took = start.elapsed();
    println!(
        "first screen ({tag}): {took:?}; {} people in {} groups, {} waiting, {:?}",
        roster.total,
        roster.groups.len(),
        digest.items.len(),
        status.corpus
    );
    let _ = std::fs::remove_dir_all(home.root());
    took
}

#[tokio::test]
async fn synthetic_corpus_first_screen() {
    let mut b = Builder::new();
    for p in 0..PEOPLE {
        b.person(&format!("p{p:04}"), None);
    }
    for u in 0..UNITS {
        let who = format!("p{:04}", u % PEOPLE);
        b.unit(UnitSpec {
            id: &format!("u{u}"),
            template: "talk",
            page: &format!("Page {}", u / 10),
            text: &format!("line {u} about the lantern and the furnace"),
            persons: &[who.as_str()],
            ..UnitSpec::default()
        });
    }
    for p in 0..PEOPLE - 1 {
        b.cooccur(
            &format!("p{p:04}"),
            &format!("p{:04}", p + 1),
            1 + (p % 7) as i64,
        );
    }
    for p in 0..3 {
        for slot in ["system", "tone", "capability"] {
            b.conn()
                .execute(
                    "INSERT INTO prompts(subject, slot, body) VALUES (?1, ?2, 'x')",
                    [&format!("p{p:04}"), slot],
                )
                .unwrap();
        }
    }
    let folio = b.finish();
    let took = first_screen(folio.path(), "synthetic").await;
    if !cfg!(debug_assertions) {
        assert!(took.as_millis() < 100, "{took:?}");
    }
}

#[tokio::test]
async fn real_corpus_first_screen() {
    let Some(path) = std::env::var_os("DRAMATIS_BENCH_FOLIO") else {
        return;
    };
    let took = first_screen(Path::new(&path), "real").await;
    if !cfg!(debug_assertions) {
        assert!(took.as_millis() < 100, "{took:?}");
    }
}
