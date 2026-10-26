//! The agent's errors, and how they cross the seam as [`api::ApiError`] codes.

use api::error::ApiError;
use api::types::Timestamp;
use world::PersonId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    World(#[from] world::Error),

    #[error(transparent)]
    Index(#[from] index::Error),

    #[error(transparent)]
    Folio(#[from] folio::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// No persona in the corpus: such a person never gets a session, like a disabled one.
    #[error("person `{0}` has no persona")]
    NoPersona(PersonId),

    /// Quota band three. `release_at` is unix milliseconds.
    #[error("quota exhausted")]
    QuotaExhausted { release_at: Option<i64> },

    /// No chat role, or it names a profile that does not exist.
    #[error("no chat endpoint configured")]
    NoEndpoint,

    /// The endpoint answered with an error, or the stream broke. `detail` is the
    /// endpoint's own text; it never contains the key.
    #[error("endpoint: {detail}")]
    Endpoint { detail: String },

    #[error("keychain: {0}")]
    Keychain(String),

    /// The configured endpoint speaks a protocol this build cannot render yet.
    #[error("{0} is not supported by this build")]
    Unsupported(&'static str),

    /// A forced tool choice on an endpoint that must not get one. The caller asks for the
    /// tool in its text and keeps `auto`.
    #[error("a forced tool choice is not allowed on this endpoint")]
    ForcedToolNotAllowed,

    /// A command or tool argument that cannot be honoured.
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<llm::Error> for Error {
    fn from(e: llm::Error) -> Self {
        match e {
            llm::Error::Keychain(d) => Error::Keychain(d),
            llm::Error::Unsupported(what) => Error::Unsupported(what),
            llm::Error::ForcedToolNotAllowed => Error::ForcedToolNotAllowed,
            // The endpoint's own text, as before.
            other => Error::Endpoint {
                detail: other.to_string(),
            },
        }
    }
}

/// RFC 3339 with the user's offset, as every timestamp crosses the seam.
pub fn timestamp(ms: i64, offset_min: i32) -> Timestamp {
    use world::clock::{DAY_MS, MINUTE_MS, civil_from_days};
    let local = ms + i64::from(offset_min) * MINUTE_MS;
    let d = civil_from_days(local.div_euclid(DAY_MS));
    let rem = local.rem_euclid(DAY_MS) / 1000;
    let (h, m, s) = (rem / 3600, rem / 60 % 60, rem % 60);
    let sign = if offset_min < 0 { '-' } else { '+' };
    let off = offset_min.unsigned_abs();
    Timestamp(format!(
        "{:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}{sign}{:02}:{:02}",
        d.year,
        d.month,
        d.day,
        off / 60,
        off % 60
    ))
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        match e {
            Error::World(world::Error::NotFound { .. }) => ApiError::NotFound,
            Error::World(world::Error::Disabled(p)) => ApiError::Disabled {
                person: api::types::PersonId(p.0),
            },
            // No dedicated code yet: the person is not available to talk to.
            Error::NoPersona(_) => ApiError::Invalid {
                field: "person".into(),
            },
            Error::World(
                world::Error::Invalid(_)
                | world::Error::NotParticipant { .. }
                | world::Error::ChannelClosed(_)
                | world::Error::NotEnoughTurns { .. }
                | world::Error::NoTask { .. },
            ) => ApiError::Invalid {
                field: "request".into(),
            },
            Error::Invalid(_) => ApiError::Invalid {
                field: "request".into(),
            },
            Error::QuotaExhausted { release_at } => ApiError::QuotaExhausted {
                release_at: timestamp(release_at.unwrap_or(0), 0),
            },
            Error::NoEndpoint => ApiError::NoEndpoint,
            Error::Unsupported(what) => ApiError::Endpoint {
                detail: format!("{what} is not supported by this build"),
            },
            Error::Endpoint { detail } => ApiError::Endpoint { detail },
            Error::ForcedToolNotAllowed => ApiError::Endpoint {
                detail: "a forced tool choice is not allowed on this endpoint".into(),
            },
            Error::Keychain(detail) => ApiError::Keychain { detail },
            Error::World(world::Error::Io(e)) => ApiError::Io {
                detail: e.to_string(),
            },
            other => ApiError::Io {
                detail: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_carry_the_offset() {
        assert_eq!(timestamp(0, 0).0, "1970-01-01T00:00:00+00:00");
        assert_eq!(timestamp(0, 480).0, "1970-01-01T08:00:00+08:00");
        assert_eq!(timestamp(0, -90).0, "1969-12-31T22:30:00-01:30");
    }

    #[test]
    fn llm_errors_keep_their_text() {
        let e: Error = llm::Error::Http {
            status: 429,
            message: "slow down".into(),
        }
        .into();
        assert!(matches!(&e, Error::Endpoint { detail } if detail == "HTTP 429: slow down"));
        assert!(matches!(
            Error::from(llm::Error::ForcedToolNotAllowed),
            Error::ForcedToolNotAllowed
        ));
    }

    #[test]
    fn codes() {
        let e: ApiError = Error::NoPersona(PersonId::from("x")).into();
        assert!(matches!(e, ApiError::Invalid { .. }));
        let e: ApiError = Error::World(world::Error::Disabled(PersonId::from("x"))).into();
        assert!(matches!(e, ApiError::Disabled { .. }));
    }
}
