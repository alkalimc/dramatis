//! Quota: one rolling hour of calls over the usage meter, and its three bands. A call is
//! the unit every endpoint shares: a request-billed call counts 1, a token-billed call its
//! points over `quota.call_points`. The agent records every call's usage; this module only
//! keeps the books and decides. The tier without a window has no bands at all.

use rusqlite::Connection;

use crate::params::{Cost, Quota};
use crate::types::{ChannelId, Shape, Tier};
use crate::{Error, Result};

pub const WINDOW_MS: i64 = 3_600_000;

/// Token counts of one call, from the endpoint's `usage`. Reasoning tokens count as
/// output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub uncached: u64,
    pub cached: u64,
    pub output: u64,
}

/// Configured endpoint prices, any unit, per token. Only the ratios matter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
}

/// The ratios `(c, o)`: from prices when configured (and usable), else `cost.c`, `cost.o`.
pub fn ratios(prices: Option<Prices>, cost: &Cost) -> (f64, f64) {
    match prices {
        Some(p) if p.input > 0.0 && p.cached_input >= 0.0 && p.output >= 0.0 => {
            (p.cached_input / p.input, p.output / p.input)
        }
        _ => (cost.c, cost.o),
    }
}

/// Quota points: one per uncached input token, `c` per cached one, `o` per output one.
pub fn points(usage: Usage, prices: Option<Prices>, cost: &Cost) -> f64 {
    let (c, o) = ratios(prices, cost);
    usage.uncached as f64 + c * usage.cached as f64 + o * usage.output as f64
}

/// Record one call. Points are fixed now, with the prices now in force. This is the
/// token-billed path; request-billed calls are recorded by the agent with `requests = 1`.
#[allow(clippy::too_many_arguments)]
pub fn record_usage(
    conn: &Connection,
    at: i64,
    shape: Shape,
    channel: Option<ChannelId>,
    model: &str,
    usage: Usage,
    prices: Option<Prices>,
    cost: &Cost,
) -> Result<f64> {
    let pts = points(usage, prices, cost);
    let n = |v: u64| i64::try_from(v).map_err(|_| Error::Invalid("token count overflow".into()));
    conn.execute(
        "INSERT INTO meter(at, shape, channel, model, uncached, cached, output, points)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            at,
            shape,
            channel,
            model,
            n(usage.uncached)?,
            n(usage.cached)?,
            n(usage.output)?,
            pts
        ],
    )?;
    Ok(pts)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    /// Below `quota.quiet_at`: everything as usual.
    Open,
    /// Unprompted speech and interjections are off; user-initiated calls proceed.
    Quiet,
    /// No new calls until the release time.
    Exhausted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub limit: f64,
    /// Calls made in the window.
    pub calls: f64,
    /// `calls / limit`, 0 and up.
    pub used: f64,
    /// When enough use rolls out of the window for it to leave its current band; `None`
    /// while it is open.
    pub release_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub tier: Tier,
    pub band: Band,
    /// `None` on the tier without a window.
    pub window: Option<Window>,
    /// The window's use fraction (0 without one).
    pub u: f64,
    /// When the band next improves.
    pub release_at: Option<i64>,
}

/// The kind of call about to be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// Anything the user started: a turn, a request and its steps, the host answering,
    /// the wrap-up of a conversation. Never blocked before the window is used up.
    UserInitiated,
    /// A person speaking first with a seed (a due commitment included).
    Opening,
    /// One more line in a group after the addressed reply.
    Interjection,
    /// The host's line about a non-empty digest on presence.
    HostLine,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Allow,
    /// Blocked by the quiet band: unprompted speech is off.
    Quiet,
    /// Blocked until `release_at`; the host explains with the pack's fixed line.
    Exhausted {
        release_at: Option<i64>,
    },
}

impl Decision {
    pub fn allowed(self) -> bool {
        self == Self::Allow
    }
}

fn band_of(u: f64, quota: &Quota) -> Band {
    if u >= 1.0 {
        Band::Exhausted
    } else if u >= quota.quiet_at {
        Band::Quiet
    } else {
        Band::Open
    }
}

/// What a meter row counts for in the window: one for a request-billed call, else its
/// points over `quota.call_points`. A SQL fragment over `meter`; `?3` is `call_points`.
const CALLS: &str = "CASE WHEN requests > 0 THEN 1.0 ELSE points / ?3 END";

/// The `call_points` divisor, kept off zero so a bad value cannot make a free call.
fn call_points(quota: &Quota) -> f64 {
    quota.call_points.max(1.0)
}

