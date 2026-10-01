//! The engine's model-facing half: one [`Agent`] per opened world and corpus.
//!
//! Every flow appends to a channel's current log segment, calls the chat endpoint on the
//! whole segment, dispatches the tool calls and appends their results. Nothing sent is
//! ever rewritten; a segment ends only by rolling over into a new one.
//!
//! Locks: the world and the corpus are each behind a mutex that is only held between
//! awaits, always taken in that order (world, then corpus). One flow runs at a time
//! (`turns`), so a log is never appended to by two flows at once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use api::endpoints::Endpoints;
use api::events::{
    MessageAdded, MessageDelta, ModeChanged, Notification, QuotaChanged, ToolCalled, WorldChange,
    WorldChanged,
};
use api::tools::Descriptions;
use index::Index;
use world::clock::Now;
use world::{SegmentId, World};

use crate::endpoint::{ChatRole, Keys, SharedTransport, Target};
use crate::error::{Error, Result};
use crate::params;
use crate::persona::{self, HostLayers, Personas};
use crate::text::{Names, Wording};

/// Everything pushed to the UI, as the api event types.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    MessageDelta(MessageDelta),
    MessageAdded(MessageAdded),
    ToolCalled(ToolCalled),
    WorldChanged(WorldChanged),
    ModeChanged(ModeChanged),
    QuotaChanged(QuotaChanged),
    Notification(Notification),
}

/// Where events go; the app forwards them to the window.
pub trait Sink: Send + Sync {
    fn emit(&self, event: Event);
}

/// Drops everything.
pub struct NoSink;

impl Sink for NoSink {
    fn emit(&self, _: Event) {}
}

/// The current instant and the user's UTC offset.
pub trait Clock: Send + Sync {
    fn now(&self) -> Now;
}

/// The system clock at a fixed offset. The app supplies the local offset (it changes
/// with daylight saving, which this crate does not track).
pub struct SystemClock {
    pub offset_min: i32,
}

impl Clock for SystemClock {
    fn now(&self) -> Now {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Now::new(ms, self.offset_min)
    }
}

/// The parameter groups the agent reads, and where request notes go.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub world: world::params::Params,
    pub find_people: index::params::FindPeople,
    pub agent: params::Params,
    /// `$DRAMATIS_HOME/office`.
    pub office: PathBuf,
}

/// What [`Agent::new`] is built from.
pub struct Parts {
    pub world: World,
    pub index: Index,
    pub config: Config,
    pub endpoints: Endpoints,
    pub keys: Arc<dyn Keys>,
    pub transport: SharedTransport,
    pub sink: Arc<dyn Sink>,
    pub clock: Arc<dyn Clock>,
}

/// Corpus-level values read once at open.
pub(crate) struct Corpus {
    pub wording: Wording,
    pub placeholder: String,
    pub scene: (String, String),
    pub personas: Personas,
    pub host_name: String,
    pub host: HostLayers,
    pub year_offset: i32,
    pub descriptions: Descriptions,
}

pub struct Agent {
    pub(crate) world: Mutex<World>,
    pub(crate) index: Mutex<Index>,
    pub(crate) corpus: Corpus,
    pub(crate) config: Config,
    pub(crate) endpoints: Mutex<Endpoints>,
    pub(crate) keys: Arc<dyn Keys>,
    pub(crate) transport: SharedTransport,
    pub(crate) sink: Arc<dyn Sink>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) turns: tokio::sync::Mutex<()>,
    /// The last harness context line appended per segment. Lost on restart, which costs
    /// one repeated line.
    pub(crate) contexts: Mutex<HashMap<SegmentId, String>>,
}

