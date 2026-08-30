//! Maintainer surface for a corpus.
//!
//! Three commands, and the third is the reason this binary exists before the daemon does:
//!
//!   inspect   what does this corpus contain, and what does it require of a reader
//!   search    run the pipeline and show what came back
//!   bench     measure latency and resident memory on this machine
//!
//! `bench` is a probe wearing a CLI. Two freeze conditions — first-token latency and
//! resident memory — cannot be settled on paper, so the skeleton that answers them has to
//! exist before the design can be called frozen. That is the honest version of "freeze
//! first, then build".

mod mem;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use folio::Folio;
use index::{Index, Mode, Request};

#[derive(Parser)]
#[command(name = "dramatis-cli", about = "Inspect, query and measure a .folio corpus")]
struct Cli {
    /// Path to the corpus.
    #[arg(long, short, global = true, default_value = "../artifacts/arknights/arknights.folio")]
    folio: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What this corpus contains, and what it requires of a reader.
    Inspect,
    /// Search. Without vectors this is the lexical path only, which is still useful.
    Search {
        query: String,
        #[arg(long, default_value_t = 6)]
        top_k: usize,
        #[arg(long, default_value = "lexical")]
        mode: String,
        /// Widen each hit by this many neighbouring units on each side.
        #[arg(long, default_value_t = 0)]
        expand: i64,
        #[arg(long)]
        template: Vec<String>,
    },
    /// Measure the pipeline: per-stage latency distribution and resident memory.
    Bench {
        /// Queries to run. Defaults to a spread across the templates.
        #[arg(long)]
        query: Vec<String>,
        #[arg(long, default_value_t = 200)]
        iterations: usize,
        #[arg(long, default_value_t = 6)]
        top_k: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect => inspect(&cli.folio),
        Command::Search {
            query,
            top_k,
            mode,
            expand,
            template,
        } => search(&cli.folio, &query, top_k, &mode, expand, template),
        Command::Bench {
            query,
            iterations,
            top_k,
        } => bench(&cli.folio, query, iterations, top_k),
    }
}

fn inspect(path: &PathBuf) -> Result<()> {
    let folio = Folio::open(path).with_context(|| format!("opening {}", path.display()))?;
    let manifest = folio.manifest();

    println!("corpus     {}", path.display());
    println!("pack       {} v{}", manifest.pack, manifest.pack_version);
    println!("format     {}", manifest.format_version);
    println!("parser     v{}", manifest.parser_version);
    println!("segmenter  {}", manifest.segmenter);
    println!("units      {}", manifest.unit_count);
    println!("people     {}", folio.person_count()?);
    println!("fingerprint {}", manifest.build_fingerprint);
    println!("revid max  {}", manifest.source_revid_max);

    println!("\nrequires");
    if manifest.requires.is_empty() {
        println!("  (nothing)");
    }
    for requirement in &manifest.requires {
        // Reaching this point means the requirement is implemented — `Folio::open` refuses
        // otherwise — so printing it is a statement about the contract, not a check.
        println!("  {requirement}  [implemented]");
    }

    println!("\nunits by template");
    for stats in folio.template_stats()? {
        println!(
            "  {:<10} {:>7}  embed p50 {:>4}  p95 {:>4}  max {:>5}",
            stats.template, stats.count, stats.embed_p50, stats.embed_p95, stats.embed_max
        );
    }

    if folio.has_vectors()? {
        let mut folio = folio;
        let vectors = folio.load_vectors()?;
        println!(
            "\nvectors    {} x {}d f16 = {:.0} MB",
            vectors.count(),
            vectors.dim(),
            vectors.bytes() as f64 / 1e6
        );
    } else {
        println!("\nvectors    not written yet — run `dramatis-forge corpus encode`");
        println!("           the lexical path works without them");
    }
    Ok(())
}