/// `(at, calls)` of the meter rows in `(now - WINDOW_MS, now]`, oldest first.
fn rows(conn: &Connection, now: i64, quota: &Quota) -> Result<Vec<(i64, f64)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT at, {CALLS} AS n FROM meter WHERE at > ?1 AND at <= ?2 AND n > 0 ORDER BY at"
    ))?;
    let rows = stmt.query_map((now - WINDOW_MS, now, call_points(quota)), |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The earliest instant at which the window's sum drops below `target`: rows leave the
/// window an hour after they were recorded.
fn release(rows: &[(i64, f64)], target: f64) -> Option<i64> {
    let mut sum: f64 = rows.iter().map(|r| r.1).sum();
    if sum < target {
        return None;
    }
    for &(at, n) in rows {
        sum -= n;
        if sum < target {
            return Some(at + WINDOW_MS);
        }
    }
    rows.last().map(|r| r.0 + WINDOW_MS)
}

pub fn status(conn: &Connection, tier: Tier, now: i64, quota: &Quota) -> Result<Status> {
    let Some(limit) = quota.window_1h.get(tier) else {
        return Ok(Status {
            tier,
            band: Band::Open,
            window: None,
            u: 0.0,
            release_at: None,
        });
    };
    let rows = rows(conn, now, quota)?;
    let calls: f64 = rows.iter().map(|r| r.1).sum();
    let used = if limit > 0.0 {
        calls / limit
    } else {
        f64::INFINITY
    };
    let band = band_of(used, quota);
    let target = match band {
        Band::Open => None,
        Band::Quiet => Some(limit * quota.quiet_at),
        Band::Exhausted => Some(limit),
    };
    let release_at = target.and_then(|t| release(&rows, t));
    Ok(Status {
        tier,
        band,
        window: Some(Window {
            limit,
            calls,
            used,
            release_at,
        }),
        u: used,
        release_at,
    })
}

/// May a call of this kind be made now?
pub fn can_call(
    conn: &Connection,
    tier: Tier,
    kind: CallKind,
    now: i64,
    quota: &Quota,
) -> Result<Decision> {
    Ok(decide(&status(conn, tier, now, quota)?, kind))
}

pub fn decide(status: &Status, kind: CallKind) -> Decision {
    match (status.band, kind) {
        (Band::Exhausted, _) => Decision::Exhausted {
            release_at: status.release_at,
        },
        (Band::Quiet, CallKind::UserInitiated) => Decision::Allow,
        (Band::Quiet, _) => Decision::Quiet,
        (Band::Open, _) => Decision::Allow,
    }
}

/// Calls per shape in the window ending at `now`, for the spending breakdown.
pub fn spent_by_shape(conn: &Connection, now: i64, quota: &Quota) -> Result<Vec<(Shape, f64)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT shape, sum({CALLS}) FROM meter WHERE at > ?1 AND at <= ?2 GROUP BY shape ORDER BY shape"
    ))?;
    let rows = stmt.query_map((now - WINDOW_MS, now, call_points(quota)), |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Metered totals of one call shape over a span.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShapeTotals {
    pub calls: u64,
    pub uncached: u64,
    pub cached: u64,
    pub output: u64,
    pub points: f64,
}

/// Totals per shape over `(since, until]`, in shape order.
pub fn totals_by_shape(
    conn: &Connection,
    since: i64,
    until: i64,
) -> Result<Vec<(Shape, ShapeTotals)>> {
    let mut stmt = conn.prepare(
        "SELECT shape, count(*), sum(uncached), sum(cached), sum(output), sum(points)
         FROM meter WHERE at > ?1 AND at <= ?2 GROUP BY shape ORDER BY shape",
    )?;
    let rows = stmt.query_map((since, until), |r| {
        let n = |i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
        Ok((
            r.get::<_, Shape>(0)?,
            ShapeTotals {
                calls: n(1)?,
                uncached: n(2)?,
                cached: n(3)?,
                output: n(4)?,
                points: r.get(5)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Cached input as a share of all input over the `span_ms` ending at `now`
/// (`inject.cache_hit`); `None` with no input.
pub fn cache_hit(conn: &Connection, now: i64, span_ms: i64) -> Result<Option<f64>> {
    let (uncached, cached): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT sum(uncached), sum(cached) FROM meter WHERE at > ?1 AND at <= ?2",
        (now - span_ms, now),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (u, c) = (uncached.unwrap_or(0) as f64, cached.unwrap_or(0) as f64);
    Ok((u + c > 0.0).then(|| c / (u + c)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    /// A token-billed call of `pts` points.
    fn spend(conn: &Connection, at: i64, pts: u64) {
        record_usage(
            conn,
            at,
            Shape::Direct,
            None,
            "m",
            Usage {
                uncached: pts,
                ..Usage::default()
            },
            None,
            &Cost::default(),
        )
        .unwrap();
    }

    /// A request-billed call: no points, one request.
    fn request(conn: &Connection, at: i64) {
        conn.execute(
            "INSERT INTO meter(at, shape, model, uncached, cached, output, points, requests)
             VALUES (?1, 'direct', 'r', 0, 0, 0, 0.0, 1)",
            [at],
        )
        .unwrap();
    }

    /// Middle tier at 10 calls an hour.
    fn few() -> Quota {
        let mut q = Quota::default();
        q.window_1h.middle = 10.0;
        q
    }

    fn call_pts(q: &Quota) -> u64 {
        q.call_points as u64
    }

    #[test]
    fn cost_formula_with_and_without_prices() {
        let u = Usage {
            uncached: 100,
            cached: 400,
            output: 10,
        };
        assert_eq!(
            points(u, None, &Cost::default()),
            100.0 + 0.25 * 400.0 + 4.0 * 10.0
        );
        let prices = Prices {
            input: 2.0,
            cached_input: 0.2,
            output: 8.0,
        };
        assert!((points(u, Some(prices), &Cost::default()) - (100.0 + 40.0 + 40.0)).abs() < 1e-9);
        let free = Prices {
            input: 0.0,
            ..prices
        };
        assert_eq!(ratios(Some(free), &Cost::default()), (0.25, 4.0));
    }

    #[test]
    fn a_token_call_of_call_points_counts_as_one() {
        let w = world();
        let q = few();
        spend(&w, T0, call_pts(&q));
        spend(&w, T0, call_pts(&q) / 2);
        let st = status(&w, Tier::Middle, T0, &q).unwrap();
        assert_eq!(st.window.unwrap().calls, 1.5);
    }

    #[test]
    fn token_and_request_calls_fill_the_same_window() {
        let w = world();
        let q = few();
        for i in 0..4 {
            request(&w, T0 + i);
        }
        for i in 0..3 {
            spend(&w, T0 + 10 + i, call_pts(&q));
        }
        let st = status(&w, Tier::Middle, T0 + 100, &q).unwrap();
        assert_eq!((st.band, st.window.unwrap().calls), (Band::Open, 7.0));
        spend(&w, T0 + 20, call_pts(&q));
        assert_eq!(
            status(&w, Tier::Middle, T0 + 100, &q).unwrap().band,
            Band::Quiet
        );
    }

    #[test]
    fn bands_and_release_an_hour_later() {
        let w = world();
        let q = few();
        let s = |now| status(&w, Tier::Middle, now, &q).unwrap();
        for i in 0..7 {
            request(&w, T0 + i * 1000);
        }
        assert_eq!(s(T0 + 10_000).band, Band::Open);
        request(&w, T0 + 7000);
        let st = s(T0 + 10_000);
        assert_eq!(st.band, Band::Quiet);
        // Quiet ends once enough calls leave: 7 left is below 8.
        assert_eq!(st.release_at, Some(T0 + HOUR));
        assert_eq!(decide(&st, CallKind::UserInitiated), Decision::Allow);
        assert_eq!(decide(&st, CallKind::Opening), Decision::Quiet);
        assert_eq!(decide(&st, CallKind::Interjection), Decision::Quiet);
        assert_eq!(decide(&st, CallKind::HostLine), Decision::Quiet);
        request(&w, T0 + 8000);
        spend(&w, T0 + 9000, call_pts(&q));
        let st = s(T0 + 10_000);
        assert_eq!(st.band, Band::Exhausted);
        assert_eq!(
            st.release_at,
            Some(T0 + HOUR),
            "the first row leaves, 9 < 10"
        );
        assert!(!decide(&st, CallKind::UserInitiated).allowed());
        assert_eq!(s(T0 + HOUR).band, Band::Quiet, "9 left, 9 >= 8");
        assert_eq!(s(T0 + HOUR + 9000).band, Band::Open);
    }

    #[test]
    fn ultra_has_no_window() {
        let w = world();
        spend(&w, T0, 1_000_000_000);
        let st = status(&w, Tier::Ultra, T0, &Quota::default()).unwrap();
        assert_eq!((st.band, st.window), (Band::Open, None));
        assert!(
            can_call(&w, Tier::Ultra, CallKind::Opening, T0, &Quota::default())
                .unwrap()
                .allowed()
        );
    }

    #[test]
    fn breakdown_and_cache_hit() {
        let w = world();
        let q = Quota::default();
        record_usage(
            &w,
            T0,
            Shape::Wrapup,
            None,
            "m",
            Usage {
                uncached: 10,
                cached: 30,
                output: 0,
            },
            None,
            &Cost::default(),
        )
        .unwrap();
        spend(&w, T0, 3000);
        request(&w, T0);
        let by = spent_by_shape(&w, T0, &q).unwrap();
        assert_eq!(
            by,
            vec![(Shape::Direct, 2.0), (Shape::Wrapup, 17.5 / 3000.0)]
        );
        assert_eq!(cache_hit(&w, T0, WINDOW_MS).unwrap(), Some(30.0 / 3040.0));
        assert_eq!(cache_hit(&w, T0 - DAY, WINDOW_MS).unwrap(), None);
    }
}
