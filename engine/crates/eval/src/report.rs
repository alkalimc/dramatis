//! Turning scores into something that can be quoted without laundering them.
//!
//! The report's shape enforces the suite's reporting rules. There is no code path that
//! prints a single headline nDCG for the suite, because there is no honest one: when one
//! family holds most of the queries, a micro-average is that family's score under a name
//! that implies all of them.

use std::fmt::Write as _;

use crate::metrics::{Aggregate, Scored, by_family, by_family_and_stratum, macro_average};
use crate::runner::{Config, Outcome};

fn row(name: &str, a: &Aggregate) -> String {
    format!(
        "| {name} | {} | {:.3} | {:.3} | {:.3} | {:.0} | {:.3} | {:.1}% | {} |",
        a.queries,
        a.ndcg,
        a.mrr,
        a.precision,
        a.gold_size,
        a.recall,
        a.negative_win_rate * 100.0,
        a.empty
    )
}

fn header(k: usize) -> String {
    format!(
        "| | Queries | nDCG@{k} | MRR | P@{k} | Relevant units | R@{k} | Negative wins | Empty |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    )
}

pub fn markdown(config: &Config, outcome: &Outcome, notes: &[String]) -> String {
    let scored: &[Scored] = &outcome.scored;
    let per_family = by_family(scored);
    let per_stratum = by_family_and_stratum(scored);
    let macro_avg = macro_average(&per_family);

    let k = config.k;
    let header = header(k);
    let mut out = String::new();
    let _ = writeln!(out, "## {}\n", config.label());
    let _ = writeln!(
        out,
        "Scored {}, unscoreable without gold {}; p50 {:.2} ms · p95 {:.2} ms · p99 {:.2} ms\n",
        scored.len(),
        outcome.unscoreable,
        outcome.latency_percentile(0.50) as f64 / 1000.0,
        outcome.latency_percentile(0.95) as f64 / 1000.0,
        outcome.latency_percentile(0.99) as f64 / 1000.0,
    );

    let _ = writeln!(
        out,
        "### By family (each family weighted equally)\n\n{header}"
    );
    for (family, aggregate) in &per_family {
        let _ = writeln!(out, "{}", row(family, aggregate));
    }
    let _ = writeln!(out, "{}", row("**Macro-average**", &macro_avg));
    let _ = writeln!(
        out,
        "\nThe macro-average is the only single number in this table that may be quoted, \
         and it is still a summary, not a conclusion."
    );

    let _ = writeln!(out, "\n### By family × stratum\n\n{header}");
    for (family, strata) in &per_stratum {
        for (stratum, aggregate) in strata {
            let _ = writeln!(out, "{}", row(&format!("{family} · {stratum}"), aggregate));
        }
    }
    let _ = writeln!(
        out,
        "\nThe verbatim stratum can be solved by exact matching; its score is a floor check, \
         not evidence of retrieval quality."
    );
    // The recall cap is computed from this run rather than quoted: the family with the
    // largest mean gold set is where R@k is most constrained by k.
    let widest = per_family
        .iter()
        .filter(|(_, a)| a.queries > 0 && a.gold_size > 0.0)
        .max_by(|x, y| x.1.gold_size.total_cmp(&y.1.gold_size));
    let cap_note = match widest {
        Some((family, a)) if a.gold_size > config.k as f64 => format!(
            "R@{k} is capped by the number of relevant units: a `{family}` query has {:.0} \
             relevant units on average, so {} slots can recall at most {:.0}%. ",
            a.gold_size,
            config.k,
            config.k as f64 / a.gold_size * 100.0
        ),
        _ => String::new(),
    };
    let _ = writeln!(
        out,
        "\n{cap_note}Read this table by nDCG@{k} and MRR; read R@{k} only alongside the \
         \"Relevant units\" column."
    );

    if !notes.is_empty() {
        let _ = writeln!(out, "\n### Notes\n");
        for note in notes {
            let _ = writeln!(out, "- {note}");
        }
    }
    out
}

/// Report keys for one run, one row each: `eval.questions`, `eval.baseline.macro_ndcg`
/// (from `baseline`, the run without the alias stage, when given) and per family
/// `eval.family.<family>.{ndcg,negwin,questions}`.
pub fn keys(config: &Config, outcome: &Outcome, baseline: Option<&Outcome>) -> String {
    let per_family = by_family(&outcome.scored);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "## Keys

From `{}`; nDCG at {}.

| Key | Value |
| --- | ---: |",
        config.label(),
        config.k
    );
    let _ = writeln!(out, "| `eval.questions` | {} |", outcome.scored.len());
    if let Some(baseline) = baseline {
        let macro_avg = macro_average(&by_family(&baseline.scored));
        let _ = writeln!(
            out,
            "| `eval.baseline.macro_ndcg` | {:.3} |",
            macro_avg.ndcg
        );
    }
    for (family, a) in &per_family {
        let _ = writeln!(out, "| `eval.family.{family}.ndcg` | {:.3} |", a.ndcg);
        let _ = writeln!(
            out,
            "| `eval.family.{family}.negwin` | {:.1}% |",
            a.negative_win_rate * 100.0
        );
        let _ = writeln!(out, "| `eval.family.{family}.questions` | {} |", a.queries);
    }
    out
}

/// The same numbers as JSON, for machine-readable result files and for anyone re-analysing
/// them.
pub fn json(config: &Config, outcome: &Outcome) -> serde_json::Value {
    let per_family = by_family(&outcome.scored);
    let macro_avg = macro_average(&per_family);

    let aggregate_json = |a: &Aggregate| {
        serde_json::json!({
            "queries": a.queries,
            "ndcg": a.ndcg,
            "precision": a.precision,
            "recall": a.recall,
            "mean_gold_size": a.gold_size,
            "mrr": a.mrr,
            "negative_win_rate": a.negative_win_rate,
            "empty": a.empty,
        })
    };

    let families: serde_json::Map<String, serde_json::Value> = per_family
        .iter()
        .map(|(name, a)| (name.clone(), aggregate_json(a)))
        .collect();

    let strata: serde_json::Map<String, serde_json::Value> = by_family_and_stratum(&outcome.scored)
        .iter()
        .map(|(family, inner)| {
            let by_stratum: serde_json::Map<String, serde_json::Value> = inner
                .iter()
                .map(|(stratum, a)| (stratum.clone(), aggregate_json(a)))
                .collect();
            (family.clone(), serde_json::Value::Object(by_stratum))
        })
        .collect();

    serde_json::json!({
        "config": {
            "label": config.label(),
            "k": config.k,
            "candidates": config.candidates,
            "mode": "lexical",
            "normalise": format!("{:?}", config.normalise).to_lowercase(),
        },
        "scored": outcome.scored.len(),
        "unscoreable": outcome.unscoreable,
        "latency_ms": {
            "p50": outcome.latency_percentile(0.50) as f64 / 1000.0,
            "p95": outcome.latency_percentile(0.95) as f64 / 1000.0,
            "p99": outcome.latency_percentile(0.99) as f64 / 1000.0,
        },
        "by_family": families,
        "by_family_and_stratum": strata,
        "macro_average": aggregate_json(&macro_avg),
        "reporting": "per family and per stratum; macro-average across families only",
    })
}
