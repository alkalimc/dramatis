//! Estimated values this crate reads, one serde struct per register group (`quota`,
//! `cost`, `ask`, `seed`, `session`, `wrapup`, `trust`). `Default` carries the register's
//! current values and is the only place they are written; use sites take the struct as
//! an argument. Missing keys in a loaded file fall back to these defaults.

use serde::{Deserialize, Serialize};

use crate::types::Tier;

/// A table with one value per tier that has quota windows (every tier but the one
/// without). Concrete per table so a partial table keeps the register's other values.
macro_rules! per_tier {
    ($(#[$doc:meta])* $name:ident [$low:expr, $middle:expr, $high:expr, $extra_high:expr, $max:expr]) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name {
            pub low: f64,
            pub middle: f64,
            pub high: f64,
            pub extra_high: f64,
            pub max: f64,
        }

        impl Default for $name {
            fn default() -> Self {
                Self { low: $low, middle: $middle, high: $high, extra_high: $extra_high, max: $max }
            }
        }

        impl $name {
            /// `None` for the tier without windows.
            pub fn get(&self, tier: Tier) -> Option<f64> {
                match tier {
                    Tier::Low => Some(self.low),
                    Tier::Middle => Some(self.middle),
                    Tier::High => Some(self.high),
                    Tier::ExtraHigh => Some(self.extra_high),
                    Tier::Max => Some(self.max),
                    Tier::Ultra => None,
                }
            }
        }
    };
}

per_tier! {
    /// `quota.window_1h[tier]`: calls per rolling hour.
    Window1h [40.0, 80.0, 160.0, 320.0, 640.0]
}

/// `quota.*`: the rolling hour of calls. A request-billed call is one call; a token-billed
/// call is its points over `call_points`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quota {
    pub window_1h: Window1h,
    /// Quota points that make one call.
    pub call_points: f64,
    /// Use fraction of the window at which unprompted speech stops.
    pub quiet_at: f64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            window_1h: Window1h::default(),
            call_points: 3000.0,
            quiet_at: 0.8,
        }
    }
}

/// `cost.*`: price ratios used when the endpoint has no prices configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cost {
    /// Cached input price / uncached input price.
    pub c: f64,
    /// Output price / uncached input price.
    pub o: f64,
}

impl Default for Cost {
    fn default() -> Self {
        Self { c: 0.25, o: 4.0 }
    }
}

/// `ask.*`: default turn budget of a request, per tier.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ask {
    pub turns: AskTurns,
}

/// Unlike the quota windows, the tier without windows has a value here too.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AskTurns {
    pub low: u32,
    pub middle: u32,
    pub high: u32,
    pub extra_high: u32,
    pub max: u32,
    pub ultra: u32,
}

impl AskTurns {
    pub fn get(&self, tier: Tier) -> u32 {
        match tier {
            Tier::Low => self.low,
            Tier::Middle => self.middle,
            Tier::High => self.high,
            Tier::ExtraHigh => self.extra_high,
            Tier::Max => self.max,
            Tier::Ultra => self.ultra,
        }
    }
}

impl Default for AskTurns {
    fn default() -> Self {
        Self {
            low: 6,
            middle: 10,
            high: 16,
            extra_high: 24,
            max: 32,
            ultra: 48,
        }
    }
}

/// `seed.*`: the experience limits on unprompted speech, the same on every tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seed {
    /// Unprompted openings per person per local day.
    pub per_agent_day: u32,
    /// Default quiet hours (user local time) until the user sets their own.
    pub quiet_hours: QuietHours,
}

/// Local wall-clock `HH:MM` pair; `from` after `to` spans midnight; equal = never quiet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    pub from: String,
    pub to: String,
}

impl Default for Seed {
    fn default() -> Self {
        Self {
            per_agent_day: 1,
            quiet_hours: QuietHours {
                from: "23:00".into(),
                to: "08:00".into(),
            },
        }
    }
}

