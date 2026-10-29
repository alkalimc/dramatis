//! Every estimated value the app reads, composed from each crate's own group and loaded
//! from an optional `$DRAMATIS_HOME/params.toml` laid out like the register's dotted names
//! (`[retrieve.confidence] high_top1 = 9.5`). Missing keys keep the defaults.

use serde::Deserialize;

/// `media.*`: the local media cache (avatars and read-aloud audio together).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Media {
    /// Upper bound of `media/` in gigabytes; least recently used files go first.
    pub cache_gb: f64,
}

impl Default for Media {
    fn default() -> Self {
        Self { cache_gb: 3.0 }
    }
}

impl Media {
    pub fn cache_bytes(&self) -> u64 {
        (self.cache_gb.max(0.0) * 1e9) as u64
    }
}

/// `coldstart.*`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Coldstart {
    /// People on the first-run card.
    pub picks: usize,
}

impl Default for Coldstart {
    fn default() -> Self {
        Self { picks: 3 }
    }
}

/// `roster.*`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Roster {
    /// Trust at or above which someone talked to counts as a regular contact.
    pub close_trust: u8,
    /// Companions shown on a row or page.
    pub companions: usize,
}

impl Default for Roster {
    fn default() -> Self {
        Self {
            close_trust: 110,
            companions: 3,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Params {
    pub roster: Roster,
    pub retrieve: index::params::Retrieve,
    pub find_people: index::params::FindPeople,
    pub media: Media,
    pub coldstart: Coldstart,
    /// `quota`, `cost`, `ask`, `seed`, `session`, `wrapup`, `events`, `trust`, at the top level.
    #[serde(flatten)]
    pub world: world::params::Params,
}

impl Params {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// The file's values, or the defaults when it is absent. A malformed file is an
    /// error: silently ignoring an override would make a calibration run meaningless.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_keeps_the_other_defaults() {
        let p = Params::from_toml(
            "[retrieve.confidence]\nhigh_top1 = 1.5\n[media]\ncache_gb = 0.5\n\
             [quota]\nquiet_at = 0.5\n[seed.quiet_hours]\nfrom = '22:00'\nto = '07:00'\n",
        )
        .unwrap();
        assert_eq!(p.retrieve.confidence.high_top1, 1.5);
        assert_eq!(p.retrieve.top_k, index::params::Retrieve::default().top_k);
        assert_eq!(p.media.cache_bytes(), 500_000_000);
        assert_eq!(p.coldstart.picks, 3);
        let w = &p.world;
        assert_eq!(w.quota.quiet_at, 0.5);
        assert_eq!(w.seed.quiet_hours.from, "22:00");
        assert_eq!(w.trust, world::params::Trust::default());
        assert_eq!(Params::from_toml("").unwrap(), Params::default());
    }
}
