//! The cost report: `inject.*` per call shape and `inject.cache_hit`, from the meter.
//!
//! `inject.<shape>` is the mean quota points of one call of that shape, the unit the
//! register's `cost.turn.*` estimates are in, so the two can be compared directly.

use rusqlite::Connection;
use world::quota::{self, Prices};
use world::{ChannelId, Shape};

use crate::params::Cost;

/// Record one call. The points are `world`'s formula plus a third term for tokens written
/// to a cache, `w` times the plain input price each; `world` has no such term yet, so the
/// meter row is inserted here. The written tokens are counted as uncached input, which
/// they are as far as the cache-hit share goes.
///
/// `write_ratio` is the model's configured cache-write price over its input price; absent,
/// the `cost.w` parameter stands in. A per-request model records its tokens but no points:
/// the row is flagged as one request, which the window counts as one call.
#[allow(clippy::too_many_arguments)]
pub fn record(
    conn: &Connection,
    at: i64,
    shape: Shape,
    channel: Option<ChannelId>,
    model: &str,
    usage: llm::Usage,
    prices: Option<Prices>,
    write_ratio: Option<f64>,
    per_request: bool,
    cost: &world::params::Cost,
    agent_cost: &Cost,
) -> world::Result<f64> {
    let base = quota::Usage {
        uncached: usage.uncached_in,
        cached: usage.cached_in,
        output: usage.output,
    };
    let w = write_ratio.unwrap_or(agent_cost.w);
    let points = if per_request {
        0.0
    } else {
        quota::points(base, prices, cost) + w * usage.cache_write_in as f64
    };
    let n =
        |v: u64| i64::try_from(v).map_err(|_| world::Error::Invalid("token count overflow".into()));
    conn.execute(
        "INSERT INTO meter(at, shape, channel, model, uncached, cached, output, points, requests)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            at,
            shape,
            channel,
            model,
            n(usage.uncached_in.saturating_add(usage.cache_write_in))?,
            n(usage.cached_in)?,
            n(usage.output)?,
            points,
            i64::from(per_request)
        ],
    )?;
    Ok(points)
}

/// The report key of a call shape.
pub fn key(shape: Shape) -> &'static str {
    match shape {
        Shape::Direct => "inject.direct",
        Shape::Group => "inject.group",
        Shape::Interject => "inject.interject",
        Shape::Opening => "inject.opening",
        Shape::Host => "inject.host",
        Shape::Wrapup => "inject.wrapup",
    }
}

/// `(key, value)` over `(until - span, until]`: mean points per call for each shape that
/// had calls, the cached share of all input, and the window's use.
pub fn report(
    conn: &Connection,
    until: i64,
    span_ms: i64,
    status: Option<&quota::Status>,
) -> world::Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    for (shape, t) in quota::totals_by_shape(conn, until - span_ms, until)? {
        if t.calls > 0 {
            out.push((key(shape).to_owned(), t.points / t.calls as f64));
        }
    }
    if let Some(hit) = quota::cache_hit(conn, until, span_ms)? {
        out.push(("inject.cache_hit".to_owned(), hit));
    }
    if let Some(s) = status.filter(|s| s.window.is_some()) {
        out.push(("quota.window_use".to_owned(), s.u));
    }
    Ok(out)
}
