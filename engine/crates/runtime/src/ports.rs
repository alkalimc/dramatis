//! The ports the engine reaches outside itself through, over the system keychain and the
//! shared LLM client.

use std::sync::Arc;
use std::time::Duration;

use api::endpoints::{Model, Profile};
use api::error::{ApiError, ApiResult};
use api::port::{self, KeyStore, LlmPort, LocalClock, NetPort, Speech};
use async_trait::async_trait;
use llm::Transport;
use llm::transport::{HttpRequest, Method};

/// How long a reachability probe waits for the endpoint's host.
const PROBE: Duration = Duration::from_millis(1500);

/// The system keychain, under the same service and accounts the agent reads.
pub(crate) struct Keys;

pub(crate) fn failed(e: llm::Error) -> ApiError {
    match e {
        llm::Error::Keychain(detail) => ApiError::Keychain { detail },
        other => ApiError::Endpoint {
            detail: other.to_string(),
        },
    }
}

impl KeyStore for Keys {
    fn set(&self, profile: &str, key: &str) -> ApiResult<()> {
        llm::keychain::set(profile, key).map_err(failed)
    }

    fn has(&self, profile: &str) -> bool {
        llm::keychain::has(profile).unwrap_or(false)
    }

    fn delete(&self, profile: &str) -> ApiResult<()> {
        llm::keychain::delete(profile).map_err(failed)
    }

    fn rename(&self, from: &str, to: &str) -> ApiResult<()> {
        match llm::keychain::get(from).map_err(failed)? {
            Some(key) => {
                self.set(to, key.expose())?;
                self.delete(from)
            }
            None => Ok(()),
        }
    }
}

/// The voice asked for when a person has none set.
const DEFAULT_VOICE: &str = "alloy";

/// The calls that are not a session, through the shared client.
pub(crate) struct Models(pub(crate) llm::Llm);

impl Models {
    /// The endpoint of a profile with the named model (none for a listing).
    fn endpoint(profile: &Profile, model: &str) -> ApiResult<llm::Endpoint> {
        let key = llm::keychain::get(&profile.name).map_err(failed)?;
        let model = Model {
            id: model.to_owned(),
            ..Model::default()
        };
        Ok(agent::endpoint::endpoint(profile, &model, key))
    }
}

#[async_trait]
impl LlmPort for Models {
    async fn models(&self, profile: &Profile) -> ApiResult<Vec<String>> {
        self.0
            .models(&Self::endpoint(profile, "")?)
            .await
            .map_err(failed)
    }

    /// The client lists no voices: the person's own choice (or the default) is what is sent.
    async fn voices(&self, _: &Profile, _: &str) -> ApiResult<Vec<String>> {
        Ok(Vec::new())
    }

    async fn speech(
        &self,
        profile: &Profile,
        model: &str,
        voice: Option<&str>,
        text: &str,
    ) -> ApiResult<Speech> {
        let bytes = self
            .0
            .speech(
                &Self::endpoint(profile, model)?,
                text,
                voice.unwrap_or(DEFAULT_VOICE),
            )
            .await
            .map_err(failed)?;
        Ok(Speech {
            bytes,
            ext: "mp3".into(),
        })
    }
}

/// Reachability is a TCP connect to the endpoint's host: nothing is sent. Plain fetches
/// (avatars) go through the shared client.
pub(crate) struct Net(pub(crate) Arc<dyn Transport>);

#[async_trait]
impl NetPort for Net {
    async fn get(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
        let reply = self
            .0
            .send(HttpRequest {
                method: Method::Get,
                url: url.to_owned(),
                headers: Vec::new(),
                body: Vec::new(),
            })
            .await
            .map_err(|e| e.to_string())?;
        match reply.status {
            200..=299 => Ok(Some(reply.body)),
            // The site answered without the file.
            400..=499 => Ok(None),
            status => Err(format!("HTTP {status}")),
        }
    }

    async fn reachable(&self, base_url: &str) -> bool {
        let Some((host, port)) = host_port(base_url) else {
            return false;
        };
        matches!(
            tokio::time::timeout(PROBE, tokio::net::TcpStream::connect((host.as_str(), port)))
                .await,
            Ok(Ok(_))
        )
    }
}

fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let default = if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        80
    };
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || p.chars().all(|c| c.is_ascii_digit()) => {
            Some((h.trim_matches(['[', ']']).to_owned(), p.parse().ok()?))
        }
        _ => Some((authority.trim_matches(['[', ']']).to_owned(), default)),
    }
}

/// The agent reads the same local clock.
pub(crate) struct Local;

impl agent::Clock for Local {
    fn now(&self) -> world::clock::Now {
        port::Clock::now(&LocalClock)
    }
}

#[cfg(test)]
mod tests {
    use llm::{Canned, MemoryTransport, Retry};

    use super::*;

    #[tokio::test]
    async fn plain_fetches_map_the_status() {
        let t = Arc::new(MemoryTransport::new([
            Ok(Canned::ok(b"png")),
            Ok(Canned::status(404, "")),
            Ok(Canned::status(503, "")),
        ]));
        let net = Net(t.clone());
        assert_eq!(net.get("http://x/a.png").await, Ok(Some(b"png".to_vec())));
        assert_eq!(net.get("http://x/b.png").await, Ok(None));
        assert_eq!(net.get("http://x/c.png").await, Err("HTTP 503".into()));
        assert_eq!(t.sent()[0].url, "http://x/a.png");
        assert_eq!(t.sent()[0].method, Method::Get);
    }

    #[tokio::test]
    async fn the_model_list_and_speech_go_through_the_client() {
        let t = Arc::new(MemoryTransport::new([
            Ok(Canned::ok(br#"{"data":[{"id":"m1"},{"id":"m2"}]}"#)),
            Ok(Canned::ok(b"audio")),
            Ok(Canned::status(401, br#"{"error":{"message":"bad key"}}"#)),
        ]));
        let port = Models(llm::Llm::new(t.clone(), Retry::none()));
        let profile = |wire| Profile {
            name: "no-such-profile-in-the-keychain".into(),
            base_url: "http://x/v1".into(),
            wire_api: wire,
            models: Vec::new(),
        };
        let chat = profile(api::endpoints::WireApi::Chat);
        assert_eq!(port.models(&chat).await.unwrap(), ["m1", "m2"]);
        let speech = port.speech(&chat, "tts", Some("v"), "hello").await.unwrap();
        assert_eq!(speech.bytes, b"audio");
        let body = String::from_utf8(t.sent()[1].body.clone()).unwrap();
        assert!(body.contains(r#""voice":"v""#) && body.contains(r#""input":"hello""#));
        let err = port.models(&chat).await.unwrap_err();
        assert!(matches!(err, ApiError::Endpoint { detail } if detail == "HTTP 401: bad key"));
        // No request is made for speech on the messages protocol.
        let messages = profile(api::endpoints::WireApi::Messages);
        let err = port.speech(&messages, "m", None, "hi").await.unwrap_err();
        assert!(matches!(err, ApiError::Endpoint { detail } if detail.contains("not supported")));
        assert_eq!(t.sent().len(), 3);
    }
}
