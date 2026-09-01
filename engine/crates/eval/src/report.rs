//! Turning scores into something that can be pasted into a paper without laundering them.
//!
//! The report's shape enforces the suite's reporting rules. There is no code path that
//! prints a single headline nDCG for the suite, because there is no honest one: `voice`
//! holds 16,124 of 19,801 queries, so a micro-average is that family's score under a name
//! that implies six.

use std::fmt::Write as _;

use crate::metrics::{by_family, by_family_and_stratum, macro_average, Aggregate, Scored};
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

const HEADER: &str = "| | 查询 | nDCG@10 | MRR | P@10 | 相关单元 | R@10 | 负例胜出 | 空结果 |\n\
     | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |";

pub fn markdown(config: &Config, outcome: &Outcome, notes: &[String]) -> String {
    let scored: &[Scored] = &outcome.scored;
    let per_family = by_family(scored);
    let per_stratum = by_family_and_stratum(scored);
    let macro_avg = macro_average(&per_family);

    let mut out = String::new();
    let _ = writeln!(out, "## {}\n", config.label());
    let _ = writeln!(
        out,
        "评分 {} 条，无 gold 不可评分 {} 条；p50 {:.2} ms · p95 {:.2} ms · p99 {:.2} ms\n",
        scored.len(),
        outcome.unscoreable,
        outcome.latency_percentile(0.50) as f64 / 1000.0,
        outcome.latency_percentile(0.95) as f64 / 1000.0,
        outcome.latency_percentile(0.99) as f64 / 1000.0,
    );

    let _ = writeln!(out, "### 按族（每族等权）\n\n{HEADER}");
    for (family, aggregate) in &per_family {
        let _ = writeln!(out, "{}", row(family, aggregate));
    }
    let _ = writeln!(out, "{}", row("**宏平均**", &macro_avg));
    let _ = writeln!(
        out,
        "\n宏平均是本表唯一可引用的单一数字，且它仍是摘要而非结论。"
    );

    let _ = writeln!(out, "\n### 按族 × 层\n\n{HEADER}");
    for (family, strata) in &per_stratum {
        for (stratum, aggregate) in strata {
            let _ = writeln!(out, "{}", row(&format!("{family} · {stratum}"), aggregate));
        }
    }
    let _ = writeln!(
        out,
        "\nverbatim 层可由精确匹配解出，其分数是下限校验，不是检索质量的证据。\n\n\
         R@10 受相关单元总数封顶：`redirect` 的一条查询平均有 55 个相关单元，10 个槽位\
         最多召回 18%。因此本表以 nDCG@10 与 MRR 为准，R@10 只与「相关单元」一栏同读。"
    );

    if !notes.is_empty() {
        let _ = writeln!(out, "\n### 说明\n");
        for note in notes {
            let _ = writeln!(out, "- {note}");
        }
    }
    out
}

/// The same numbers as JSON, for `probe-results.json` and for anyone re-analysing them.
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
            "mode": format!("{:?}", config.mode).to_lowercase(),
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