/// What one tier sets of the pacing of unprompted life. A higher tier is a more frequent
/// life; the ceilings in [`Pace::clamped`] hold on every tier, the unlimited ones included.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pace {
    /// Unprompted openings across everyone per local day.
    pub daily_total: u32,
    /// Least minutes between two unprompted openings (and a daily event), whoever opens.
    pub gap_min: u32,
    /// Minutes between plannings of the daily seeds while the user is about and not in
    /// a conversation, apart from his returns.
    pub interval_min: u32,
    /// Days without the user talking to an enabled person (and without an opening of
    /// his) after which he may drop by with nothing in particular to say.
    pub visit_days: u32,
    /// Hours an enabled group may stay silent before one of its members speaks in it.
    pub group_quiet_hours: u32,
    /// Share of the high retrieval boundary that lets a member add a line to a group
    /// (lower = more often).
    pub interject_relax: f64,
    /// Daily events per local day until the user chooses another number.
    pub events_default: u32,
    /// The most daily events per day the user can choose.
    pub events_max: u32,
}

/// Most openings a day on any tier.
pub const VISITS_CEILING: u32 = 10;
/// Fewest minutes between two openings on any tier.
pub const GAP_FLOOR_MIN: u32 = 10;
/// Most daily events a day on any tier.
pub const EVENTS_CEILING: u32 = 6;
/// Lowest `interject_relax` on any tier.
pub const RELAX_FLOOR: f64 = 0.6;

impl Pace {
    const fn row(
        daily_total: u32,
        gap_min: u32,
        interval_min: u32,
        visit_days: u32,
        group_quiet_hours: u32,
        interject_relax: f64,
        events: (u32, u32),
    ) -> Self {
        Self {
            daily_total,
            gap_min,
            interval_min,
            visit_days,
            group_quiet_hours,
            interject_relax,
            events_default: events.0,
            events_max: events.1,
        }
    }

    /// The row held to the ceilings, whatever a configuration file says.
    pub fn clamped(mut self) -> Self {
        self.daily_total = self.daily_total.min(VISITS_CEILING);
        self.gap_min = self.gap_min.max(GAP_FLOOR_MIN);
        self.interval_min = self.interval_min.max(1);
        self.interject_relax = self.interject_relax.max(RELAX_FLOOR);
        self.events_max = self.events_max.min(EVENTS_CEILING);
        self.events_default = self.events_default.min(self.events_max);
        self
    }
}

impl Default for Pace {
    /// The Middle row.
    fn default() -> Self {
        Self::row(5, 15, 15, 2, 6, 0.8, (1, 4))
    }
}

/// `pacing[tier]`: the table of [`Pace`] rows, next to the quota windows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pacing {
    pub low: Pace,
    pub middle: Pace,
    pub high: Pace,
    pub extra_high: Pace,
    pub max: Pace,
    pub ultra: Pace,
}

impl Pacing {
    pub fn get(&self, tier: Tier) -> Pace {
        match tier {
            Tier::Low => self.low,
            Tier::Middle => self.middle,
            Tier::High => self.high,
            Tier::ExtraHigh => self.extra_high,
            Tier::Max => self.max,
            Tier::Ultra => self.ultra,
        }
        .clamped()
    }
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            low: Pace::row(2, 30, 30, 4, 12, 1.0, (1, 2)),
            middle: Pace::default(),
            high: Pace::row(6, 15, 15, 2, 5, 0.8, (2, 5)),
            extra_high: Pace::row(7, 12, 15, 1, 4, 0.75, (2, 6)),
            max: Pace::row(8, 10, 10, 1, 3, 0.7, (3, 6)),
            ultra: Pace::row(10, 10, 10, 1, 3, 0.7, (3, 6)),
        }
    }
}

/// `session.*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Minutes without a message after which a conversation has ended and a return
    /// counts as coming back.
    pub idle_timeout: u32,
}

impl Session {
    pub fn idle_timeout_ms(&self) -> i64 {
        i64::from(self.idle_timeout) * 60_000
    }
}

impl Default for Session {
    fn default() -> Self {
        Self { idle_timeout: 30 }
    }
}

/// `wrapup.*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wrapup {
    pub max_facts: u32,
}

impl Default for Wrapup {
    fn default() -> Self {
        Self { max_facts: 2 }
    }
}

/// `events.*`: the happenings the host posts about the day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Events {
    /// Past happenings a listing shows, and how far back the units used are avoided.
    pub list: u32,
    /// People offered to the model as those a happening may involve.
    pub pool: u32,
    /// Most people one happening involves.
    pub max_persons: u32,
    /// Minutes before another try when one made nothing.
    pub retry_min: u32,
}

