//! Quota: rolling 5-hour and 7-day windows over the usage meter, and the three bands
//! of the tighter one. The agent records every call's usage; this module only keeps the
//! books and decides. The tier without windows has no bands at all.

use rusqlite::Connection;

use crate::params::{Cost, Quota};
use crate::types::{ChannelId, Shape, Tier};
use crate::{Error, Result};

pub const SPAN_5H_MS: i64 = 5 * 3_600_000;
pub const SPAN_7D_MS: i64 = 7 * 24 * 3_600_000;

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

/// Record one call. Points are fixed now, with the prices now in force.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Span {
    FiveHours,
    SevenDays,
}

impl Span {
    pub fn ms(self) -> i64 {
        match self {
            Self::FiveHours => SPAN_5H_MS,
            Self::SevenDays => SPAN_7D_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub span: Span,
    pub limit: f64,
    pub points: f64,
    /// `points / limit`, 0 and up.
    pub used: f64,
    /// When enough use rolls out of this window for it to leave its current band; `None`
    /// while it is open.
    pub release_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub tier: Tier,
    pub band: Band,
    /// Empty on the tier without windows.
    pub windows: Vec<Window>,
    /// The window deciding the band, when not open.
    pub binding: Option<Span>,
    /// The tighter window's use fraction (0 without windows).
    pub u: f64,
    /// When the band next improves: the binding window's release time.
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

/// Meter rows in `(now - span, now]`, oldest first.
fn rows(conn: &Connection, now: i64, span: i64) -> Result<Vec<(i64, f64)>> {
    let mut stmt =
        conn.prepare("SELECT at, points FROM meter WHERE at > ?1 AND at <= ?2 ORDER BY at")?;
    let rows = stmt.query_map((now - span, now), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The earliest instant at which the window's sum drops below `target`: rows leave the
/// window `span` after they were recorded.
fn release(rows: &[(i64, f64)], target: f64, span: i64) -> Option<i64> {
    let mut sum: f64 = rows.iter().map(|r| r.1).sum();
    if sum < target {
        return None;
    }
    for &(at, pts) in rows {
        sum -= pts;
        if sum < target {
            return Some(at + span);
        }
    }
    rows.last().map(|r| r.0 + span)
}

fn window(conn: &Connection, span: Span, limit: f64, now: i64, quota: &Quota) -> Result<Window> {
    let rows = rows(conn, now, span.ms())?;
    let pts: f64 = rows.iter().map(|r| r.1).sum();
    let used = if limit > 0.0 {
        pts / limit
    } else {
        f64::INFINITY
    };
    let target = match band_of(used, quota) {
        Band::Open => None,
        Band::Quiet => Some(limit * quota.quiet_at),
        Band::Exhausted => Some(limit),
    };
    Ok(Window {
        span,
        limit,
        points: pts,
        used,
        release_at: target.and_then(|t| release(&rows, t, span.ms())),
    })
}

pub fn status(conn: &Connection, tier: Tier, now: i64, quota: &Quota) -> Result<Status> {
    let (Some(l5), Some(l7)) = (quota.window_5h.get(tier), quota.window_7d.get(tier)) else {
        return Ok(Status {
            tier,
            band: Band::Open,
            windows: Vec::new(),
            binding: None,
            u: 0.0,
            release_at: None,
        });
    };
    let windows = vec![
        window(conn, Span::FiveHours, l5, now, quota)?,
        window(conn, Span::SevenDays, l7, now, quota)?,
    ];
    let tight = windows
        .iter()
        .max_by(|a, b| a.used.total_cmp(&b.used))
        .expect("two windows");
    let u = tight.used;
    let band = band_of(u, quota);
    // Leaving the band needs every window below its threshold: the latest release wins.
    let release_at = match band {
        Band::Open => None,
        _ => windows.iter().filter_map(|w| w.release_at).max(),
    };
    Ok(Status {
        tier,
        band,
        binding: (band != Band::Open).then_some(tight.span),
        windows,
        u,
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

/// Points per call shape within `(now - span, now]`, for the spending breakdown.
pub fn spent_by_shape(conn: &Connection, now: i64, span: Span) -> Result<Vec<(Shape, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT shape, sum(points) FROM meter WHERE at > ?1 AND at <= ?2 GROUP BY shape ORDER BY shape",
    )?;
    let rows = stmt.query_map((now - span.ms(), now), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Cached input as a share of all input over the span (`inject.cache_hit`); `None`
/// with no input.
pub fn cache_hit(conn: &Connection, now: i64, span: Span) -> Result<Option<f64>> {
    let (uncached, cached): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT sum(uncached), sum(cached) FROM meter WHERE at > ?1 AND at <= ?2",
        (now - span.ms(), now),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (u, c) = (uncached.unwrap_or(0) as f64, cached.unwrap_or(0) as f64);
    Ok((u + c > 0.0).then(|| c / (u + c)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

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

    fn small() -> Quota {
        let mut q = Quota::default();
        q.window_5h.middle = 100.0;
        q.window_7d.middle = 1000.0;
        q
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
    fn three_bands_and_release() {
        let w = world();
        let q = small();
        let s = |now| status(&w, Tier::Middle, now, &q).unwrap();
        assert_eq!(s(T0).band, Band::Open);
        spend(&w, T0, 50);
        spend(&w, T0 + HOUR, 35);
        let st = s(T0 + HOUR);
        assert_eq!((st.band, st.binding), (Band::Quiet, Some(Span::FiveHours)));
        // Quiet ends once the first 50 leave: 35 < 80.
        assert_eq!(st.release_at, Some(T0 + SPAN_5H_MS));
        assert_eq!(decide(&st, CallKind::UserInitiated), Decision::Allow);
        assert_eq!(decide(&st, CallKind::Opening), Decision::Quiet);
        assert_eq!(decide(&st, CallKind::Interjection), Decision::Quiet);
        assert_eq!(decide(&st, CallKind::HostLine), Decision::Quiet);
        spend(&w, T0 + 2 * HOUR, 20);
        let st = s(T0 + 2 * HOUR);
        assert_eq!(st.band, Band::Exhausted);
        assert_eq!(st.release_at, Some(T0 + SPAN_5H_MS), "105 - 50 = 55 < 100");
        assert!(matches!(
            decide(&st, CallKind::UserInitiated),
            Decision::Exhausted {
                release_at: Some(_)
            }
        ));
        assert_eq!(
            s(T0 + SPAN_5H_MS).band,
            Band::Open,
            "55 < 80 after the first leaves"
        );
        assert_eq!(s(T0 + 2 * HOUR + SPAN_5H_MS).band, Band::Open);
    }

    #[test]
    fn the_seven_day_window_can_bind() {
        let w = world();
        let mut q = small();
        q.window_7d.middle = 500.0;
        for d in 0..6 {
            spend(&w, T0 + d * DAY, 70);
        }
        let st = status(&w, Tier::Middle, T0 + 5 * DAY, &q).unwrap();
        assert_eq!((st.band, st.binding), (Band::Quiet, Some(Span::SevenDays)));
        assert!(st.release_at.unwrap() > T0 + 5 * DAY);
    }

    #[test]
    fn ultra_has_no_windows() {
        let w = world();
        spend(&w, T0, 1_000_000_000);
        let st = status(&w, Tier::Ultra, T0, &Quota::default()).unwrap();
        assert_eq!((st.band, st.windows.len()), (Band::Open, 0));
        assert!(
            can_call(&w, Tier::Ultra, CallKind::Opening, T0, &Quota::default())
                .unwrap()
                .allowed()
        );
    }

    #[test]
    fn breakdown_and_cache_hit() {
        let w = world();
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
        spend(&w, T0, 5);
        let by = spent_by_shape(&w, T0, Span::FiveHours).unwrap();
        assert_eq!(by, vec![(Shape::Direct, 5.0), (Shape::Wrapup, 17.5)]);
        assert_eq!(
            cache_hit(&w, T0, Span::FiveHours).unwrap(),
            Some(30.0 / 45.0)
        );
        assert_eq!(cache_hit(&w, T0 - DAY, Span::FiveHours).unwrap(), None);
    }
}
