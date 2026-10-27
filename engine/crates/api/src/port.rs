//! Everything the engine reaches outside its own files, behind traits the app supplies.
//!
//! * [`AgentPort`]: whatever needs a model call (a user turn and its replies, a relay, a
//!   request, the host's line and the openings on presence, event seeds, the rollover
//!   after a rename). The app implements it over the `agent` crate, which depends on this
//!   crate for the seam types, so this crate never names it.
//! * [`LlmPort`]: the endpoint calls that are not a session (`GET /models`, voices,
//!   `/audio/speech`), implemented over the shared LLM client.
//! * [`KeyStore`]: the system keychain. [`NetPort`]: plain fetches from the corpus's
//!   source site (avatars) and whether the chat endpoint can be reached at all.
//! * [`Sink`]: events to the window. [`Clock`]: the instant and the user's UTC offset.
//!
//! Until a port is wired, [`Unwired`] stands in: it calls nothing, and the app stays in
//! library form, truthfully.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use world::clock::Now;

use crate::endpoints::{Endpoints, Profile};
use crate::error::{ApiError, ApiResult};
use crate::events::{
    MessageAdded, MessageDelta, ModeChanged, Notification, QuotaChanged, RosterChanged, ToolCalled,
    TurnActivity, WorldChanged,
};
use crate::types::{ChannelId, MessageId, PersonId, TaskId};
use crate::views::ImagePart;

#[async_trait]
pub trait AgentPort: Send + Sync + 'static {
    /// Whether anything is behind this port. `false` keeps the app in library form.
    fn wired(&self) -> bool {
        true
    }

    /// Store and echo the user's message (`MessageAdded`), return its id at once, and run
    /// the replies in the background (streamed as events).
    async fn send_message(
        &self,
        channel: ChannelId,
        text: String,
        image: Option<ImagePart>,
    ) -> ApiResult<MessageId>;

    /// Retell an excerpt to someone else; their answer lands in their direct channel,
    /// which is returned at once.
    async fn relay(&self, excerpt: String, from: MessageId, to: PersonId) -> ApiResult<ChannelId>;

    /// Create the request and return it at once; the work runs in the background.
    async fn ask(
        &self,
        person: PersonId,
        question: String,
        turns: Option<u32>,
        parent: Option<TaskId>,
    ) -> ApiResult<TaskId>;

    /// The user is present (`opened`: the app was just opened). The digest is already on
    /// screen; this starts what presence asks of the model in the background: wrap-ups of
    /// conversations that ended while away, the host's one line, at most one opening.
    async fn presence(&self, opened: bool) -> ApiResult<()>;

    /// Something happened that may seed an opening outside presence (a commitment fell
    /// due, a conclusion was written, a question pinned). Runs in the background.
    async fn on_event(&self) -> ApiResult<()>;

    /// The user's name changed: every session rolls over so no log keeps the old one.
    async fn on_rename(&self) -> ApiResult<()>;

    /// The endpoints changed; the next call uses these.
    fn set_endpoints(&self, endpoints: &Endpoints);
}

/// Synthesised speech, ready to cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speech {
    pub bytes: Vec<u8>,
    /// File extension for the format the endpoint returned (`mp3`, `wav`, ...).
    pub ext: String,
}

#[async_trait]
pub trait LlmPort: Send + Sync + 'static {
    /// `GET {base_url}/models`. A failure carries the endpoint's own text.
    async fn models(&self, profile: &Profile) -> ApiResult<Vec<String>>;
    /// The voices the read-aloud endpoint offers; empty when it lists none.
    async fn voices(&self, profile: &Profile, model: &str) -> ApiResult<Vec<String>>;
    /// `POST {base_url}/audio/speech` for one message's speech.
    async fn speech(
        &self,
        profile: &Profile,
        model: &str,
        voice: Option<&str>,
        text: &str,
    ) -> ApiResult<Speech>;
}

/// Profile keys. Implementations never hand a key back through this trait.
pub trait KeyStore: Send + Sync + 'static {
    fn set(&self, profile: &str, key: &str) -> ApiResult<()>;
    fn has(&self, profile: &str) -> bool;
    fn delete(&self, profile: &str) -> ApiResult<()>;
    /// Move a key to a renamed profile (no-op when there is none).
    fn rename(&self, from: &str, to: &str) -> ApiResult<()>;
}

#[async_trait]
pub trait NetPort: Send + Sync + 'static {
    /// GET a URL. `Ok(None)`: the server answered without the file; `Err`: no answer
    /// (offline, timed out), worth trying again later.
    async fn get(&self, url: &str) -> Result<Option<Vec<u8>>, String>;
    /// Whether a connection to the endpoint's host can be opened. Sends nothing.
    async fn reachable(&self, base_url: &str) -> bool;
}