impl Default for Events {
    fn default() -> Self {
        Self {
            list: 5,
            pool: 8,
            max_persons: 3,
            retry_min: 60,
        }
    }
}

/// `trust.*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trust {
    /// Growth per shared turn, before the diminishing factor.
    pub g1: f64,
    /// Growth per finished request, before the diminishing factor.
    pub g2: f64,
    /// Loss per hurt.
    pub h: f64,
    /// Lower bounds of `normal`, `close` and `deep`; below the first is `guarded`.
    pub bands: [u8; 3],
}

impl Default for Trust {
    fn default() -> Self {
        Self {
            g1: 0.5,
            g2: 2.0,
            h: 15.0,
            bands: [60, 100, 150],
        }
    }
}

/// Every group this crate reads, as `api` composes them into its one `Params`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub quota: Quota,
    pub cost: Cost,
    pub ask: Ask,
    pub seed: Seed,
    pub pacing: Pacing,
    pub session: Session,
    pub wrapup: Wrapup,
    pub events: Events,
    pub trust: Trust,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_sets_the_pacing_and_no_tier_breaks_the_ceilings() {
        let table = Pacing::default();
        let low = table.get(Tier::Low);
        let middle = table.get(Tier::Middle);
        let ultra = table.get(Tier::Ultra);
        assert_eq!(
            (
                low.daily_total,
                low.gap_min,
                low.interval_min,
                low.visit_days,
                low.group_quiet_hours
            ),
            (2, 30, 30, 4, 12)
        );
        assert_eq!(
            (low.events_default, low.events_max, low.interject_relax),
            (1, 2, 1.0)
        );
        assert_eq!(middle, Pace::default());
        assert_eq!(
            (
                ultra.daily_total,
                ultra.gap_min,
                ultra.visit_days,
                ultra.group_quiet_hours
            ),
            (10, 10, 1, 3)
        );
        assert_eq!(
            (
                ultra.events_default,
                ultra.events_max,
                ultra.interject_relax
            ),
            (3, 6, 0.7)
        );
        // More life as the tier goes up, never past the ceilings.
        let tiers = [
            Tier::Low,
            Tier::Middle,
            Tier::High,
            Tier::ExtraHigh,
            Tier::Max,
            Tier::Ultra,
        ];
        for pair in tiers.windows(2) {
            let (a, b) = (table.get(pair[0]), table.get(pair[1]));
            assert!(a.daily_total <= b.daily_total && a.gap_min >= b.gap_min);
            assert!(a.events_max <= b.events_max && a.interject_relax >= b.interject_relax);
        }
        for tier in tiers {
            let pace = table.get(tier);
            assert!(pace.daily_total <= VISITS_CEILING && pace.gap_min >= GAP_FLOOR_MIN);
            assert!(pace.events_max <= EVENTS_CEILING && pace.interject_relax >= RELAX_FLOOR);
        }
        // A configuration cannot lift them.
        let wild: Pacing = serde_json::from_str(
            r#"{"ultra": {"daily_total": 99, "gap_min": 1, "events_max": 40, "events_default": 40, "interject_relax": 0.1}}"#,
        )
        .unwrap();
        let ultra = wild.get(Tier::Ultra);
        assert_eq!(
            (
                ultra.daily_total,
                ultra.gap_min,
                ultra.events_max,
                ultra.events_default
            ),
            (
                VISITS_CEILING,
                GAP_FLOOR_MIN,
                EVENTS_CEILING,
                EVENTS_CEILING
            )
        );
        assert_eq!(ultra.interject_relax, RELAX_FLOOR);
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let p: Params = serde_json::from_str(
            r#"{"quota": {"quiet_at": 0.5, "window_1h": {"low": 10}}, "trust": {"h": 3}}"#,
        )
        .unwrap();
        assert_eq!(p.quota.quiet_at, 0.5);
        assert_eq!(p.quota.window_1h.low, 10.0);
        assert_eq!(p.quota.window_1h.middle, Window1h::default().middle);
        assert_eq!(p.quota.call_points, 3000.0);
        assert_eq!(p.trust.h, 3.0);
        assert_eq!(p.trust.g1, Trust::default().g1);
        assert_eq!(p.seed, Seed::default());
        assert_eq!(p.quota.window_1h.get(Tier::Ultra), None);
        assert_eq!(p.ask.turns.get(Tier::Ultra), 48);
    }
}
