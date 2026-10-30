//! Everything the engine reaches outside its own files, behind traits the app supplies.
//!
//! * [`AgentPort`]: whatever needs a model call (a user turn and its replies, a forward, a
//!   request, a forced wrap-up, the host's line and the openings on presence, event seeds,
//!   the rollover after a rename). The `runtime` crate implements it over the `agent`
//!   crate, which depends on this crate for the seam types, so this crate never names it.
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
    AwayChanged, MessageAdded, MessageDelta, ModeChanged, Notification, QuotaChanged, Rewound,
    RosterChanged, ToolCalled, TurnActivity, WorldChanged,
};
use crate::types::{ChannelId, MessageId, PersonId};
use crate::views::{ForwardTo, ImagePart};

/// What changed about a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupChange {
    Name,
    Description,
    Members,
}

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
        reply_to: Option<MessageId>,
    ) -> ApiResult<MessageId>;

    /// Forward a message. To a person or the host it is posted as a forwarded copy in their
    /// direct channel and answered in the background; to the favorites it is kept as its
    /// author wrote it, with no model call. Returns the channel it landed in at once.
    async fn forward(&self, message: MessageId, to: ForwardTo) -> ApiResult<ChannelId>;

    /// Take back the user's message `message` and everything after it in `channel`;
    /// returns its text. Refused while a turn is running in the channel. Emits `Rewound`.
    async fn rewind(&self, channel: ChannelId, message: MessageId) -> ApiResult<String>;

    /// End the channel's conversation now and write what is worth remembering from it.
    /// Runs in the background; the facts it writes arrive as `WorldChanged`.
    async fn wrapup(&self, channel: ChannelId) -> ApiResult<()>;

    /// The user is present (`opened`: the app was just opened). The digest is already on
    /// screen; this starts what presence asks of the model in the background: wrap-ups of
    /// conversations that ended while away, the host's one line, at most one opening.
    async fn presence(&self, opened: bool) -> ApiResult<()>;

    /// Something happened that may seed an opening outside presence (a commitment fell
    /// due). Runs in the background.
    async fn on_event(&self) -> ApiResult<()>;

    /// The user's name changed: every session rolls over so no log keeps the old one.
    async fn on_rename(&self) -> ApiResult<()>;

    /// Away mode is being turned off: settle the time away and say, in one fixed line in
    /// the host's channel, what fell due meanwhile. Emits `AwayChanged`.
    async fn end_away(&self) -> ApiResult<()> {
        Ok(())
    }

    /// A group was made from a suggestion, or by the host, for `purpose`: write the
    /// group's first line saying why it exists and who suggested it (`by`, or the host
    /// when none). Fixed wording, no model call.
    async fn group_opened(
        &self,
        _channel: ChannelId,
        _by: Option<PersonId>,
        _purpose: String,
    ) -> ApiResult<()> {
        Ok(())
    }

    /// A group's name, description or members changed: say so in the group as a service
    /// line and in the model's log. A stub until the real hook lands: it does nothing.
    async fn group_changed(&self, _channel: ChannelId, _what: GroupChange) -> ApiResult<()> {
        Ok(())
    }

    /// The user took a person found for `topic` from a suggestion by `by` (or the host
    /// when none): put the reason in the person's log and show it as a service line.
    /// Once per reason; no model call.
    async fn found_opened(
        &self,
        _channel: ChannelId,
        _by: Option<PersonId>,
        _topic: String,
    ) -> ApiResult<()> {
        Ok(())
    }

    /// People joined a group or left it (the roster already changed): say so in the group,
    /// for the user and for the members' log. Fixed wording, no model call.
    async fn members_changed(
        &self,
        _channel: ChannelId,
        _joined: Vec<PersonId>,
        _left: Vec<PersonId>,
    ) -> ApiResult<()> {
        Ok(())
    }

    /// The endpoints changed; the next call uses these.
    fn set_endpoints(&self, endpoints: &Endpoints);

    /// Whether no model-driven work (a reply, a request, a wrap-up) is running.
    fn idle(&self) -> bool {
        true
    }

    /// Work on the world database at `path` from now on (a different save, or the same one
    /// after its password changed; `key` is the password of an encrypted one). Only called
    /// while [`idle`](Self::idle).
    fn switch_world(&self, _path: &std::path::Path, _key: Option<&str>) {}
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
    AwayChanged(AwayChanged),
    QuotaChanged(QuotaChanged),
    Rewound(Rewound),
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

/// The messages already announced to the window (`MessageAdded`), newest last and
/// bounded. Lets the engine tell a message its own agent wrote, which the window knows,
/// from one another process wrote, which it does not. A rewind forgets the ids it frees,
/// since they are handed out again.
#[derive(Debug, Default)]
pub struct Seen {
    ids: Mutex<std::collections::VecDeque<(String, String)>>,
    /// The lowest message id freed by a rewind since the last [`Seen::take_rewound`].
    rewound: Mutex<Option<i64>>,
}

impl Seen {
    const KEEP: usize = 1024;

    pub fn record(&self, event: &Event) {
        let mut ids = self.ids.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            Event::MessageAdded(e) => {
                ids.push_back((e.message.channel.0.clone(), e.message.id.0.clone()));
                if ids.len() > Self::KEEP {
                    ids.pop_front();
                }
            }
            Event::Rewound(e) => {
                if let Ok(from) = e.from.0.parse::<i64>() {
                    let mut low = self.rewound.lock().unwrap_or_else(|e| e.into_inner());
                    *low = Some(low.map_or(from - 1, |l| l.min(from - 1)));
                    ids.retain(|(ch, id)| {
                        *ch != e.channel.0 || id.parse::<i64>().is_ok_and(|id| id < from)
                    });
                }
            }
            _ => {}
        }
    }

    /// Forget every id above `newest`: the world no longer holds them (another process
    /// rewound), so a later message may carry the same id.
    pub fn forget_above(&self, newest: i64) {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(_, id)| id.parse::<i64>().is_ok_and(|id| id <= newest));
    }

    /// The newest id that survived the rewinds since the last call, if there were any: a
    /// watcher's cursor must not stay above it, or a message reusing a freed id is skipped.
    pub fn take_rewound(&self) -> Option<i64> {
        self.rewound
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    pub fn has(&self, id: &str) -> bool {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|(_, i)| i == id)
    }
}

/// A sink that notes what it forwards in a [`Seen`].
pub struct Tracking {
    pub inner: std::sync::Arc<dyn Sink>,
    pub seen: std::sync::Arc<Seen>,
}

impl Sink for Tracking {
    fn emit(&self, event: Event) {
        self.seen.record(&event);
        self.inner.emit(event);
    }
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

/// No agent and no LLM client behind the port: the library form.
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
        _: Option<MessageId>,
    ) -> ApiResult<MessageId> {
        Err(ApiError::NotImplemented)
    }

    async fn forward(&self, _: MessageId, _: ForwardTo) -> ApiResult<ChannelId> {
        Err(ApiError::NotImplemented)
    }

    async fn rewind(&self, _: ChannelId, _: MessageId) -> ApiResult<String> {
        Err(ApiError::NotImplemented)
    }

    async fn wrapup(&self, _: ChannelId) -> ApiResult<()> {
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
