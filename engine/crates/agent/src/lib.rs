//! The agent loop: append-only session logs, the endpoint client, tool dispatch and
//! usage metering. `world` holds state and rules; this crate makes the model calls
//! `world` asks for.

pub mod endpoint;
pub mod error;
pub mod params;
pub mod persona;
pub mod stream;
pub mod text;
pub mod wire;

pub use error::{Error, Result};
