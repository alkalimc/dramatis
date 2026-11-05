//! Bytes in, bytes out. Everything protocol-shaped happens above this trait, so a test
//! swaps in [`MemoryTransport`] and the real [`Http`] stays a thin reqwest wrapper.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    /// Includes the authentication header; never printed.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &self.headers.iter().map(|h| &h.0).collect::<Vec<_>>(),
            )
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// Seconds from a `Retry-After` header, when it carries a plain number.
    pub retry_after: Option<u64>,
}

pub type Chunks = BoxStream<'static, Result<Vec<u8>>>;

pub struct StreamResponse {
    pub status: u16,
    pub chunks: Chunks,
    /// As in [`HttpResponse`].
    pub retry_after: Option<u64>,
}

#[async_trait]
pub trait Transport: Send + Sync {
    /// The whole reply. An error status is a reply, not an `Err`.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse>;
    /// The reply as it arrives.
    async fn open(&self, req: HttpRequest) -> Result<StreamResponse>;
    /// Change how later requests leave this machine; requests already running finish as
    /// they began. A transport with no network of its own has nothing to change.
    fn set_network(&self, _network: &Network) -> Result<()> {
        Ok(())
    }
}

/// How connections leave this machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Network {
    /// No proxy, whatever the environment or the system settings say.
    Direct,
    /// What the system says: the proxy environment variables, and on macOS and Windows
    /// the proxy in the system settings.
    #[default]
    System,
    /// The proxy given here, for every address its exclusion list does not name.
    Manual(Proxy),
}

/// A proxy chosen by hand.
#[derive(Clone, PartialEq, Eq)]
pub struct Proxy {
    /// `http://`, `https://`, `socks5://` or `socks5h://` and an address.
    pub url: String,
    pub username: Option<String>,
    /// Never printed.
    pub password: Option<String>,
    /// Comma-separated hosts, domains (`.corp`), addresses and networks that bypass the
    /// proxy, in the `NO_PROXY` notation.
    pub no_proxy: Option<String>,
}

impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proxy")
            .field("url", &redact_proxy(&self.url))
            .field("username", &self.username.is_some())
            .field("password", &self.password.is_some())
            .field("no_proxy", &self.no_proxy)
            .finish()
    }
}

/// A proxy address with any login in it hidden, for messages and logs.
pub fn redact_proxy(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
            match authority.rsplit_once('@') {
                Some((_, host)) => format!("{scheme}://***@{host}{tail}"),
                None => url.to_owned(),
            }
        }
        None => url.to_owned(),
    }
}

/// Real HTTP over reqwest with the platform TLS stack.
#[derive(Clone)]
pub struct Http {
    /// Behind a lock so a change of network applies to every clone at once.
    client: Arc<RwLock<reqwest::Client>>,
}

impl fmt::Debug for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Http")
    }
}

/// The client for a network choice.
fn build(network: &Network) -> Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        // Sites that serve public files (avatars) expect a client that names itself.
        .user_agent(concat!(
            "dramatis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/alkalimc/dramatis)"
        ))
        .connect_timeout(Duration::from_secs(30))
        // Between chunks, not for the whole call: a thinking model can be silent for
        // minutes before its first byte.
        .read_timeout(Duration::from_secs(600));
    let builder = match network {
        // A client picks up the proxy environment variables on its own; asking for no
        // proxy is what turns that off.
        Network::Direct => builder.no_proxy(),
        Network::System => builder,
        Network::Manual(p) => {
            // The message never carries the address: it may hold a login.
            let bad = |_| Error::Invalid("the proxy address is not valid".into());
            let mut proxy = reqwest::Proxy::all(p.url.trim()).map_err(bad)?;
            if let Some(user) = p.username.as_deref().filter(|u| !u.is_empty()) {
                proxy = proxy.basic_auth(user, p.password.as_deref().unwrap_or_default());
            }
            proxy = proxy.no_proxy(
                p.no_proxy
                    .as_deref()
                    .and_then(reqwest::NoProxy::from_string),
            );
            // Adding a proxy also turns the system's off, so the exclusion list means
            // "go direct" and not "go to the system proxy".
            builder.proxy(proxy)
        }
    };
    builder
        .build()
        .map_err(|e| Error::Transport(e.without_url().to_string()))
}

