//! The agent loop: append-only session logs, the endpoint client, tool dispatch and
//! usage metering. `world` holds state and rules; this crate makes the model calls
//! `world` asks for.
//!
//! The app holds one [`Agent`] (behind an `Arc`) and calls its flows: [`Agent::post`] +
//! [`Agent::respond`] for a user turn, [`Agent::wrapup`], [`Agent::presence`], [`Agent::on_event`] and
//! [`Agent::on_rename`]. Everything the UI hears arrives through the [`Sink`].

mod address;
pub mod agent;
pub mod assemble;
pub mod diag;
mod dispatch;
pub mod endpoint;
pub mod error;
mod events;
mod flows;
mod leak;
pub mod meter;
pub mod params;
pub mod persona;
mod picture;
pub mod request;
mod session;
pub mod text;
pub mod view;
mod wrapup_json;

pub use agent::{Agent, Clock, Config, Event, Frontend, NoSink, Parts, Sink};
pub use error::{Error, Result};
pub use flows::{Forward, Posted, PresenceOutcome};