/// Everything the engine pushes to the window, as the seam's event types.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    MessageDelta(MessageDelta),
    TurnActivity(TurnActivity),
    MessageAdded(MessageAdded),
    ToolCalled(ToolCalled),
    WorldChanged(WorldChanged),
    ModeChanged(ModeChanged),
    RosterChanged(RosterChanged),
    QuotaChanged(QuotaChanged),
    Notification(Notification),
}

pub trait Sink: Send + Sync + 'static {
    fn emit(&self, event: Event);
}

/// Drops everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSink;

impl Sink for NoSink {
    fn emit(&self, _: Event) {}
}

/// Keeps every event, for tests.
#[derive(Debug, Default)]
pub struct Recorder(pub Mutex<Vec<Event>>);

impl Recorder {
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Sink for Recorder {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(event);
    }
}

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Now;
}

/// The wall clock at the current local offset (read on every call, so daylight saving
/// changes take effect without a restart).
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalClock;

impl Clock for LocalClock {
    fn now(&self) -> Now {
        let t = chrono::Local::now();
        Now::new(t.timestamp_millis(), t.offset().local_minus_utc() / 60)
    }
}

/// A fixed instant, for tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub Now);

impl Clock for FixedClock {
    fn now(&self) -> Now {
        self.0
    }
}

/// No agent and no LLM client behind the port.
///
/// TODO(integrator): once `agent` and `llm` are on main, implement [`AgentPort`] in
/// `bins/dramatis` over `Arc<agent::Agent>` (`post` + spawned `respond` for
/// `send_message`; `relay` likewise; `ask` + spawned `run_request`; `presence`,
/// `on_event`, `on_rename` spawned; `set_endpoints` one to one), and [`LlmPort`] over
/// `llm` (`models`, `voices`, `speech`). Then pass them in `Ports` instead of this.
#[derive(Debug, Default, Clone, Copy)]
pub struct Unwired;

#[async_trait]
impl AgentPort for Unwired {
    fn wired(&self) -> bool {
        false
    }

    async fn send_message(
        &self,
        _: ChannelId,
        _: String,
        _: Option<ImagePart>,
    ) -> ApiResult<MessageId> {
        Err(ApiError::NotImplemented)
    }

    async fn relay(&self, _: String, _: MessageId, _: PersonId) -> ApiResult<ChannelId> {
        Err(ApiError::NotImplemented)
    }

    async fn ask(
        &self,
        _: PersonId,
        _: String,
        _: Option<u32>,
        _: Option<TaskId>,
    ) -> ApiResult<TaskId> {
        Err(ApiError::NotImplemented)
    }

    async fn presence(&self, _: bool) -> ApiResult<()> {
        Ok(())
    }

    async fn on_event(&self) -> ApiResult<()> {
        Ok(())
    }

    async fn on_rename(&self) -> ApiResult<()> {
        Ok(())
    }

    fn set_endpoints(&self, _: &Endpoints) {}
}

#[async_trait]
impl LlmPort for Unwired {
    async fn models(&self, _: &Profile) -> ApiResult<Vec<String>> {
        Err(ApiError::NotImplemented)
    }

    async fn voices(&self, _: &Profile, _: &str) -> ApiResult<Vec<String>> {
        Err(ApiError::NotImplemented)
    }

    async fn speech(&self, _: &Profile, _: &str, _: Option<&str>, _: &str) -> ApiResult<Speech> {
        Err(ApiError::NotImplemented)
    }
}

/// Keys in memory: tests, and hosts without a keychain.
#[derive(Debug, Default)]
pub struct MemoryKeys(Mutex<BTreeMap<String, String>>);

impl MemoryKeys {
    fn map(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl KeyStore for MemoryKeys {
    fn set(&self, profile: &str, key: &str) -> ApiResult<()> {
        self.map().insert(profile.to_owned(), key.to_owned());
        Ok(())
    }

    fn has(&self, profile: &str) -> bool {
        self.map().contains_key(profile)
    }

    fn delete(&self, profile: &str) -> ApiResult<()> {
        self.map().remove(profile);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> ApiResult<()> {
        let mut map = self.map();
        if let Some(k) = map.remove(from) {
            map.insert(to.to_owned(), k);
        }
        Ok(())
    }
}

/// No network: every fetch fails, nothing is reachable.
#[derive(Debug, Default, Clone, Copy)]
pub struct Offline;

#[async_trait]
impl NetPort for Offline {
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
        Err("offline".into())
    }

    async fn reachable(&self, _: &str) -> bool {
        false
    }
}
