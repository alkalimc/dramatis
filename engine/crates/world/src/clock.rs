//! Time as callers pass it in, and the in-world date as a pure function of it.
//!
//! Nothing here reads the wall clock. A [`Now`] is a UTC instant plus the user's current
//! UTC offset; "today", quiet hours and birthdays are all local to the user.

use serde::{Deserialize, Serialize};

use crate::params::QuietHours;
use crate::types::MonthDay;

pub const MINUTE_MS: i64 = 60_000;
pub const HOUR_MS: i64 = 60 * MINUTE_MS;
pub const DAY_MS: i64 = 24 * HOUR_MS;

/// An instant (unix milliseconds) and the user's UTC offset at that instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Now {
    pub ms: i64,
    pub offset_min: i32,
}

/// A local calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Date {
    pub fn month_day(self) -> MonthDay {
        MonthDay {
            month: self.month,
            day: self.day,
        }
    }
}

/// The in-world calendar date: the local date with the year shifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InWorldDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Now {
    pub fn new(ms: i64, offset_min: i32) -> Self {
        Self { ms, offset_min }
    }

    pub fn utc(ms: i64) -> Self {
        Self { ms, offset_min: 0 }
    }

    /// The same offset at another instant.
    pub fn at(self, ms: i64) -> Self {
        Self { ms, ..self }
    }

    fn local_ms(self) -> i64 {
        self.ms + i64::from(self.offset_min) * MINUTE_MS
    }

    pub fn local_date(self) -> Date {
        civil_from_days(self.local_ms().div_euclid(DAY_MS))
    }

    /// Minutes since local midnight.
    pub fn local_minute(self) -> u32 {
        (self.local_ms().rem_euclid(DAY_MS) / MINUTE_MS) as u32
    }

    /// The UTC instant of the start of the local day, at this offset.
    pub fn local_day_start(self) -> i64 {
        self.ms - self.local_ms().rem_euclid(DAY_MS)
    }
}

/// The in-world date for a local instant. The year offset belongs to the corpus (its
/// manifest's `clock.year_offset`), never to this code.
pub fn in_world_date(now: Now, year_offset: i32) -> InWorldDate {
    let d = now.local_date();
    InWorldDate {
        year: d.year - year_offset,
        month: d.month,
        day: d.day,
    }
}

