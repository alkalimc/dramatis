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
    /// `quota.window_5h[tier]`.
    Window5h [150_000.0, 400_000.0, 1_000_000.0, 2_500_000.0, 6_000_000.0]
}

per_tier! {
    /// `quota.window_7d[tier]`.
    Window7d [750_000.0, 2_000_000.0, 5_000_000.0, 12_000_000.0, 30_000_000.0]
}

/// `quota.*`: rolling windows in quota points (uncached-input-token equivalents).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quota {
    pub window_5h: Window5h,
    pub window_7d: Window7d,
    /// Use fraction of the tighter window at which unprompted speech stops.
    pub quiet_at: f64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            window_5h: Window5h::default(),
            window_7d: Window7d::default(),
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
    /// Unprompted openings across everyone per local day.
    pub daily_total: u32,
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
            daily_total: 3,
            quiet_hours: QuietHours {
                from: "23:00".into(),
                to: "08:00".into(),
            },
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
    pub session: Session,
    pub wrapup: Wrapup,
    pub trust: Trust,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let p: Params = serde_json::from_str(
            r#"{"quota": {"quiet_at": 0.5, "window_5h": {"low": 10}}, "trust": {"h": 3}}"#,
        )
        .unwrap();
        assert_eq!(p.quota.quiet_at, 0.5);
        assert_eq!(p.quota.window_5h.low, 10.0);
        assert_eq!(p.quota.window_5h.middle, Window5h::default().middle);
        assert_eq!(p.quota.window_7d, Window7d::default());
        assert_eq!(p.trust.h, 3.0);
        assert_eq!(p.trust.g1, Trust::default().g1);
        assert_eq!(p.seed, Seed::default());
        assert_eq!(p.quota.window_5h.get(Tier::Ultra), None);
        assert_eq!(p.ask.turns.get(Tier::Ultra), 48);
    }
}
