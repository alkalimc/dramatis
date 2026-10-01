//! The agent loop: append-only session logs, the endpoint client, tool dispatch and
//! usage metering. `world` holds state and rules; this crate makes the model calls
//! `world` asks for.
//!
//! The app holds one [`Agent`] (behind an `Arc`) and calls its flows: [`Agent::post`] +
//! [`Agent::respond`] for a user turn, [`Agent::ask`] + [`Agent::run_request`] for a
//! request, [`Agent::wrapup`], [`Agent::presence`], [`Agent::on_event`] and
//! [`Agent::on_rename`]. Everything the UI hears arrives through the [`Sink`].

pub mod agent;
pub mod assemble;
mod dispatch;
pub mod endpoint;
pub mod error;
mod flows;
pub mod meter;
pub mod params;
pub mod persona;
mod session;
pub mod store;
pub mod stream;
pub mod text;
pub mod view;
pub mod wire;

pub use agent::{Agent, Clock, Config, Event, NoSink, Parts, Sink, SystemClock};
pub use error::{Error, Result};
pub use flows::{Posted, PresenceOutcome};
pub use session::open_prefix;