fn search(
    path: &PathBuf,
    query: &str,
    top_k: usize,
    mode: &str,
    expand: i64,
    templates: Vec<String>,
) -> Result<()> {
    let mut folio = Folio::open(path)?;
    let mode = match mode {
        "hybrid" => Mode::Hybrid,
        "dense" => Mode::Dense,
        _ => Mode::Lexical,
    };
    if mode != Mode::Lexical && folio.has_vectors()? {
        folio.load_vectors()?;
    }

    let idx = Index::new(&folio);
    println!("segmenter  {} (from the corpus manifest)", idx.segmenter().name());
    let all = idx.segmenter().segment(query);
    let queried = idx.segmenter().segment_for_query(query);
    println!("tokens     {all:?}");
    if queried != all {
        // Stopwords are dropped from queries only; the index keeps them. Showing both makes
        // the difference visible rather than something to discover from a score.
        println!("queried    {queried:?}  (stopwords dropped)");
    }
    println!();

    let request = Request {
        query: query.to_string(),
        mode,
        top_k,
        templates,
        expand,
        ..Default::default()
    };
    let response = idx.search(&request, None)?;

    for (rank, hit) in response.hits.iter().enumerate() {
        let ranks = match (hit.lexical_rank, hit.dense_rank) {
            (Some(l), Some(d)) => format!("lex#{l} dense#{d}"),
            (Some(l), None) => format!("lex#{l}"),
            (None, Some(d)) => format!("dense#{d}"),
            (None, None) => "—".to_string(),
        };
        println!(
            "{}. [{}] {}  score={:.5}  {}",
            rank + 1,
            hit.unit.template,
            hit.unit.title,
            hit.score,
            ranks
        );
        println!("   {}", hit.unit.header);
        let preview: String = hit.unit.text.chars().take(110).collect();
        println!("   {preview}");
        println!(
            "   {} rev.{}  {}",
            hit.unit.page,
            hit.unit.revid.unwrap_or(0),
            folio.manifest().source_url(&hit.unit.page)
        );
        for neighbour in &hit.context {
            let preview: String = neighbour.text.chars().take(60).collect();
            println!("   ↳ {preview}");
        }
        println!();
    }

    let signals = &response.signals;
    let agreement = if signals.path_agreement.is_nan() {
        "n/a (one path)".to_string()
    } else {
        format!("{:.3}", signals.path_agreement)
    };
    println!(
        "confidence {:?}  top={:.5}  agreement={}  entropy={:.3}",
        signals.level, signals.top_score, agreement, signals.score_entropy
    );
    let trace = &response.trace;
    println!(
        "latency    lexical {}us  dense {}us  fuse {}us  fetch {}us  expand {}us  total {}us",
        trace.lexical_us, trace.dense_us, trace.fuse_us, trace.fetch_us, trace.expand_us,
        trace.total_us()
    );
    Ok(())
}

/// Default bench queries: a spread across templates and query shapes, so the distribution
/// is not dominated by one kind of lookup. Deliberately a mix of exact nouns, paraphrases
/// and multi-word phrases.
const DEFAULT_QUERIES: &[&str] = &[
    "源石技艺",
    "罗德岛的成立经过",
    "凯尔希 医疗部",
    "切城发生了什么",
    "感染者的处境",
    "叙拉古的家族",
    "阿米娅 任命助理",
    "天灾与源石的关系",
];

fn bench(path: &PathBuf, queries: Vec<String>, iterations: usize, top_k: usize) -> Result<()> {
    let baseline = mem::resident_bytes();

    let mut folio = Folio::open(path)?;
    let after_open = mem::resident_bytes();

    let has_vectors = folio.has_vectors()?;
    if has_vectors {
        folio.load_vectors()?;
    }
    let after_vectors = mem::resident_bytes();

    let idx = Index::new(&folio);
    let queries: Vec<String> = if queries.is_empty() {
        DEFAULT_QUERIES.iter().map(|q| q.to_string()).collect()
    } else {
        queries
    };

    println!("corpus     {} units", folio.manifest().unit_count);
    println!("vectors    {}", if has_vectors { "loaded" } else { "absent — lexical only" });
    println!("queries    {}  iterations {}\n", queries.len(), iterations);

    // One warm pass before measuring. A cold first query pays for page cache and prepared
    // statements; folding that into the distribution would report a number no user ever
    // experiences twice.
    for query in &queries {
        let _ = idx.search(
            &Request { query: query.clone(), mode: Mode::Lexical, top_k, ..Default::default() },
            None,
        )?;
    }

    let mut totals: Vec<u128> = Vec::with_capacity(iterations * queries.len());
    let mut lexical: Vec<u128> = Vec::with_capacity(totals.capacity());
    let started = Instant::now();

    for _ in 0..iterations {
        for query in &queries {
            let response = idx.search(
                &Request { query: query.clone(), mode: Mode::Lexical, top_k, ..Default::default() },
                None,
            )?;
            totals.push(response.trace.total_us());
            lexical.push(response.trace.lexical_us);
        }
    }

    let wall = started.elapsed();
    let peak = mem::peak_resident_bytes();

    report("lexical path", &mut lexical);
    report("end to end  ", &mut totals);
    println!(
        "\nthroughput {:.0} queries/s over {:.2}s",
        totals.len() as f64 / wall.as_secs_f64(),
        wall.as_secs_f64()
    );

    println!("\nresident memory");
    println!("  before open      {:>8.1} MB", baseline as f64 / 1e6);
    println!("  corpus open      {:>8.1} MB", after_open as f64 / 1e6);
    if has_vectors {
        println!("  vectors loaded   {:>8.1} MB", after_vectors as f64 / 1e6);
    }
    println!("  peak             {:>8.1} MB", peak as f64 / 1e6);
    println!(
        "\n  This is the retrieval layer alone. The budget it feeds into also carries the\n  \
         encoder and reranker servers, so compare against the daemon's share, not the whole."
    );
    Ok(())
}

fn report(label: &str, samples: &mut [u128]) {
    if samples.is_empty() {
        return;
    }
    samples.sort_unstable();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q) as usize];
    println!(
        "{label}  p50 {:>6}us  p95 {:>6}us  p99 {:>6}us  max {:>6}us",
        at(0.50),
        at(0.95),
        at(0.99),
        samples[samples.len() - 1]
    );
}
