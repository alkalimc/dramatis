//! Identifiers and enums shared by every area, with their SQLite encodings.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

macro_rules! row_id {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                i64::column_result(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

row_id! {
    ChannelId,
    MessageId,
    TaskId,
    FactId,
    SegmentId,
}

/// A roster id from the corpus; stable across corpus versions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PersonId(pub String);

impl PersonId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PersonId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for PersonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ToSql for PersonId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for PersonId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value).map(Self)
    }
}

/// Who wrote something: `'user'`, `'host'` or a person id in the database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Actor {
    User,
    Host,
    Person(PersonId),
}

impl Actor {
    pub fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Host => "host",
            Self::Person(p) => p.as_str(),
        }
    }

    pub fn person(&self) -> Option<&PersonId> {
        match self {
            Self::Person(p) => Some(p),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "user" => Self::User,
            "host" => Self::Host,
            _ => Self::Person(PersonId(s.to_owned())),
        }
    }
}

impl From<PersonId> for Actor {
    fn from(p: PersonId) -> Self {
        Self::Person(p)
    }
}

impl ToSql for Actor {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for Actor {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str().map(Self::parse)
    }
}

/// A lowercase-text enum column under a CHECK.
macro_rules! text_enum {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $variant:ident = $text:literal),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vdoc])* $variant),*
        }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),*
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($text => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.as_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let s = value.as_str()?;
                Self::parse(s).ok_or_else(|| FromSqlError::Other(
                    format!("unknown {} `{s}`", stringify!($name)).into(),
                ))
            }
        }
    };
}

text_enum! {
    /// Whether a person or a group may act without being addressed.
    Mode {
        /// May speak unprompted, with a seed, within the daily limits.
        Enabled = "enabled",
        /// Never speaks first; still answers when triggered. Everyone starts here.
        Frozen = "frozen",
        /// Cannot be triggered and never appears in any option offered to a model.
        Disabled = "disabled",
    }
}

impl Default for Mode {
    fn default() -> Self {
        Self::Frozen
    }
}

text_enum! {
    ChannelKind {
        Direct = "direct",
        Group = "group",
    }
}

text_enum! {
    Origin {
        User = "user",
        System = "system",
    }
}

text_enum! {
    FactKind {
        Fact = "fact",
        Commitment = "commitment",
        Conclusion = "conclusion",
        Hurt = "hurt",
    }
}

text_enum! {
    TaskStatus {
        Active = "active",
        Done = "done",
    }
}

text_enum! {
    /// The call shapes usage is recorded under.
    Shape {
        Direct = "direct",
        Group = "group",
        Interject = "interject",
        Opening = "opening",
        Ask = "ask",
        Host = "host",
        Wrapup = "wrapup",
    }
}

text_enum! {
    /// The intensity slider, the only quota control.
    Tier {
        Low = "low",
        Middle = "middle",
        High = "high",
        ExtraHigh = "extra_high",
        Max = "max",
        /// No windows; only the daily experience limits apply.
        Ultra = "ultra",
    }
}

impl Default for Tier {
    fn default() -> Self {
        Self::Middle
    }
}

/// Portable pointer into the corpus. Chunk ids are content hashes and change when a page
/// changes, so the page and span travel with them for re-resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub page: String,
    pub revid: u32,
    pub span_from: u32,
    pub span_to: u32,
    pub chunk_id: String,
}

/// A month and day, as birthdays are stored (`"MM-DD"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonthDay {
    pub month: u8,
    pub day: u8,
}

impl MonthDay {
    /// `"MM-DD"`; `None` when malformed or out of range.
    pub fn parse(s: &str) -> Option<Self> {
        let (m, d) = s.split_once('-')?;
        if m.len() != 2 || d.len() != 2 {
            return None;
        }
        let md = Self {
            month: m.parse().ok()?,
            day: d.parse().ok()?,
        };
        let max = match md.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => 29,
            _ => return None,
        };
        (1..=max).contains(&md.day).then_some(md)
    }
}

impl fmt::Display for MonthDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}-{:02}", self.month, self.day)
    }
}

impl Serialize for MonthDay {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MonthDay {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad MM-DD `{s}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_day_round_trips_and_rejects_nonsense() {
        let md = MonthDay::parse("02-29").unwrap();
        assert_eq!(md.to_string(), "02-29");
        for bad in ["2-3", "13-01", "04-31", "00-10", "ab-cd", ""] {
            assert_eq!(MonthDay::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn actor_encoding() {
        for a in [Actor::User, Actor::Host, Actor::Person("p1".into())] {
            assert_eq!(Actor::parse(a.as_str()), a);
        }
        assert_eq!(Tier::parse("extra_high"), Some(Tier::ExtraHigh));
        assert_eq!(Mode::default(), Mode::Frozen);
    }
}
