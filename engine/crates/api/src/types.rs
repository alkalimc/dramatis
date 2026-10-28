//! Values that cross the seam. Everything here is data: no user-visible sentence is ever
//! composed on this side. The UI renders i18n keys and the pack's wording; strings in
//! these types are names, quotes and corpus text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

macro_rules! id {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    )*};
}

id! {
    /// A roster id from the corpus; stable across corpus versions.
    PersonId,
    ChannelId,
    MessageId,
    TaskId,
    FactId,
}

/// RFC 3339 with offset (`2026-01-02T03:04:05+08:00`). Strings rather than integers: the
/// TypeScript side has no lossless 64-bit integer, and a date string needs no glue.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct Timestamp(pub String);

/// A file on this machine the UI may display or play (avatar, note, audio, attachment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MediaFile {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    User,
    /// The in-world assistant.
    Host,
    Person {
        id: PersonId,
    },
}

/// A person as a list row needs them: id plus the corpus display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PersonRef {
    pub id: PersonId,
    pub name: String,
    /// `None` when the corpus does not say; screens then use the neutral pronoun.
    #[serde(default)]
    #[specta(optional)]
    pub pronoun: Option<Pronoun>,
}

/// Which pronoun a sentence about a person takes. The pack names the facet that carries
/// it and the facet values of each class (wording `gender.facet`, `gender.female`,
/// `gender.male`: values separated by `|`); everyone else has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Pronoun {
    Female,
    Male,
}

impl Pronoun {
    pub fn of(
        wording: &std::collections::BTreeMap<String, String>,
        facets: &std::collections::BTreeMap<String, String>,
    ) -> Option<Self> {
        let value = facets.get(wording.get("gender.facet")?)?.trim();
        let listed = |key: &str| {
            wording
                .get(key)
                .is_some_and(|vals| vals.split('|').any(|v| v.trim() == value))
        };
        if listed("gender.female") {
            Some(Self::Female)
        } else if listed("gender.male") {
            Some(Self::Male)
        } else {
            None
        }
    }
}

/// Whether a person or a group may act without being addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// May speak unprompted (with a seed, within the daily limits).
    Enabled,
    /// Never speaks first; still answers when triggered. Everyone starts here.
    #[default]
    Frozen,
    /// Cannot be triggered and never appears in any option offered to a model.
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Person { id: PersonId },
    Channel { id: ChannelId },
}

/// The intensity slider: the only quota control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Low,
    #[default]
    Middle,
    High,
    ExtraHigh,
    Max,
    /// No windows; only the daily experience limits apply.
    Ultra,
}

/// Portable pointer into the corpus. Chunk ids are content hashes and change when a page
/// changes, so the page and span travel with them for re-resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Citation {
    pub page: String,
    pub revid: u32,
    pub span_from: u32,
    pub span_to: u32,
    pub chunk_id: String,
}

/// How much the corpus supports an answer, as retrieval measured it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[cfg(test)]
mod tests {
    use super::Pronoun;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn pack_values_pick_the_class() {
        let wording = map(&[
            ("gender.facet", "sex"),
            ("gender.female", "f|woman"),
            ("gender.male", "m"),
        ]);
        let of = |facets| Pronoun::of(&wording, &map(facets));
        assert_eq!(of(&[("sex", "woman")]), Some(Pronoun::Female));
        assert_eq!(of(&[("sex", "m")]), Some(Pronoun::Male));
        assert_eq!(of(&[("sex", "unknown")]), None);
        assert_eq!(of(&[]), None);
        // A pack that declares nothing yields the no class.
        assert_eq!(Pronoun::of(&map(&[]), &map(&[("sex", "m")])), None);
    }
}
