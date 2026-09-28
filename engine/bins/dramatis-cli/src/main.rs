//! Maintainer surface for a corpus.
//!
//!   inspect    what does this corpus contain, and what does it require of a reader
//!   search     run the pipeline and show what came back
//!   people     who, by their own units, knows about a topic
//!   evaluate   score retrieval against the structural benchmark, per family and per stratum
//!   calibrate  fit the confidence bands on a slice of that benchmark
//!   bench      measure latency and resident memory on this machine
//!
//! `bench` is a measurement wearing a CLI: latency and resident memory are budgets that
//! cannot be settled on paper.

mod mem;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use folio::Folio;
use index::params::{FindPeople, Retrieve};
use index::{Filters, Index, Item, Normalise, RerankPolicy, SearchMode, SearchRequest};

/// Where the corpus is, when nobody said.
///
/// There is deliberately no default *path*: a relative path baked in here would have to
/// name the pack that produced the corpus, and this repository holds mechanism only —
/// naming the artifact means naming the subject matter. It would also be a guess about
/// someone's directory layout, since artifacts live outside every repository.
///
/// So the fallback is an environment variable, and its absence is an error with the fix
/// in it rather than a file-not-found on a path the user never chose.
fn default_folio() -> Result<PathBuf> {
    std::env::var_os("DRAMATIS_FOLIO")
        .map(PathBuf::from)
        .context("no corpus given: pass --folio <path>, or set DRAMATIS_FOLIO")
}

