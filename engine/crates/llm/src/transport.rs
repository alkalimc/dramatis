//! Bytes in, bytes out. Everything protocol-shaped happens above this trait, so a test
//! swaps in [`MemoryTransport`] and the real [`Http`] stays a thin reqwest wrapper.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;
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
}

/// Real HTTP over reqwest with the platform TLS stack.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl fmt::Debug for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Http")
    }
}

impl Http {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            // Sites that serve public files (avatars) expect a client that names itself.
            .user_agent(concat!(
                "dramatis/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/alkalimc/dramatis)"
            ))
            .connect_timeout(Duration::from_secs(30))
            // Between chunks, not for the whole call: a thinking model can be silent for
            // minutes before its first byte.
            .read_timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self { client })
    }

    async fn start(&self, req: HttpRequest) -> Result<reqwest::Response> {
        let mut b = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self.client.post(&req.url).body(req.body),
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
