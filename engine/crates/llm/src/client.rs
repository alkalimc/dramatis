//! The one interface callers use: `complete`, `stream`, `models`, `speech`, `embed`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use crate::decode::{Decoder, Sse, decode_response};
use crate::error::{Error, Result};
use crate::render::{Rendered, auth_headers, render, render_stream, to_bytes};
use crate::transport::{
    Chunks, Http, HttpRequest, HttpResponse, Method, StreamResponse, Transport,
};
use crate::types::{Endpoint, Event, Protocol, Request, Response};

/// Bounded retries with exponential backoff on rate limits, server errors and network
/// failures. Every other error is final.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub attempts: u32,
    pub base: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 4,
            base: Duration::from_secs(2),
        }
    }
}

impl Retry {
    pub fn none() -> Self {
        Self {
            attempts: 1,
            base: Duration::ZERO,
        }
    }

    fn delay(&self, failed: u32) -> Duration {
        (self.base * 2u32.saturating_pow(failed.saturating_sub(1))).min(Duration::from_secs(60))
    }
}

#[derive(Clone)]
pub struct Llm {
    transport: Arc<dyn Transport>,
    retry: Retry,
}

impl Llm {
    pub fn new(transport: Arc<dyn Transport>, retry: Retry) -> Self {
        Self { transport, retry }
    }

    pub fn http() -> Result<Self> {
        Ok(Self::new(Arc::new(Http::new()?), Retry::default()))
    }