#[derive(Parser)]
#[command(
    name = "dramatis-cli",
    about = "Inspect, query and measure a .folio corpus"
)]
struct Cli {
    /// Path to the corpus. Defaults to $DRAMATIS_FOLIO.
    #[arg(long, short, global = true)]
    folio: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What this corpus contains, and what it requires of a reader.
    Inspect,
    /// Search, and show what came back with its confidence and trace.
    Search {
        query: String,
        /// Window size; 0 means `retrieve.top_k`.
        #[arg(long, default_value_t = 0)]
        top_k: usize,
        /// Rank for this person: tag each hit by where it sits in their knowledge.
        #[arg(long)]
        as_person: Option<String>,
        #[arg(long)]
        template: Vec<String>,
        #[arg(long)]
        person: Vec<String>,
        #[arg(long)]
        page: Vec<String>,
        /// Units already in the session.
        #[arg(long)]
        exclude: Vec<String>,
        /// auto | lexical | hybrid | dense.
        #[arg(long, default_value = "auto")]
        mode: String,
    },
    /// People whose own units match a topic.
    People {
        topic: String,
        /// Restrict to these people (a group's participants).
        #[arg(long)]
        among: Vec<String>,
        /// Never offer these people.
        #[arg(long)]
        exclude: Vec<String>,
        /// 0 means `find_people.k`.
        #[arg(long, default_value_t = 0)]
        k: usize,
    },
    /// Score retrieval against the structural benchmark.
    ///
    /// With `--compare-alias` the three alias settings run over identical queries and each
    /// is compared with the one before it by a paired bootstrap.
    Evaluate {
        /// The suite, as JSONL. Defaults to $DRAMATIS_SUITE.
        #[arg(long)]
        suite: Option<PathBuf>,
        /// Cut-off for every metric; 0 means `retrieve.top_k`.
        #[arg(long, default_value_t = 0)]
        k: usize,
        /// Score a stratified subsample. 0 means the whole suite.
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
    /// Fit `retrieve.confidence` on a stratified subset of the suite, then check the fitted
    /// bands on a disjoint subset.
    Calibrate {
        #[arg(long)]
        suite: Option<PathBuf>,
        /// Queries to fit on.
        #[arg(long, default_value_t = 2000)]
        sample: usize,
        /// Queries to check on, drawn from those not fitted on. 0 skips the check.
        #[arg(long, default_value_t = 4000)]
        holdout: usize,
        /// How often a `high` answer's top hit must be relevant.
        #[arg(long, default_value_t = 0.9)]
        high_precision: f64,
    },
    /// Measure the lexical path: latency distribution, throughput and resident memory.
    Bench {
        /// Queries to run. Defaults to suite queries when --suite is given, else to unit
        /// titles drawn from the corpus.
        #[arg(long)]
        query: Vec<String>,
        /// Draw this many queries from a suite (stratified by family).
        #[arg(long)]
        suite: Option<PathBuf>,
        #[arg(long, default_value_t = 400)]
        suite_queries: usize,
        #[arg(long, default_value_t = 20)]
        iterations: usize,
        /// Write BENCH.md here.
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let folio = match cli.folio {
        Some(path) => path,
        None => default_folio()?,
    };
    match cli.command {
        Command::Inspect => inspect(&folio),
        Command::Search {
            query,
            top_k,
            as_person,
            template,
            person,
            page,
            exclude,
            mode,
        } => {
            let request = SearchRequest {
                query,
                mode: parse_mode(&mode)?,
                top_k,
                filters: Filters {
                    templates: template,
                    persons: person,
                    pages: page,
                },
                as_person,
                exclude,
                rerank: RerankPolicy::Auto,
            };
            search(&folio, &request)
        }
        Command::People {
            topic,
            among,
            exclude,
            k,
        } => people(&folio, &topic, among, exclude, k),
        Command::Evaluate {
            suite,
            k,
            sample,
            compare_alias,
            json,
            markdown,
        } => evaluate(
            &folio,
            &suite_path(suite)?,
            k,
            sample,
            compare_alias,
            json.as_deref(),
            markdown.as_deref(),
        ),
        Command::Calibrate {
            suite,
            sample,
            holdout,
            high_precision,
        } => calibrate(&folio, &suite_path(suite)?, sample, holdout, high_precision),
        Command::Bench {
            query,
            suite,
            suite_queries,
            iterations,
            report,
        } => bench(
            &folio,
            query,
            suite.as_deref(),
            suite_queries,
            iterations,
            report.as_deref(),
        ),
    }
}

fn suite_path(given: Option<PathBuf>) -> Result<PathBuf> {
    match given {
        Some(path) => Ok(path),
        None => std::env::var_os("DRAMATIS_SUITE")
            .map(PathBuf::from)
            .context("no suite given: pass --suite <path>, or set DRAMATIS_SUITE"),
    }
}

fn parse_mode(mode: &str) -> Result<SearchMode> {
    Ok(match mode {
        "auto" => SearchMode::Auto,
        "lexical" => SearchMode::Lexical,
        "hybrid" => SearchMode::Hybrid,
        "dense" => SearchMode::Dense,
        other => anyhow::bail!("unknown mode {other:?}: auto | lexical | hybrid | dense"),
    })
}

fn open_index(path: &Path) -> Result<Index> {
    Index::open(path, Retrieve::default()).with_context(|| format!("opening {}", path.display()))
}

fn inspect(path: &Path) -> Result<()> {
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
    println!(
        "user name  {:?} (placeholder in stored text)",
        folio.user_placeholder()
    );

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
            "  {:<10} {:>7}  chars p50 {:>4}  p95 {:>4}  max {:>5}",
            stats.template, stats.count, stats.embed_p50, stats.embed_p95, stats.embed_max
        );
    }
    println!(
        "\nvectors    {}",
        if folio.has_vectors()? {
            "present (unused: this build has the lexical path only)"
        } else {
            "absent; the lexical path needs none"
        }
    );
    Ok(())
}

fn preview(text: &str, chars: usize) -> String {
    let flat = text.replace('\n', " ");
    let mut out: String = flat.chars().take(chars).collect();
    if flat.chars().count() > chars {
        out.push('…');
    }
    out
}

