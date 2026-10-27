use crate::types::Protocol;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint answered with an error status. `message` is the endpoint's own text.
    #[error("{}", http_text(*.status, .message))]
    Http { status: u16, message: String },
    /// HTTP 429. `retry_after` is the endpoint's own hint in seconds, when it gave one.
    #[error("{}", rate_limit_text(.retry_after, .message))]
    RateLimited {
        retry_after: Option<u64>,
        message: String,
    },
    /// The request never got an answer (connect, TLS, timeout, broken stream).
    #[error("transport: {0}")]
    Transport(String),
    /// An error the endpoint reported inside a stream or a 200 body.
    #[error("endpoint: {0}")]
    Endpoint(String),
    /// The answer is not what the protocol promises.
    #[error("unreadable reply: {0}")]
    Decode(String),
    /// This protocol cannot do that (speech and embeddings on the messages protocol).
    #[error("{0} is not supported on this protocol")]
    Unsupported(&'static str),
    /// A forced tool choice on an endpoint that must not get one. The caller asks for the
    /// tool in its text and keeps `Auto`.
    #[error("a forced tool choice is not allowed on this endpoint")]
    ForcedToolNotAllowed,
    /// A provider-native block sent to a protocol that did not produce it.
    #[error("a native block of type `{kind}` does not belong to {protocol:?}")]
    ForeignNative { kind: String, protocol: Protocol },
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("keychain: {0}")]
    Keychain(String),
}

fn rate_limit_text(retry_after: &Option<u64>, message: &str) -> String {
    let mut s = "HTTP 429".to_owned();
    if let Some(n) = retry_after {
        s.push_str(&format!(" (retry after {n} s)"));
    }
    if !message.is_empty() {
        s.push_str(": ");
        s.push_str(message);
    }
    s
}

fn http_text(status: u16, message: &str) -> String {
    if message.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {message}")
    }
}

impl Error {
    /// Whether another attempt can help: rate limits, server errors, network failures.
    pub fn retryable(&self) -> bool {
        match self {
            Error::Http { status, .. } => matches!(status, 408 | 500..=599),
            // A limit that lasts minutes is the user's to wait out, not this call's.
            Error::RateLimited { retry_after, .. } => retry_after.is_none_or(|n| n <= 10),
            Error::Transport(_) => true,
            _ => false,
        }
    }

    /// Map a non-success reply. Both OpenAI-style and Anthropic-style bodies carry the
    /// text at `error.message`; anything else is shown as received, truncated.
    pub fn from_status(status: u16, body: &[u8], retry_after: Option<u64>) -> Self {
        let text = String::from_utf8_lossy(body);
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.get("message"))
                    .and_then(|m| m.as_str().map(str::to_owned))
            })
            .unwrap_or_else(|| text.chars().take(500).collect());
        if status == 429 {
            return Error::RateLimited {
                retry_after,
                message,
            };
        }
        Error::Http { status, message }
    }
}
