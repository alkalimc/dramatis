//! The engine's model-facing half: one [`Agent`] per opened world and corpus.
//!
//! Every flow appends to a channel's current log segment, calls the chat endpoint on the
//! whole segment, dispatches the tool calls and appends their results. Nothing sent is
//! ever rewritten; a segment ends only by rolling over into a new one.
//!
//! Locks: the world and the corpus are each behind a mutex that is only held between
//! awaits, always taken in that order (world, then corpus). Flows are ordered per
//! channel: a flow that writes a channel's log or messages holds that channel's async
//! lock ([`Agent::lock_channel`]) for the whole turn, so a log is never appended to by
//! two flows at once while different channels (and the host) run side by side.
//!
//! Lock order, outermost first: `planner`, then one channel lock, then the std mutexes
//! (world, corpus, caches), none of which is held across an await. No flow holds two
//! channel locks: a flow that touches several channels takes them one at a time, so
//! there is no order among channels to get wrong. `planner` only keeps the flows that
//! choose who speaks unprompted (presence, event seeds) from planning the same seed twice;
//! it is never taken while a channel lock is held.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use api::endpoints::Endpoints;
use api::events::{
    MessageAdded, MessageDelta, ModeChanged, Notification, QuotaChanged, Rewound, ToolCalled,
    TurnActivity, WorldChange, WorldChanged,
};
use api::tools::Descriptions;
use index::Index;
use world::clock::Now;
use world::{ChannelId, SegmentId, World};

use crate::assemble;
use crate::endpoint::{self, ChatRole, Keys};
use crate::error::{Error, Result};
use crate::params;
use crate::persona::{self, HostLayers, Personas};
use crate::session::LogKind;
use crate::text::{Names, Wording};

/// Everything pushed to the UI, as the api event types.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    MessageDelta(MessageDelta),
    TurnActivity(TurnActivity),
    MessageAdded(MessageAdded),
    ToolCalled(ToolCalled),
    WorldChanged(WorldChanged),
    ModeChanged(ModeChanged),
    QuotaChanged(QuotaChanged),
    Rewound(Rewound),
    Notification(Notification),
}

/// Where events go; the app forwards them to the window. Called with no lock held, so
/// it may read through the agent's read side ([`Agent::with_world`], [`Agent::history`]);
/// it must not wait for a flow to finish.
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

/// The front end the user is on. The host's usage guide covers only that one's operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Frontend {
    #[default]
    App,
    Terminal,
}

/// The parameter groups the agent reads, and where request notes go.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub frontend: Frontend,
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
    /// Where profile keys come from; `None`: the system keychain.
    pub keys: Option<Arc<dyn Keys>>,
    pub llm: llm::Llm,
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
    /// The corpus is written in CJK text: a reply with none is not the person's.
    pub cjk: bool,
    pub descriptions: Descriptions,
}

/// The model a listing call names: any, since `GET /models` takes none.
fn llm_model(p: &api::endpoints::Profile) -> api::endpoints::Model {
    p.models.first().cloned().unwrap_or_default()
}

pub struct Agent {
    pub(crate) world: Mutex<World>,
    pub(crate) index: Mutex<Index>,
    pub(crate) corpus: Corpus,
    pub(crate) config: Config,
    pub(crate) endpoints: Mutex<Endpoints>,
    pub(crate) keys: Option<Arc<dyn Keys>>,
    pub(crate) llm: llm::Llm,
    pub(crate) sink: Arc<dyn Sink>,
    pub(crate) clock: Arc<dyn Clock>,
    /// One lock per channel, created on first use. A turn, a wrap-up or a rollover of a
    /// channel holds its lock; entries are never removed (one small allocation per channel).
    locks: Mutex<HashMap<ChannelId, Arc<tokio::sync::Mutex<()>>>>,
    /// Serialises the flows that plan unprompted speech; see the module docs.
    pub(crate) planner: tokio::sync::Mutex<()>,
    /// Held while the quota status is read and sent, so a stale status never overtakes
    /// a fresher one at the UI.
    pub(crate) quota_events: Mutex<()>,
    /// Input plus output tokens of the last call per segment: how full the context is.
    /// Lost on restart, when the segment's bytes are estimated instead.
    pub(crate) filled: Mutex<HashMap<SegmentId, u64>>,
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
            cjk: manifest.segmenter_name() == "jieba"
                || manifest
                    .wording
                    .values()
                    .any(|v| v.chars().any(assemble::is_cjk)),
            descriptions,
            wording,
        };
        // Nothing runs yet: a request still open was cut off by a crash or a quit.
        let now = parts.clock.now().ms;
        world::task::interrupt_open(&parts.world, now)?;
        Ok(Self {
            world: Mutex::new(parts.world),
            index: Mutex::new(parts.index),
            corpus,
            config: parts.config,
            endpoints: Mutex::new(parts.endpoints),
            keys: parts.keys,
            llm: parts.llm,
            sink: parts.sink,
            clock: parts.clock,
            locks: Mutex::new(HashMap::new()),
            planner: tokio::sync::Mutex::new(()),
            quota_events: Mutex::new(()),
            filled: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn db(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn ix(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn channel_lock(&self, ch: ChannelId) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(ch)
            .or_default()
            .clone()
    }

    /// Wait for a channel's turn. Waiters are served in the order they asked, which is
    /// what keeps two messages to one channel answered in order.
    pub(crate) async fn lock_channel(&self, ch: ChannelId) -> tokio::sync::OwnedMutexGuard<()> {
        self.channel_lock(ch).lock_owned().await
    }

    /// The channel's turn if nobody holds it.
    pub(crate) fn try_lock_channel(
        &self,
        ch: ChannelId,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        self.channel_lock(ch).try_lock_owned().ok()
    }

    /// Channels with a flow in progress right now.
    pub(crate) fn busy_channels(&self) -> Vec<ChannelId> {
        let locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .iter()
            .filter(|(_, l)| l.try_lock().is_err())
            .map(|(c, _)| *c)
            .collect()
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

    pub(crate) fn chat(&self) -> Result<(ChatRole, llm::Endpoint)> {
        self.role_for(LogKind::Person)
    }

    /// The role and endpoint of a log kind: the host's own log uses the host role
    /// (the chat role where none is set), every other log the chat role. A group log
    /// belongs to its persons, whoever else is a member.
    pub(crate) fn role_for(&self, kind: LogKind) -> Result<(ChatRole, llm::Endpoint)> {
        let endpoints = self.endpoints();
        let role = match kind {
            LogKind::Person => ChatRole::resolve(&endpoints)?,
            LogKind::Host => ChatRole::resolve_host(&endpoints)?,
        };
        let key = endpoint::key(self.keys.as_deref(), &role.profile.name)?;
        let ep = endpoint::endpoint(&role.profile, &role.model, key);
        Ok((role, ep))
    }

    /// `GET {base_url}/models` for a profile.
    pub async fn fetch_models(&self, profile: &str) -> Result<Vec<String>> {
        let endpoints = self.endpoints();
        let p = endpoints
            .profile(profile)
            .ok_or_else(|| Error::Invalid(format!("no profile `{profile}`")))?
            .clone();
        let key = endpoint::key(self.keys.as_deref(), &p.name)?;
        let ep = endpoint::endpoint(&p, &llm_model(&p), key);
        Ok(self.llm.models(&ep).await?)
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