fn search(path: &Path, request: &SearchRequest) -> Result<()> {
    let index = open_index(path)?;
    let segmenter = index.segmenter();
    println!("segmenter  {} (from the corpus manifest)", segmenter.name());
    let all = segmenter.segment(&request.query);
    let queried = segmenter.segment_for_query(&request.query);
    println!("tokens     {all:?}");
    if queried != all {
        // Stopwords are dropped from queries only; the index keeps them. Showing both makes
        // the difference visible rather than something to discover from a score.
        println!("queried    {queried:?}  (stopwords dropped)");
    }
    println!();

    let response = index.search(request)?;
    if let Some(resolved) = &response.resolved {
        let how = match resolved.how {
            index::How::Unique => "alias     ",
            index::How::Qualified => "qualified ",
        };
        let person = resolved.person.as_deref().unwrap_or("(not a person)");
        println!(
            "{how} {} → {}  person={person}\n",
            resolved.alias, resolved.target
        );
    }
    if let Some(ambiguity) = &response.ambiguous {
        // Reported rather than resolved: the source says this name means several things.
        println!(
            "ambiguous  {} may refer to {} things: {}",
            ambiguity.alias,
            ambiguity.candidates.len(),
            ambiguity.candidates.join(" · ")
        );
        println!("           the query did not say which; results below are unrestricted\n");
    }

    let manifest = index.folio().manifest();
    for (rank, hit) in response.hits.iter().enumerate() {
        let source = hit
            .source
            .map(|s| format!("{s:?}").to_lowercase())
            .unwrap_or_else(|| "unscoped".into());
        match &hit.item {
            Item::Unit(unit) => {
                println!(
                    "{}. [{}] {}  score={:.3} (bm25 {:.3})  {source}",
                    rank + 1,
                    unit.template,
                    unit.title,
                    hit.score,
                    hit.unweighted
                );
                println!("   {}", unit.header);
                println!("   {}", preview(&unit.text, 110));
                println!(
                    "   {} rev.{}  {}  persons={:?}",
                    unit.page,
                    unit.revid.unwrap_or(0),
                    manifest.source_url(&unit.page),
                    unit.persons
                );
                for neighbour in &hit.neighbours {
                    println!("   ↳ {}", preview(&neighbour.text, 60));
                }
            }
            Item::Memory { id, text } => {
                println!(
                    "{}. [memory {id}] score={:.3}  {}",
                    rank + 1,
                    hit.score,
                    preview(text, 110)
                );
            }
        }
        println!();
    }
    if !response.excluded.is_empty() {
        println!("in session {}", response.excluded.join(" "));
    }

    let c = &response.confidence;
    println!(
        "confidence {:?}  top1={:.3}  entropy={:.3}",
        c.level, c.top1, c.entropy
    );
    let t = &response.trace;
    println!(
        "latency    normalise {}us  lexical {}us ({} candidates)  fetch {}us  scope {}us  \
         expand {}us  total {}us",
        t.normalise_us,
        t.lexical_us,
        t.lexical_candidates,
        t.fetch_us,
        t.scope_us,
        t.expand_us,
        t.total_us()
    );
    Ok(())
}

fn people(
    path: &Path,
    topic: &str,
    among: Vec<String>,
    exclude: Vec<String>,
    k: usize,
) -> Result<()> {
    let index = open_index(path)?;
    let k = if k == 0 { FindPeople::default().k } else { k };
    let scope = (!among.is_empty()).then_some(among.as_slice());
    let found = index.find_people(topic, scope, k, &exclude)?;
    for (rank, person) in found.persons.iter().enumerate() {
        println!(
            "{}. {}  bm25={:.3}  {:?}  own units matched {}",
            rank + 1,
            person.person,
            person.score,
            person.level,
            person.matched
        );
        println!(
            "   [{}] {}",
            person.reason.template,
            preview(&person.reason.text, 100)
        );
    }
    let c = &found.confidence;
    println!(
        "\nconfidence {:?}  top1={:.3}  entropy={:.3}",
        c.level, c.top1, c.entropy
    );
    Ok(())
}

/// How many queries to draw from the corpus when neither queries nor a suite are given.
const SAMPLED_QUERIES: usize = 8;

