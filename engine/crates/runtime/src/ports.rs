//! The ports the engine reaches outside itself through, over the system keychain and the
//! shared LLM client.

use std::sync::Arc;
use std::time::Duration;

use api::endpoints::{Model, Profile};
use api::error::{ApiError, ApiResult};
use api::port::{self, KeyStore, LlmPort, LocalClock, NetPort, PROXY_ACCOUNT, Speech};
use api::views::{EndpointTest, NetworkMode, NetworkSetting, TestFailure};
use async_trait::async_trait;
use llm::Transport;
use llm::transport::{HttpRequest, Method};

/// How long a reachability probe waits for the endpoint's host.
const PROBE: Duration = Duration::from_millis(1500);
/// How long an endpoint test waits for a reply before calling it a timeout.
const TEST_LIMIT: Duration = Duration::from_secs(30);
/// Output budget of the endpoint test: enough for a word, small enough to cost nothing.
const TEST_TOKENS: u32 = 32;
/// What the test says to the model.
const TEST_TEXT: &str = "Reply with the single word: ready";
/// How much of the reply the test shows.
const TEST_SHOWN: usize = 80;

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

/// Addresses on this computer never go through a manual proxy (a local model server).
const LOOPBACK: &str = "localhost,127.0.0.1,::1";

/// The transport's network for a saved setting. The proxy password is passed in: it comes
/// from the keychain, and only a manual proxy has one.
pub(crate) fn network_of(setting: &NetworkSetting, password: Option<String>) -> llm::Network {
    let some = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
    match setting.mode {
        NetworkMode::Direct => llm::Network::Direct,
        NetworkMode::System => llm::Network::System,
        NetworkMode::Manual => llm::Network::Manual(llm::Proxy {
            url: setting.proxy.trim().to_owned(),
            username: some(&setting.username),
            password: password.filter(|_| some(&setting.username).is_some()),
            no_proxy: Some(match some(&setting.no_proxy) {
                Some(list) => format!("{list},{LOOPBACK}"),
                None => LOOPBACK.to_owned(),
            }),
        }),
    }
}

fn proxy_password(setting: &NetworkSetting) -> Option<String> {
    if setting.mode != NetworkMode::Manual {
        return None;
    }
    llm::keychain::get(PROXY_ACCOUNT)
        .ok()
        .flatten()
        .map(|k| k.expose().to_owned())
}