    fn request(&self, ep: &Endpoint, r: Rendered) -> HttpRequest {
        let mut headers = r.headers;
        headers.extend(auth_headers(ep));
        HttpRequest {
            method: Method::Post,
            url: r.url,
            headers,
            body: r.body,
        }
    }

    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        let mut failed = 0;
        loop {
            let outcome = match self.transport.send(req.clone()).await {
                Ok(r) if r.status >= 400 => Err(Error::from_status(r.status, &r.body)),
                other => other,
            };
            match outcome {
                Err(e) if e.retryable() && failed + 1 < self.retry.attempts => {
                    failed += 1;
                    tokio::time::sleep(self.retry.delay(failed)).await;
                }
                other => return other,
            }
        }
    }

    async fn open(&self, req: HttpRequest) -> Result<Chunks> {
        let mut failed = 0;
        loop {
            let outcome = match self.transport.open(req.clone()).await {
                Ok(StreamResponse { status, chunks }) if status >= 400 => {
                    let body: Vec<u8> = chunks
                        .filter_map(|c| async move { c.ok() })
                        .collect::<Vec<_>>()
                        .await
                        .concat();
                    Err(Error::from_status(status, &body))
                }
                Ok(r) => Ok(r.chunks),
                Err(e) => Err(e),
            };
            match outcome {
                Err(e) if e.retryable() && failed + 1 < self.retry.attempts => {
                    failed += 1;
                    tokio::time::sleep(self.retry.delay(failed)).await;
                }
                other => return other,
            }
        }
    }

    /// One request, one whole reply.
    pub async fn complete(&self, ep: &Endpoint, req: &Request) -> Result<Response> {
        let resp = self.send(self.request(ep, render(ep, req)?)).await?;
        let v: Value = serde_json::from_slice(&resp.body)
            .map_err(|e| Error::Decode(format!("reply is not JSON: {e}")))?;
        decode_response(ep.protocol, &v)
    }

    /// One request, events as they arrive, ending in `Done`. Retries cover only the
    /// connection: once bytes flow, a failure is the caller's to handle.
    pub async fn stream(
        &self,
        ep: &Endpoint,
        req: &Request,
    ) -> Result<BoxStream<'static, Result<Event>>> {
        let chunks = self.open(self.request(ep, render_stream(ep, req)?)).await?;
        let state = StreamState {
            chunks,
            sse: Sse::default(),
            decoder: Decoder::new(ep.protocol),
            pending: VecDeque::new(),
            ended: false,
        };
        Ok(futures::stream::unfold(state, |mut st| async move {
            loop {
                if let Some(ev) = st.pending.pop_front() {
                    return Some((Ok(ev), st));
                }
                if st.ended {
                    return None;
                }
                let data = match st.chunks.next().await {
                    Some(Ok(c)) => st.sse.push(&c),
                    Some(Err(e)) => {
                        st.ended = true;
                        return Some((Err(e), st));
                    }
                    None => {
                        st.ended = true;
                        let mut data = st.sse.finish();
                        data.push(String::new()); // marks the end of the body
                        data
                    }
                };
                for d in data {
                    let step = if d.is_empty() {
                        st.decoder.finish()
                    } else {
                        serde_json::from_str::<Value>(&d)
                            .map_err(|e| Error::Decode(format!("event is not JSON: {e}")))
                            .and_then(|v| st.decoder.push(&v))
                    };
                    match step {
                        Ok(evs) => st.pending.extend(evs),
                        Err(e) => {
                            st.ended = true;
                            st.pending.clear();
                            return Some((Err(e), st));
                        }
                    }
                }
            }
        })
        .boxed())
    }

    /// Model ids from `GET {base_url}/models`. A failure carries the endpoint's own text.
    pub async fn models(&self, ep: &Endpoint) -> Result<Vec<String>> {
        let mut url = ep.url("models");
        if ep.protocol == Protocol::AnthropicMessages {
            url.push_str("?limit=1000");
        }
        let mut headers = auth_headers(ep);
        if ep.protocol == Protocol::AnthropicMessages {
            headers.push((
                "anthropic-version".into(),
                crate::render::ANTHROPIC_VERSION.into(),
            ));
        }
        let resp = self
            .send(HttpRequest {
                method: Method::Get,
                url,
                headers,
                body: Vec::new(),
            })
            .await?;
        let v: Value = serde_json::from_slice(&resp.body)
            .map_err(|e| Error::Decode(format!("model list is not JSON: {e}")))?;
        Ok(v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect())
    }

    /// Spoken audio (the endpoint's default container) for `text`.
    pub async fn speech(&self, ep: &Endpoint, text: &str, voice: &str) -> Result<Vec<u8>> {
        if ep.protocol == Protocol::AnthropicMessages {
            return Err(Error::Unsupported("speech"));
        }
        let body = to_bytes(json!({"model": ep.model, "input": text, "voice": voice}));
        let resp = self.send(self.json_post(ep, "audio/speech", body)).await?;
        Ok(resp.body)
    }

    pub async fn embed(&self, ep: &Endpoint, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if ep.protocol == Protocol::AnthropicMessages {
            return Err(Error::Unsupported("embeddings"));
        }
        let body = to_bytes(json!({"model": ep.model, "input": inputs}));
        let resp = self.send(self.json_post(ep, "embeddings", body)).await?;
        let v: Value = serde_json::from_slice(&resp.body)
            .map_err(|e| Error::Decode(format!("embeddings reply is not JSON: {e}")))?;
        let mut rows: Vec<(u64, Vec<f32>)> = v["data"]
            .as_array()
            .ok_or_else(|| Error::Decode("embeddings reply has no data".into()))?
            .iter()
            .map(|d| {
                let vec = d["embedding"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_f64().map(|x| x as f32))
                            .collect()
                    })
                    .ok_or_else(|| Error::Decode("embedding is not an array".into()))?;
                Ok((d["index"].as_u64().unwrap_or(0), vec))
            })
            .collect::<Result<_>>()?;
        rows.sort_by_key(|r| r.0);
        if rows.len() != inputs.len() {
            return Err(Error::Decode(format!(
                "{} embeddings for {} inputs",
                rows.len(),
                inputs.len()
            )));
        }
        Ok(rows.into_iter().map(|r| r.1).collect())
    }

    fn json_post(&self, ep: &Endpoint, path: &str, body: Vec<u8>) -> HttpRequest {
        let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        headers.extend(auth_headers(ep));
        HttpRequest {
            method: Method::Post,
            url: ep.url(path),
            headers,
            body,
        }
    }
}

struct StreamState {
    chunks: Chunks,
    sse: Sse,
    decoder: Decoder,
    pending: VecDeque<Event>,
    ended: bool,
}
