//! One internal interface over the external LLM API protocols.
//!
//! Callers build a protocol-neutral [`Request`] and call [`Llm::complete`] or
//! [`Llm::stream`]; the adapters for OpenAI chat, OpenAI responses and Anthropic messages
//! live in [`render`] (request side, including each protocol's cache policy) and
//! [`decode`] (reply side). A new protocol is one more arm in each, nothing else.
//!
//! This crate depends on no other workspace crate: callers convert their own endpoint
//! description into an [`Endpoint`].

pub mod client;
pub mod decode;
pub mod error;
pub mod keychain;
pub mod render;
pub mod transport;
pub mod types;

pub use client::{Llm, Retry};
pub use error::{Error, Result};
pub use render::{Rendered, auth_headers, render, render_stream};
pub use transport::{Canned, Http, MemoryTransport, Transport};
pub use types::*;
