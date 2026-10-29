use serde::{Deserialize, Serialize};
use specta::Type;

use crate::types::{PersonId, Timestamp};

/// Every command's error. A code the UI maps to an i18n key, never a sentence; `detail`
/// carries only text that did not originate here (an endpoint's own error body, an OS
/// error), shown verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ApiError {
    #[error("not implemented")]
    NotImplemented,
    #[error("not found")]
    NotFound,
    /// The request itself is malformed (empty text, unknown key, out-of-range value).
    #[error("invalid argument `{field}`")]
    Invalid { field: String },
    /// The target is disabled and so does not exist for this purpose.
    #[error("person {person:?} is disabled")]
    Disabled { person: PersonId },
    /// The corpus has no persona for this person: nobody can speak as them yet.
    #[error("person {person:?} has no persona")]
    NoPersona { person: PersonId },
    /// Quota band three: no new calls until the window releases.
    #[error("quota exhausted until {release_at:?}")]
    QuotaExhausted { release_at: Timestamp },
    /// No chat role configured: the app is in library form.
    #[error("no chat endpoint configured")]
    NoEndpoint,
    /// A reply or request is being written; what was asked waits for it to end.
    #[error("busy")]
    Busy,
    /// A save with that name is already there.
    #[error("already exists")]
    Exists,
    /// The save is encrypted and this process has no password for it yet: ask, then
    /// `unlock`.
    #[error("locked")]
    Locked,
    /// The save is open in another window or process, so a step that would rewrite, move
    /// or delete its file waits until it is closed there.
    #[error("open elsewhere")]
    OpenElsewhere,
    /// A password was given and does not open the save.
    #[error("wrong password")]
    WrongPassword,
    #[error("endpoint error: {detail}")]
    Endpoint { detail: String },
    #[error("keychain error: {detail}")]
    Keychain { detail: String },
    #[error("i/o error: {detail}")]
    Io { detail: String },
}

pub type ApiResult<T> = Result<T, ApiError>;
