//! The cost report: `inject.*` per call shape and `inject.cache_hit`, from the meter.
//!
//! `inject.<shape>` is the mean quota points of one call of that shape, the unit the
//! register's `cost.turn.*` estimates are in, so the two can be compared directly.

use rusqlite::Connection;
use world::Shape;
use world::quota::{self, Span};

use crate::store;

/// The report key of a call shape.
pub fn key(shape: Shape) -> &'static str {
    match shape {
        Shape::Direct => "inject.direct",
        Shape::Group => "inject.group",
        Shape::Interject => "inject.interject",
        Shape::Opening => "inject.opening",
        Shape::Ask => "inject.ask",
        Shape::Host => "inject.host",
        Shape::Wrapup => "inject.wrapup",
    }
}

/// `(key, value)` over `(until - span, until]`: mean points per call for each shape that
/// had calls, the cached share of all input, and the tighter window's use.
pub fn report(
    conn: &Connection,
    until: i64,
    span: Span,
    status: Option<&quota::Status>,
) -> world::Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    for (shape, t) in store::totals_by_shape(conn, until - span.ms(), until)? {
        if t.calls > 0 {
            out.push((key(shape).to_owned(), t.points / t.calls as f64));
        }
    }
    if let Some(hit) = quota::cache_hit(conn, until, span)? {
        out.push(("inject.cache_hit".to_owned(), hit));
    }
    if let Some(s) = status.filter(|s| !s.windows.is_empty()) {
        out.push(("quota.window_use".to_owned(), s.u));
    }
    Ok(out)
}
