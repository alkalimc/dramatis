//! Maintainer surface for a corpus.
//!
//! Four commands, and the last two are the reason this binary exists before the daemon does:
//!
//!   inspect   what does this corpus contain, and what does it require of a reader
//!   search    run the pipeline and show what came back
//!   bench     measure latency and resident memory on this machine
//!   evaluate  score retrieval against the structural benchmark, per family and per stratum
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
use index::{Index, Mode, Normalise, Request};

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
    /// Score retrieval against the structural benchmark.
    ///
    /// Two configurations are run over identical queries and compared with a paired
    /// bootstrap, because the question worth asking is not "how good is retrieval" — a
    /// number with no comparison is not interpretable — but "does alias resolution earn
    /// its place".
    Evaluate {
        /// The suite, as JSONL.
        #[arg(long, default_value = "../artifacts/arknights/evals/structural.queries.jsonl")]
        suite: PathBuf,
        #[arg(long, default_value_t = 10)]
        k: usize,
        #[arg(long, default_value_t = 50)]
        candidates: usize,
        /// Score a random subsample. 0 means the whole suite.
        #[arg(long, default_value_t = 0)]
        sample: usize,
        /// Run all three alias settings and compare them pairwise.
        #[arg(long)]
        compare_alias: bool,
        /// Write the machine-readable report here.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Write the markdown report here.
        #[arg(long)]
        markdown: Option<PathBuf>,
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
        Command::Evaluate {
            suite,
            k,
            candidates,
            sample,
            compare_alias,
            json,
            markdown,
        } => evaluate(
            &cli.folio,
            &suite,
            k,
            candidates,
            sample,
            compare_alias,
            json.as_deref(),
            markdown.as_deref(),
        ),
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

    if let Some(resolved) = &response.resolved {
        let how = match resolved.how {
            index::How::Unique => "别名",
            index::How::Qualified => "消歧限定",
        };
        let person = resolved.person.as_deref().unwrap_or("（非人物）");
        println!("{how}       {} → {}  person={person}\n", resolved.alias, resolved.target);
    }
    if let Some(ambiguity) = &response.ambiguous {
        // Reported rather than resolved. The site says this name means several things, so
        // the honest response is to answer broadly and say what the alternatives were.
        println!(
            "歧义       {} 可指 {} 个对象：{}",
            ambiguity.alias,
            ambiguity.candidates.len(),
            ambiguity.candidates.join(" · ")
        );
        println!("           查询未指明，下列结果未加限定\n");
    }

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

/// A deterministic subsample, stratified by family.
///
/// Uniform sampling would be wrong here: `disambig` holds ten queries out of 19,801, so a
/// 5% uniform sample would usually contain none of them and the report would silently drop
/// a family. Taking every n-th query within each family keeps all six present.
fn subsample(suite: eval::Suite, target: usize) -> eval::Suite {
    if target == 0 || target >= suite.queries.len() {
        return suite;
    }
    let mut by_family: std::collections::BTreeMap<String, Vec<eval::Query>> = Default::default();
    for query in suite.queries {
        by_family.entry(query.family.clone()).or_default().push(query);
    }
    let total: usize = by_family.values().map(Vec::len).sum();
    let mut kept = Vec::with_capacity(target);
    for queries in by_family.into_values() {
        // At least one per family, and never more than the family holds.
        let want = ((queries.len() as f64 / total as f64) * target as f64).round() as usize;
        let want = want.clamp(1, queries.len());
        let step = (queries.len() as f64 / want as f64).max(1.0);
        for i in 0..want {
            kept.push(queries[((i as f64 * step) as usize).min(queries.len() - 1)].clone());
        }
    }
    eval::Suite { queries: kept }
}

#[allow(clippy::too_many_arguments)]
fn evaluate(
    path: &PathBuf,
    suite_path: &PathBuf,
    k: usize,
    candidates: usize,
    sample: usize,
    compare_alias: bool,
    json_out: Option<&std::path::Path>,
    markdown_out: Option<&std::path::Path>,
) -> Result<()> {
    let folio = Folio::open(path).with_context(|| format!("opening {}", path.display()))?;
    let suite = eval::Suite::load(suite_path)
        .with_context(|| format!("loading {}", suite_path.display()))?;
    let full = suite.queries.len();
    let suite = subsample(suite, sample);

    println!("corpus     {} units", folio.manifest().unit_count);
    println!("suite      {} queries", suite.queries.len());
    if suite.queries.len() != full {
        println!("           (stratified subsample of {full}; every family kept)");
    }
    println!("families   {}\n", suite.families().join(" "));

    // The dense path needs an encoder that does not exist yet, so this measures the lexical
    // path. Saying so in the output matters more than it looks: a reader who finds these
    // numbers later must not mistake them for the full system's.
    let note = "词法路单独测量。稠密路需要编码器权重，尚不存在——本表不是完整系统的分数。"
        .to_string();

    let config = |normalise| eval::Config {
        k,
        candidates,
        mode: Mode::Lexical,
        normalise,
        ..eval::Config::default()
    };
    let configs: Vec<eval::Config> = if compare_alias {
        vec![
            config(Normalise::Off),
            config(Normalise::Expand),
            config(Normalise::ExpandAndFilter),
        ]
    } else {
        vec![config(Normalise::ExpandAndFilter)]
    };

    let progress = |done: usize, total: usize| {
        if done > 0 && done % 2500 == 0 {
            println!("  {done}/{total}");
        }
    };

    let mut runs: Vec<(eval::Config, eval::Outcome)> = Vec::new();
    for config in configs {
        println!("running {}", config.label());
        let outcome = eval::runner::run(&folio, &suite, &config, progress)?;
        runs.push((config, outcome));
    }

    let mut markdown = String::new();
    let mut reports = Vec::new();
    for (i, (config, outcome)) in runs.iter().enumerate() {
        let mut notes = vec![note.clone()];
        let mut paired = serde_json::Value::Null;
        if i > 0 {
            // Compare against the previous setting, not against the first: the question is
            // what each stage adds on top of the one before it, and comparing everything to
            // the baseline would credit filtering with expansion's gains.
            let (previous_config, previous) = &runs[i - 1];
            let (left, right) = eval::runner::align(&previous.scored, &outcome.scored);
            let p = eval::metrics::paired_bootstrap(&left, &right, 10_000);
            let delta = if left.is_empty() {
                0.0
            } else {
                right.iter().zip(&left).map(|(r, l)| r - l).sum::<f64>() / left.len() as f64
            };
            notes.push(match p {
                Some(p) => format!(
                    "相对 `{}`：nDCG@{k} 平均差 {delta:+.4}，配对 bootstrap p={p:.4}\
                     （10,000 次重采样，种子固定）",
                    previous_config.label()
                ),
                None => "配对比较不可用：两次运行没有共同查询。".to_string(),
            });
            paired = serde_json::json!({
                "against": previous_config.label(),
                "paired_queries": left.len(),
                "mean_ndcg_delta": delta,
                "p_value": p,
                "iterations": 10_000,
                "test": "paired bootstrap over per-query nDCG, fixed seed",
            });
        }
        let section = eval::report::markdown(config, outcome, &notes);
        print!("\n{section}");
        markdown.push_str(&section);
        markdown.push('\n');
        let mut payload = eval::report::json(config, outcome);
        if !paired.is_null() {
            payload["paired_vs_previous"] = paired;
        }
        reports.push(payload);
    }

    if let Some(out) = json_out {
        let payload = serde_json::json!({
            "suite": suite_path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
            "folio_fingerprint": folio.manifest().build_fingerprint,
            "queries_scored": suite.queries.len(),
            "queries_in_suite": full,
            "runs": reports,
        });
        std::fs::write(out, serde_json::to_string_pretty(&payload)? + "\n")?;
        println!("\nwrote {}", out.display());
    }
    if let Some(out) = markdown_out {
        std::fs::write(out, &markdown)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}