/// The real transport for a saved network choice. A choice that cannot be built (a
/// proxy address edited by hand into nonsense) leaves the system's way in place rather
/// than keeping the app from starting.
pub(crate) fn http_for(setting: &NetworkSetting) -> ApiResult<llm::Http> {
    let network = network_of(setting, proxy_password(setting));
    llm::Http::with_network(&network)
        .or_else(|_| llm::Http::new())
        .map_err(failed)
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

    async fn probe(&self, profile: &Profile, model: &str) -> ApiResult<EndpointTest> {
        let started = std::time::Instant::now();
        let lapse = || u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
        let failure = |class: TestFailure, detail: String| EndpointTest {
            ok: false,
            latency_ms: lapse(),
            reply: String::new(),
            failure: Some(class),
            detail,
        };
        let key = match llm::keychain::get(&profile.name) {
            Ok(key) => key,
            Err(e) => return Ok(failure(TestFailure::Key, e.to_string())),
        };
        // A server on this computer needs no key; any other without one would be refused.
        let local = host_port(&profile.base_url)
            .is_some_and(|(host, _)| matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"));
        if key.is_none() && !local {
            return Ok(failure(TestFailure::Key, "no key is stored".into()));
        }
        let model = Model {
            id: model.to_owned(),
            ..Model::default()
        };
        let endpoint = agent::endpoint::endpoint(profile, &model, key);
        let request = llm::Request {
            messages: vec![llm::Message::user(TEST_TEXT)],
            max_output: Some(TEST_TOKENS),
            ..llm::Request::default()
        };
        let once = self.0.once();
        let sent = tokio::time::timeout(TEST_LIMIT, once.complete(&endpoint, &request));
        Ok(match sent.await {
            Err(_) => failure(TestFailure::Timeout, String::new()),
            Ok(Err(e)) => failure(classify(&e), shorten(&e.to_string(), 300)),
            Ok(Ok(reply)) => EndpointTest {
                ok: true,
                latency_ms: lapse(),
                reply: shorten(
                    &reply
                        .text()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    TEST_SHOWN,
                ),
                failure: None,
                detail: String::new(),
            },
        })
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

/// At most `max` characters, with an ellipsis where it was cut.
fn shorten(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

/// What a person should fix, from what the endpoint (or the wire) said. A 404 is the
/// address when the text does not name the model, and the model when it does.
pub(crate) fn classify(e: &llm::Error) -> TestFailure {
    let names_model = |text: &str| text.to_lowercase().contains("model");
    match e {
        llm::Error::Http {
            status: 401 | 403, ..
        } => TestFailure::Key,
        llm::Error::Http {
            status: 404,
            message,
        } if names_model(message) => TestFailure::Model,
        llm::Error::Http {
            status: 404 | 405, ..
        } => TestFailure::Address,
        llm::Error::Http {
            status: 400 | 422,
            message,
        } if names_model(message) => TestFailure::Model,
        llm::Error::Endpoint(message) if names_model(message) => TestFailure::Model,
        llm::Error::Transport(message) => {
            let lower = message.to_lowercase();
            if lower.contains("timed out") || lower.contains("timeout") {
                TestFailure::Timeout
            } else {
                TestFailure::Address
            }
        }
        // A page or a service that answers 200 with something that is not the protocol.
        llm::Error::Decode(_) => TestFailure::Address,
        llm::Error::Keychain(_) => TestFailure::Key,
        _ => TestFailure::Other,
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

    fn set_network(&self, setting: &NetworkSetting) -> Result<(), String> {
        let network = network_of(setting, proxy_password(setting));
        self.0.set_network(&network).map_err(|e| e.to_string())
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

    fn local(wire: api::endpoints::WireApi) -> Profile {
        Profile {
            name: "no-such-profile-in-the-keychain".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            wire_api: wire,
            models: Vec::new(),
        }
    }

    #[tokio::test]
    async fn the_endpoint_test_sends_one_small_request_and_shows_the_first_words() {
        let reply = r#"{"choices":[{"message":{"content":"ready\n  and   more words"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#;
        let t = Arc::new(MemoryTransport::new([Ok(Canned::ok(reply))]));
        let port = Models(llm::Llm::new(t.clone(), Retry::default()));
        let got = port
            .probe(&local(api::endpoints::WireApi::Chat), "m")
            .await
            .unwrap();
        assert!(got.ok && got.failure.is_none());
        assert_eq!(got.reply, "ready and more words");
        let sent = t.sent();
        assert_eq!(sent.len(), 1);
        let body = String::from_utf8(sent[0].body.clone()).unwrap();
        assert!(body.contains(&format!(":{TEST_TOKENS}")), "{body}");
    }

    #[tokio::test]
    async fn the_endpoint_test_names_what_to_fix_and_does_not_retry() {
        let status = |code, body: &str| Ok(Canned::status(code, body));
        let t = Arc::new(MemoryTransport::new([
            status(401, r#"{"error":{"message":"bad key"}}"#),
            status(
                404,
                r#"{"error":{"message":"The model `x` does not exist"}}"#,
            ),
            status(404, "<html>not found</html>"),
            status(503, "down"),
            Err(llm::Error::Transport("operation timed out".into())),
            Err(llm::Error::Transport("error sending request".into())),
            status(200, "<html>welcome</html>"),
        ]));
        // Retries would use up the queue on the 503.
        let port = Models(llm::Llm::new(t.clone(), Retry::default()));
        let profile = local(api::endpoints::WireApi::Chat);
        let mut classes = Vec::new();
        for _ in 0..7 {
            let got = port.probe(&profile, "m").await.unwrap();
            assert!(!got.ok && got.reply.is_empty());
            classes.push(got.failure.unwrap());
        }
        use TestFailure::*;
        assert_eq!(
            classes,
            [Key, Model, Address, Other, Timeout, Address, Address]
        );
        assert_eq!(t.sent().len(), 7);
    }

    #[tokio::test]
    async fn the_endpoint_test_without_a_key_sends_nothing() {
        let t = Arc::new(MemoryTransport::new([]));
        let port = Models(llm::Llm::new(t.clone(), Retry::none()));
        let mut remote = local(api::endpoints::WireApi::Chat);
        remote.base_url = "https://api.example.com/v1".into();
        let got = port.probe(&remote, "m").await.unwrap();
        assert_eq!(got.failure, Some(TestFailure::Key));
        assert!(t.sent().is_empty());
    }

    #[test]
    fn a_saved_network_becomes_the_transports_and_a_password_needs_its_user() {
        let setting = NetworkSetting {
            mode: NetworkMode::Manual,
            proxy: " socks5://10.0.0.2:1080 ".into(),
            username: "me".into(),
            no_proxy: " .corp ,localhost ".into(),
        };
        assert_eq!(
            network_of(&setting, Some("pw".into())),
            llm::Network::Manual(llm::Proxy {
                url: "socks5://10.0.0.2:1080".into(),
                username: Some("me".into()),
                password: Some("pw".into()),
                no_proxy: Some(".corp ,localhost,localhost,127.0.0.1,::1".into()),
            })
        );
        let anonymous = NetworkSetting {
            username: String::new(),
            ..setting.clone()
        };
        let llm::Network::Manual(p) = network_of(&anonymous, Some("pw".into())) else {
            panic!("manual");
        };
        assert_eq!((p.username, p.password), (None, None));
        let direct = NetworkSetting {
            mode: NetworkMode::Direct,
            ..setting
        };
        assert_eq!(network_of(&direct, None), llm::Network::Direct);
        assert_eq!(
            network_of(&NetworkSetting::default(), None),
            llm::Network::System
        );
    }
}