impl Http {
    /// Follows the system's proxy settings.
    pub fn new() -> Result<Self> {
        Self::with_network(&Network::default())
    }

    pub fn with_network(network: &Network) -> Result<Self> {
        Ok(Self {
            client: Arc::new(RwLock::new(build(network)?)),
        })
    }

    fn client(&self) -> reqwest::Client {
        self.client
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    async fn start(&self, req: HttpRequest) -> Result<reqwest::Response> {
        let client = self.client();
        let mut b = match req.method {
            Method::Get => client.get(&req.url),
            Method::Post => client.post(&req.url).body(req.body),
        };
        for (k, v) in &req.headers {
            b = b.header(k, v);
        }
        b.send()
            .await
            .map_err(|e| Error::Transport(e.without_url().to_string()))
    }
}

#[async_trait]
impl Transport for Http {
    fn set_network(&self, network: &Network) -> Result<()> {
        let next = build(network)?;
        *self.client.write().unwrap_or_else(|e| e.into_inner()) = next;
        Ok(())
    }

    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        let resp = self.start(req).await?;
        let status = resp.status().as_u16();
        let retry_after = retry_after(&resp);
        let body = resp
            .bytes()
            .await
            .map_err(|e| Error::Transport(e.without_url().to_string()))?;
        Ok(HttpResponse {
            status,
            body: body.to_vec(),
            retry_after,
        })
    }

    async fn open(&self, req: HttpRequest) -> Result<StreamResponse> {
        let resp = self.start(req).await?;
        let status = resp.status().as_u16();
        let retry_after = retry_after(&resp);
        let chunks = futures::stream::unfold(resp, |mut resp| async move {
            match resp.chunk().await {
                Ok(Some(c)) => Some((Ok(c.to_vec()), resp)),
                Ok(None) => None,
                Err(e) => Some((Err(Error::Transport(e.without_url().to_string())), resp)),
            }
        })
        .boxed();
        Ok(StreamResponse {
            status,
            chunks,
            retry_after,
        })
    }
}

/// `Retry-After` in seconds. The date form is not read: the delay it would give is not
/// worth a date parser here.
fn retry_after(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// A canned reply: status plus the body, split into the chunks a stream delivers.
#[derive(Debug, Clone)]
pub struct Canned {
    pub status: u16,
    pub chunks: Vec<Vec<u8>>,
    pub retry_after: Option<u64>,
}

impl Canned {
    pub fn ok(body: impl AsRef<[u8]>) -> Self {
        Self {
            status: 200,
            chunks: vec![body.as_ref().to_vec()],
            retry_after: None,
        }
    }

    pub fn status(status: u16, body: impl AsRef<[u8]>) -> Self {
        Self {
            status,
            chunks: vec![body.as_ref().to_vec()],
            retry_after: None,
        }
    }

    pub fn retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs);
        self
    }

    /// A stream cut into pieces of `size` bytes, to exercise events split across chunks.
    pub fn chunked(body: impl AsRef<[u8]>, size: usize) -> Self {
        Self {
            status: 200,
            chunks: body
                .as_ref()
                .chunks(size.max(1))
                .map(<[u8]>::to_vec)
                .collect(),
            retry_after: None,
        }
    }
}

/// Replays canned replies in order and records what was sent. For tests, here and in
/// the crates that call this one.
#[derive(Debug, Default)]
pub struct MemoryTransport {
    replies: Mutex<VecDeque<Result<Canned>>>,
    sent: Mutex<Vec<HttpRequest>>,
}

impl MemoryTransport {
    pub fn new(replies: impl IntoIterator<Item = Result<Canned>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            sent: Mutex::default(),
        }
    }

    pub fn sent(&self) -> Vec<HttpRequest> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn next(&self, req: HttpRequest) -> Result<Canned> {
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req);
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or_else(|| Err(Error::Transport("no canned reply left".into())))
    }
}