/// Local `HH:MM` for display.
pub fn local_time(now: Now) -> String {
    let m = now.local_minute();
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Whether the local time falls in `[from, to)`; `from > to` spans midnight, `from ==
/// to` is never. Unparsable bounds mean no quiet hours rather than always quiet.
pub fn in_quiet_hours(quiet: &QuietHours, now: Now) -> bool {
    let (Some(from), Some(to)) = (parse_hhmm(&quiet.from), parse_hhmm(&quiet.to)) else {
        return false;
    };
    let m = now.local_minute();
    match from.cmp(&to) {
        std::cmp::Ordering::Less => (from..to).contains(&m),
        std::cmp::Ordering::Greater => m >= from || m < to,
        std::cmp::Ordering::Equal => false,
    }
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(d: Date) -> i64 {
    let y = i64::from(d.year) - i64::from(d.month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(d.month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d.day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> Date {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    Date { year, month, day }
}

/// A due time as a model writes it: ISO 8601 `YYYY-MM-DD` (local midnight), or a
/// date-time `YYYY-MM-DDTHH:MM[:SS]` with an optional `Z` / `±HH:MM` (local when absent).
/// A space may stand for the `T`. `None` when it is not one of those.
pub fn parse_due(s: &str, now: Now) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = match s.find(['T', 't', ' ']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let mut parts = date.splitn(3, '-');
    let year: i32 = digits(parts.next()?, 4)?;
    let month: u8 = digits(parts.next()?, 2)?;
    let day: u8 = digits(parts.next()?, 2)?;
    MonthDay::parse(&format!("{month:02}-{day:02}"))?;
    let date = Date { year, month, day };
    if date.day == 29 && date.month == 2 && civil_from_days(days_from_civil(date)) != date {
        return None;
    }
    let mut offset = now.offset_min;
    let mut minutes = 0i64;
    if let Some(rest) = rest {
        let (time, zone) = match rest.find(['Z', 'z', '+', '-']) {
            Some(i) => (&rest[..i], Some(&rest[i..])),
            None => (rest, None),
        };
        let mut t = time.split(':');
        let h: i64 = digits(t.next()?, 2)?;
        let m: i64 = digits(t.next()?, 2)?;
        if let Some(sec) = t.next() {
            let whole = sec.split('.').next()?;
            let _: u8 = digits(whole, 2)?;
        }
        if t.next().is_some() || h > 23 || m > 59 {
            return None;
        }
        minutes = h * 60 + m;
        if let Some(zone) = zone {
            offset = match zone {
                "Z" | "z" => 0,
                _ => {
                    let sign = if zone.starts_with('-') { -1 } else { 1 };
                    let (zh, zm) = zone[1..].split_once(':').unwrap_or((&zone[1..], "00"));
                    let zh: i32 = digits(zh, 2)?;
                    let zm: i32 = digits(zm, 2)?;
                    if zh > 14 || zm > 59 {
                        return None;
                    }
                    sign * (zh * 60 + zm)
                }
            };
        }
    }
    Some(days_from_civil(date) * DAY_MS + minutes * MINUTE_MS - i64::from(offset) * MINUTE_MS)
}

fn digits<T: std::str::FromStr>(s: &str, len: usize) -> Option<T> {
    (s.len() == len && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for z in [-800_000, -1, 0, 1, 19_000, 20_000, 2_000_000] {
            assert_eq!(days_from_civil(civil_from_days(z)), z);
        }
        assert_eq!(
            civil_from_days(0),
            Date {
                year: 1970,
                month: 1,
                day: 1
            }
        );
        assert_eq!(
            days_from_civil(Date {
                year: 2000,
                month: 3,
                day: 1
            }),
            11_017
        );
    }

    #[test]
    fn in_world_date_shifts_only_the_year_and_uses_local_date() {
        // 2026-03-10 23:30 UTC is already 2026-03-11 at +08:00.
        let t = days_from_civil(Date {
            year: 2026,
            month: 3,
            day: 10,
        }) * DAY_MS
            + 23 * HOUR_MS
            + 30 * MINUTE_MS;
        let d = in_world_date(Now::new(t, 480), 7);
        assert_eq!(
            (d.year, d.month, d.day),
            (2019, 3, 11),
            "offset is a parameter"
        );
        let d = in_world_date(Now::utc(t), 0);
        assert_eq!((d.year, d.month, d.day), (2026, 3, 10));
        assert_eq!(local_time(Now::new(t, 480)), "07:30");
    }

    #[test]
    fn quiet_hours_across_midnight() {
        let q = QuietHours {
            from: "23:00".into(),
            to: "08:00".into(),
        };
        let at = |h: i64, m: i64| Now::utc(h * HOUR_MS + m * MINUTE_MS);
        assert!(in_quiet_hours(&q, at(23, 0)));
        assert!(in_quiet_hours(&q, at(3, 0)));
        assert!(!in_quiet_hours(&q, at(8, 0)));
        assert!(!in_quiet_hours(&q, at(22, 59)));
        let day = QuietHours {
            from: "13:00".into(),
            to: "14:00".into(),
        };
        assert!(in_quiet_hours(&day, at(13, 30)));
        assert!(!in_quiet_hours(&day, at(14, 0)));
        let never = QuietHours {
            from: "bad".into(),
            to: "08:00".into(),
        };
        assert!(!in_quiet_hours(&never, at(3, 0)));
    }

    #[test]
    fn day_start_is_local_midnight() {
        let now = Now::new(10 * DAY_MS + 2 * HOUR_MS, 180);
        assert_eq!(now.local_day_start(), 10 * DAY_MS - 3 * HOUR_MS);
        let now = Now::new(10 * DAY_MS + 2 * HOUR_MS, -180);
        assert_eq!(now.local_day_start(), 9 * DAY_MS + 3 * HOUR_MS);
    }

    #[test]
    fn parses_due_forms() {
        let now = Now::new(0, 60);
        let day = days_from_civil(Date {
            year: 2026,
            month: 1,
            day: 2,
        }) * DAY_MS;
        assert_eq!(parse_due("2026-01-02", now), Some(day - HOUR_MS));
        assert_eq!(
            parse_due("2026-01-02T15:30", now),
            Some(day + 14 * HOUR_MS + 30 * MINUTE_MS)
        );
        assert_eq!(
            parse_due("2026-01-02 15:30:00Z", now),
            Some(day + 15 * HOUR_MS + 30 * MINUTE_MS)
        );
        assert_eq!(
            parse_due("2026-01-02T15:30:00.5+08:00", now),
            Some(day + 7 * HOUR_MS + 30 * MINUTE_MS)
        );
        assert_eq!(
            parse_due("2026-01-02T01:00-02:00", now),
            Some(day + 3 * HOUR_MS)
        );
        for bad in [
            "tomorrow",
            "2026-13-01",
            "2026-02-30",
            "2025-02-29",
            "2026-1-2",
            "2026-01-02T25:00",
            "2026-01-02T10",
            "",
        ] {
            assert_eq!(parse_due(bad, now), None, "{bad}");
        }
        assert!(parse_due("2024-02-29", now).is_some());
    }
}