/// Draw bench queries from the corpus itself.
///
/// A list of terms written into this file cannot live here: naming things worth searching
/// for means naming the subject matter, and this repository holds mechanism only. One title
/// per template, then more from the head of the corpus. Deterministic, so two runs on one
/// corpus are comparable.
fn sampled_queries(folio: &Folio, want: usize) -> Result<Vec<String>> {
    const USABLE: &str = "title IS NOT NULL AND length(title) BETWEEN 4 AND 24";
    let mut out: Vec<String> = Vec::new();
    for sql in [
        format!("SELECT title FROM chunks WHERE {USABLE} GROUP BY template ORDER BY MIN(ord)"),
        format!("SELECT title FROM chunks WHERE {USABLE} ORDER BY ord"),
    ] {
        let mut stmt = folio.conn().prepare(&sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            if out.len() >= want {
                return Ok(out);
            }
            let title = row?;
            if !out.contains(&title) {
                out.push(title);
            }
        }
    }
    Ok(out)
}

/// Latency percentiles in microseconds, nearest rank.
struct Percentiles {
    p50: u128,
    p95: u128,
    p99: u128,
    max: u128,
}

impl Percentiles {
    fn of(samples: &mut [u128]) -> Self {
        samples.sort_unstable();
        let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
        Self {
            p50: at(0.50),
            p95: at(0.95),
            p99: at(0.99),
            max: samples[samples.len() - 1],
        }
    }
}

fn ms(us: u128) -> f64 {
    us as f64 / 1000.0
}