impl Agent {
    pub fn new(parts: Parts) -> Result<Self> {
        let folio = parts.index.folio();
        let manifest = folio.manifest();
        let wording = Wording::new(&manifest.wording);
        let descriptions: Descriptions = manifest
            .wording
            .iter()
            .filter(|(k, _)| k.starts_with("tool."))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let corpus = Corpus {
            host_name: wording.get("host.name").to_owned(),
            placeholder: manifest.user_placeholder.clone(),
            scene: manifest.scene_marker.clone(),
            personas: Personas::load(folio)?,
            host: persona::host(folio)?,
            year_offset: manifest.year_offset as i32,
            descriptions,
            wording,
        };
        Ok(Self {
            world: Mutex::new(parts.world),
            index: Mutex::new(parts.index),
            corpus,
            config: parts.config,
            endpoints: Mutex::new(parts.endpoints),
            keys: parts.keys,
            transport: parts.transport,
            sink: parts.sink,
            clock: parts.clock,
            turns: tokio::sync::Mutex::new(()),
            contexts: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn db(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn ix(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn now(&self) -> Now {
        self.clock.now()
    }

    pub(crate) fn emit(&self, e: Event) {
        self.sink.emit(e);
    }

    pub(crate) fn changed(&self, changes: Vec<WorldChange>) {
        if !changes.is_empty() {
            self.emit(Event::WorldChanged(WorldChanged { changes }));
        }
    }

    /// Run `f` on the world database: read-side commands share the agent's connection.
    /// `f` must not call back into the agent (the lock is not reentrant).
    pub fn with_world<T>(&self, f: impl FnOnce(&World) -> T) -> T {
        f(&self.db())
    }

    /// Run `f` on the retriever and its corpus. Same rule as [`Agent::with_world`]; when
    /// both are needed, take the world first.
    pub fn with_index<T>(&self, f: impl FnOnce(&Index) -> T) -> T {
        f(&self.ix())
    }

    /// A channel's messages as the UI renders them, oldest first.
    pub fn history(
        &self,
        channel: world::ChannelId,
        before: Option<world::MessageId>,
        limit: u32,
    ) -> Result<Vec<api::views::Message>> {
        let offset = self.now().offset_min;
        let msgs = world::message::history(&self.db(), channel, before, limit)?;
        Ok(msgs
            .iter()
            .map(|m| crate::view::message(m, offset, &self.config.office))
            .collect())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Whether a person may have a session: he has a persona in the corpus.
    pub fn has_persona(&self, person: &str) -> bool {
        self.corpus.personas.has_persona(person)
    }

    pub fn personas(&self) -> &Personas {
        &self.corpus.personas
    }

    /// How the user is named in corpus text right now (for display as well).
    pub fn names(&self) -> Result<Names> {
        let name = world::settings::user_name(&self.db())?;
        Ok(Names::new(
            &self.corpus.placeholder,
            name.as_deref(),
            &self.corpus.wording,
        ))
    }

    pub fn wording(&self) -> &Wording {
        &self.corpus.wording
    }

    /// The endpoints the next call uses; switching takes effect at the next call.
    pub fn set_endpoints(&self, endpoints: Endpoints) {
        *self.endpoints.lock().unwrap_or_else(|e| e.into_inner()) = endpoints;
    }

    pub fn endpoints(&self) -> Endpoints {
        self.endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn chat(&self) -> Result<(ChatRole, Target)> {
        let endpoints = self.endpoints();
        let role = ChatRole::resolve(&endpoints)?;
        let target = crate::endpoint::target(&role.profile, self.keys.as_ref())?;
        Ok((role, target))
    }

    /// `GET {base_url}/models` for a profile.
    pub async fn fetch_models(&self, profile: &str) -> Result<Vec<String>> {
        let endpoints = self.endpoints();
        let p = endpoints
            .profile(profile)
            .ok_or_else(|| Error::Invalid(format!("no profile `{profile}`")))?
            .clone();
        let target = crate::endpoint::target(&p, self.keys.as_ref())?;
        self.transport.models(&target).await
    }

    /// Display name of a person, the id when the roster lacks him.
    pub(crate) fn display(&self, person: &str) -> Result<String> {
        Ok(self
            .ix()
            .folio()
            .person(person)?
            .map(|p| p.display)
            .unwrap_or_else(|| person.to_owned()))
    }
}