#[async_trait]
impl Transport for MemoryTransport {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        let c = self.next(req)?;
        Ok(HttpResponse {
            status: c.status,
            body: c.chunks.concat(),
            retry_after: c.retry_after,
        })
    }

    async fn open(&self, req: HttpRequest) -> Result<StreamResponse> {
        let c = self.next(req)?;
        Ok(StreamResponse {
            status: c.status,
            chunks: futures::stream::iter(c.chunks.into_iter().map(Ok)).boxed(),
            retry_after: c.retry_after,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A server on this machine that answers one request with `body` and sends back the
    /// request line it saw. Nothing leaves the loopback interface.
    async fn serve(body: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = conn.read(&mut buf).await.unwrap();
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            conn.write_all(reply.as_bytes()).await.unwrap();
            head
        });
        (port, seen)
    }

    fn get(url: String) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn manual(port: u16, no_proxy: Option<&str>) -> Network {
        Network::Manual(Proxy {
            url: format!("http://127.0.0.1:{port}"),
            username: Some("me".into()),
            password: Some("hunter2".into()),
            no_proxy: no_proxy.map(str::to_owned),
        })
    }

    #[tokio::test]
    async fn a_manual_proxy_receives_the_request_with_its_login() {
        let (port, seen) = serve("via proxy").await;
        let http = Http::with_network(&manual(port, None)).unwrap();
        // The host does not exist: only the proxy can have answered.
        let reply = http
            .send(get("http://models.invalid/v1/models".into()))
            .await
            .unwrap();
        assert_eq!(reply.body, b"via proxy");
        let head = seen.await.unwrap().to_lowercase();
        assert!(head.starts_with("get http://models.invalid/v1/models http/1.1"));
        // "me:hunter2" in base64.
        assert!(head.contains("proxy-authorization: basic bwu6ahvudgvymg=="));
    }

    #[tokio::test]
    async fn the_exclusion_list_goes_direct_and_not_to_the_system_proxy() {
        let (proxy, never) = serve("via proxy").await;
        let (target, seen) = serve("direct").await;
        let http = Http::with_network(&manual(proxy, Some("127.0.0.1"))).unwrap();
        let reply = http
            .send(get(format!("http://127.0.0.1:{target}/x")))
            .await
            .unwrap();
        assert_eq!(reply.body, b"direct");
        // Origin form: the target saw a request for itself, not one meant for a proxy.
        assert!(seen.await.unwrap().starts_with("GET /x HTTP/1.1"));
        never.abort();
    }

    #[tokio::test]
    async fn a_change_of_network_applies_to_clones_already_made() {
        let (port, seen) = serve("via proxy").await;
        let http = Http::with_network(&Network::Direct).unwrap();
        let clone = http.clone();
        http.set_network(&manual(port, None)).unwrap();
        let reply = clone
            .send(get("http://models.invalid/".into()))
            .await
            .unwrap();
        assert_eq!(reply.body, b"via proxy");
        seen.await.unwrap();
    }

    #[test]
    fn a_bad_proxy_is_refused_without_repeating_its_address() {
        let bad = Network::Manual(Proxy {
            url: "not a url with secret".into(),
            username: None,
            password: None,
            no_proxy: None,
        });
        let e = Http::with_network(&bad).unwrap_err().to_string();
        assert!(!e.contains("secret"), "{e}");
    }

    #[test]
    fn logins_are_hidden_when_a_proxy_is_printed() {
        assert_eq!(
            redact_proxy("http://me:hunter2@proxy.example:8080/path"),
            "http://***@proxy.example:8080/path"
        );
        assert_eq!(
            redact_proxy("socks5://10.0.0.1:1080"),
            "socks5://10.0.0.1:1080"
        );
        let shown = format!("{:?}", manual(1, None));
        assert!(!shown.contains("hunter2"), "{shown}");
    }
}