fn bench(
    path: &Path,
    queries: Vec<String>,
    suite: Option<&Path>,
    suite_queries: usize,
    iterations: usize,
    report: Option<&Path>,
) -> Result<()> {
    // Queries are read before the corpus is opened, and only their text is kept, so the
    // peak below is the retriever's rather than the suite parser's.
    let queries = match (queries.is_empty(), suite) {
        (false, _) => queries,
        (true, Some(suite)) => eval::suite::stratified_texts(suite, suite_queries)
            .with_context(|| format!("reading {}", suite.display()))?,
        (true, None) => Vec::new(),
    };
    let before = mem::resident_bytes();
    let started = Instant::now();
    let index = open_index(path)?;
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let after_open = mem::resident_bytes();
    let queries = if queries.is_empty() {
        sampled_queries(index.folio(), SAMPLED_QUERIES)?
    } else {
        queries
    };
    if queries.is_empty() {
        anyhow::bail!("no queries: pass --query or --suite, or use a corpus with titled units");
    }
    // The person with the most own units: the heaviest knowledge-scope lookup there is.
    let person: Option<String> = index
        .folio()
        .conn()
        .query_row(
            "SELECT person_id FROM unit_persons GROUP BY person_id \
             ORDER BY COUNT(*) DESC, person_id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let params = index.params().clone();
    println!(
        "corpus     {} units  (open {open_ms:.0} ms)",
        index.folio().manifest().unit_count
    );
    println!(
        "queries    {}  iterations {iterations}  top_k {}  candidates {}  neighbours {}\n",
        queries.len(),
        params.top_k,
        params.candidates,
        params.neighbours
    );

    let request = |query: &str, as_person: Option<&String>| SearchRequest {
        as_person: as_person.cloned(),
        ..SearchRequest::new(query)
    };
    // One warm pass: a cold first query pays for the page cache and statement preparation,
    // which no user pays twice.
    for query in &queries {
        index.search(&request(query, None))?;
        index.search(&request(query, person.as_ref()))?;
    }

    let mut lexical_stage = Vec::with_capacity(iterations * queries.len());
    let mut unscoped = Vec::with_capacity(lexical_stage.capacity());
    let mut scoped = Vec::with_capacity(lexical_stage.capacity());
    let wall = Instant::now();
    for _ in 0..iterations {
        for query in &queries {
            let started = Instant::now();
            let response = index.search(&request(query, None))?;
            unscoped.push(started.elapsed().as_micros());
            lexical_stage.push(response.trace.lexical_us);
        }
    }
    let wall = wall.elapsed();
    let searches = unscoped.len();
    if person.is_some() {
        for _ in 0..iterations {
            for query in &queries {
                let started = Instant::now();
                index.search(&request(query, person.as_ref()))?;
                scoped.push(started.elapsed().as_micros());
            }
        }
    }
    let peak = mem::peak_resident_bytes();
    let throughput = searches as f64 / wall.as_secs_f64();

    let lexical = Percentiles::of(&mut unscoped);
    let stage = Percentiles::of(&mut lexical_stage);
    let scoped = (!scoped.is_empty()).then(|| Percentiles::of(&mut scoped));
    let line = |label: &str, p: &Percentiles| {
        format!(
            "| {label} | {:.2} | {:.2} | {:.2} | {:.2} |",
            ms(p.p50),
            ms(p.p95),
            ms(p.p99),
            ms(p.max)
        )
    };

    let mut out = String::new();
    let _ = writeln!(out, "# Bench\n");
    let _ = writeln!(
        out,
        "Generated by `dramatis-cli bench --report`. Do not edit.\n\n\
         - corpus: `{}` · {} units · fingerprint `{}`\n\
         - {} queries × {iterations} iterations; `retrieve.top_k` {} · `retrieve.candidates` {} \
         · neighbours {} per side; warm (one untimed pass first)\n\
         - build: {} · {}\n",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default(),
        index.folio().manifest().unit_count,
        index.folio().manifest().build_fingerprint,
        queries.len(),
        params.top_k,
        params.candidates,
        params.neighbours,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::env::consts::ARCH,
    );
    let _ = writeln!(out, "| Key | Value |\n| --- | ---: |");
    let _ = writeln!(out, "| `bench.lexical.p50` | {:.2} ms |", ms(lexical.p50));
    let _ = writeln!(out, "| `bench.lexical.p95` | {:.2} ms |", ms(lexical.p95));
    let _ = writeln!(
        out,
        "| `bench.resident_peak` | {:.1} MB |",
        peak as f64 / 1e6
    );
    let _ = writeln!(out, "| `bench.throughput` | {throughput:.0} searches/s |");
    let _ = writeln!(
        out,
        "\n`bench.lexical.*` is one whole `search` call on the maintainer surface: alias stage, \
         FTS5, unit fetch and neighbour expansion.\n"
    );
    let _ = writeln!(
        out,
        "| Latency (ms) | p50 | p95 | p99 | max |\n| --- | ---: | ---: | ---: | ---: |"
    );
    let _ = writeln!(out, "{}", line("search, no person", &lexical));
    let _ = writeln!(out, "{}", line("FTS5 stage alone", &stage));
    if let Some(scoped) = &scoped {
        let _ = writeln!(
            out,
            "{}",
            line("search, as the person with most units", scoped)
        );
    }
    let _ = writeln!(
        out,
        "\n| Resident memory | MB |\n| --- | ---: |\n\
         | before opening | {:.1} |\n| corpus open, segmenter loaded | {:.1} |\n\
         | after the runs | {:.1} |\n| peak | {:.1} |",
        before as f64 / 1e6,
        after_open as f64 / 1e6,
        mem::resident_bytes() as f64 / 1e6,
        peak as f64 / 1e6
    );
    let _ = writeln!(
        out,
        "\nThe retrieval layer alone, in this process; the SQLite page cache is part of it."
    );
    print!("{out}");
    if let Some(report) = report {
        std::fs::write(report, &out).with_context(|| format!("writing {}", report.display()))?;
        println!("\nwrote {}", report.display());
    }
    Ok(())
}

/// The retrieval settings every evaluation runs with, so a suite score is a statement about
/// the retriever the product ships.
fn eval_config(k: usize, normalise: Normalise) -> eval::Config {
    eval::Config {
        k,
        candidates: Retrieve::default().candidates,
        normalise,
        ..eval::Config::default()
    }
}

fn load_suite(path: &Path, sample: usize) -> Result<(eval::Suite, usize)> {
    let suite = eval::Suite::load(path).with_context(|| format!("loading {}", path.display()))?;
    let full = suite.queries.len();
    Ok((suite.stratified(sample), full))
}

fn evaluate(
    path: &Path,
    suite_path: &Path,
    k: usize,
    sample: usize,
    compare_alias: bool,
    json_out: Option<&Path>,
    markdown_out: Option<&Path>,
) -> Result<()> {
    let mut index = open_index(path)?;
    let (suite, full) = load_suite(suite_path, sample)?;
    let k = if k == 0 { index.params().top_k } else { k };

    println!("corpus     {} units", index.folio().manifest().unit_count);
    println!("suite      {} queries", suite.queries.len());
    if suite.queries.len() != full {
        println!("           (stratified subsample of {full}; every family kept)");
    }
    println!("families   {}\n", suite.families().join(" "));

    let note = "Lexical path only: this build has no dense path or reranker, so this is \
                not the full system's score."
        .to_string();
    let configs: Vec<eval::Config> = if compare_alias {
        vec![
            eval_config(k, Normalise::Off),
            eval_config(k, Normalise::Expand),
            eval_config(k, Normalise::ExpandAndFilter),
        ]
    } else {
        vec![eval_config(k, Normalise::ExpandAndFilter)]
    };

    let progress = |done: usize, total: usize| {
        if done > 0 && done % 5000 == 0 {
            println!("  {done}/{total}");
        }
    };

    let mut runs: Vec<(eval::Config, eval::Outcome)> = Vec::new();
    for config in configs {
        println!("running {}", config.label());
        let outcome = eval::runner::run(&mut index, &suite, &config, progress)?;
        runs.push((config, outcome));
    }

    let manifest = index.folio().manifest();
    let mut markdown = format!(
        "# Results\n\nGenerated by `dramatis-cli evaluate --markdown`. Do not edit.\n\n\
         - corpus fingerprint `{}`\n- suite `{}`: {} of {full} queries scored\n\n",
        manifest.build_fingerprint,
        suite_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default(),
        suite.queries.len(),
    );
    let mut reports = Vec::new();
    for (i, (config, outcome)) in runs.iter().enumerate() {
        let mut notes = vec![note.clone()];
        let mut paired = serde_json::Value::Null;
        if i > 0 {
            // Each setting against the one before it: the question is what each stage adds
            // on top of the previous one.
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
                    "vs `{}`: mean nDCG@{k} difference {delta:+.4}, paired bootstrap \
                     p={p:.4} (10,000 resamples, fixed seed)",
                    previous_config.label()
                ),
                None => "Paired comparison unavailable: the two runs share no queries.".into(),
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
            "folio_fingerprint": manifest.build_fingerprint,
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

fn calibrate(
    path: &Path,
    suite_path: &Path,
    sample: usize,
    holdout: usize,
    high_precision: f64,
) -> Result<()> {
    let mut index = open_index(path)?;
    let suite = eval::Suite::load(suite_path)
        .with_context(|| format!("loading {}", suite_path.display()))?;
    let fit_on = suite.stratified(sample);
    let rest = suite.without(&fit_on).stratified(holdout);
    // The window confidence is read over in production, with the production alias stage.
    let config = eval_config(index.params().top_k, Normalise::ExpandAndFilter);

    println!("fitting on {} queries (stratified)", fit_on.queries.len());
    let samples = eval::calibrate::collect(&mut index, &fit_on, &config)?;
    let fit = eval::calibrate::fit(&samples, high_precision)
        .context("nothing to calibrate on: no query matched anything")?;
    let b = &fit.bands;
    println!("\n[retrieve.confidence]");
    println!("low_top1 = {:.3}", b.low_top1);
    println!("high_top1 = {:.3}", b.high_top1);
    println!("high_entropy = {:.3}", b.high_entropy);
    println!("\nYouden's J at low_top1: {:.3}", fit.low_separation);
    let show = |label: &str, a: &eval::calibrate::Assessment| {
        println!(
            "{label} ({} queries, families weighted equally): high {:.1}% right {:.1}% · \
             medium {:.1}% right {:.1}% · low {:.1}% right {:.1}%",
            a.samples,
            a.high_share * 100.0,
            a.high_answered * 100.0,
            a.medium_share * 100.0,
            a.medium_answered * 100.0,
            a.low_share * 100.0,
            a.low_answered * 100.0
        );
    };
    show("fit    ", &fit.fit);
    if !rest.queries.is_empty() {
        let held = eval::calibrate::collect(&mut index, &rest, &config)?;
        show("holdout", &eval::calibrate::assess(&held, b));
    }
    Ok(())
}
