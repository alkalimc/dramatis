//! The engine wired to the real world: the agent behind the seam's `AgentPort`, the system
//! keychain, the shared LLM client and the network. The desktop app and the terminal
//! client both build their engine here, so a command means the same thing in either.
//!
//! This is its own crate because `agent` depends on `api` for the seam types: `api`
//! cannot name the agent, and whatever joins them has to sit above both.

mod bridge;
mod ports;

use std::path::PathBuf;
use std::sync::Arc;

use api::endpoints::Endpoints;
use api::error::ApiResult;
use api::home::Home;
use api::params::Params;
use api::port::{self, Sink};
use api::{Engine, Ports};
use llm::{Retry, Transport};

pub use agent::Frontend;
pub use agent::diag;
pub use bridge::{AgentBridge, BridgeParts, Call};

/// Everything a front end holds: the engine it issues commands to, and, when a corpus is
/// open, the bridge that runs the model-driven work behind it.
pub struct Runtime {
    pub engine: Arc<Engine>,
    pub bridge: Option<Arc<AgentBridge>>,
}

/// What differs between front ends and tests: the byte transport, where keys come from and
/// where background failures go.
#[derive(Default)]
pub struct Options {
    /// `None`: HTTP.
    pub transport: Option<Arc<dyn Transport>>,
    /// Keys the agent reads before the system keychain; `None`: the keychain only.
    pub keys: Option<Arc<dyn agent::endpoint::Keys>>,
    /// See [`BridgeParts::quiet`].
    pub quiet: bool,
    /// Which front end this is; the host explains only that one's operations.
    pub frontend: Frontend,
    /// The password of the current save, when it is encrypted and the front end has it
    /// (the terminal asks before it starts; the app unlocks later through `unlock`).
    pub password: Option<String>,
    /// What to do when the engine asks to be opened again (a corpus was chosen); `None`:
    /// nothing, the front end asks the user to restart.
    pub restart: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The system's file dialog for a corpus, where the front end has one.
    pub choose_file: Option<api::engine::ChooseFile>,
    /// Opens a web address in the system browser, where the front end can.
    pub open_url: Option<api::engine::OpenUrl>,
}

/// Hands the agent's events to the engine's sink, as the seam's own event type.
pub struct Forward(pub Arc<dyn Sink>);

impl agent::Sink for Forward {
    fn emit(&self, event: agent::Event) {
        use agent::Event as A;
        use port::Event as P;
        self.0.emit(match event {
            A::MessageDelta(e) => P::MessageDelta(e),
            A::TurnActivity(e) => P::TurnActivity(e),
            A::MessageAdded(e) => P::MessageAdded(e),
            A::ToolCalled(e) => P::ToolCalled(e),
            A::WorldChanged(e) => P::WorldChanged(e),
            A::ModeChanged(e) => P::ModeChanged(e),
            A::AwayChanged(e) => P::AwayChanged(e),
            A::QuotaChanged(e) => P::QuotaChanged(e),
            A::Rewound(e) => P::Rewound(e),
            A::Notification(e) => P::Notification(e),
        });
    }
}

impl Runtime {
    /// Open the home and the corpus (if any). Every event, the engine's and the agent's,
    /// goes to `sink`.
    pub fn open(
        home: Home,
        folio: Option<PathBuf>,
        sink: Arc<dyn Sink>,
        options: Options,
    ) -> ApiResult<Self> {
        // One record of what the window has been told, shared by every event source, so
        // the engine can tell another process's messages from its own agent's.
        let seen = Arc::new(port::Seen::default());
        let sink: Arc<dyn Sink> = Arc::new(port::Tracking {
            inner: sink,
            seen: seen.clone(),
        });
        let params = Params::load(&home.params()).map_err(|detail| api::ApiError::Io { detail })?;
        let http: Arc<dyn Transport> = match options.transport {
            Some(t) => t,
            None => Arc::new(llm::Http::new().map_err(ports::failed)?),
        };
        let client = llm::Llm::new(http.clone(), Retry::default());
        let mut ports = Ports {
            llm: Arc::new(ports::Models(client.clone())),
            keys: Arc::new(ports::Keys),
            net: Arc::new(ports::Net(http)),
            sink: sink.clone(),
            seen,
            restart: options.restart.unwrap_or_else(|| Arc::new(|| {})),
            choose_file: options.choose_file,
            open_url: options.open_url,
            ..Ports::default()
        };
        let bridge = folio
            .as_ref()
            .map(|path| {
                AgentBridge::new(BridgeParts {
                    home: home.clone(),
                    corpus: path.clone(),
                    params: params.clone(),
                    endpoints: Endpoints::default(),
                    llm: client,
                    sink: Arc::new(Forward(sink)),
                    clock: Arc::new(ports::Local),
                    keys: options.keys,
                    quiet: options.quiet,
                    frontend: options.frontend,
                    password: options.password.clone(),
                })
            })
            .transpose()?;
        if let Some(b) = &bridge {
            ports.agent = Arc::new(bridge::Port(b.clone()));
        }
        let engine = Arc::new(Engine::open_with_password(
            home,
            folio,
            ports,
            options.password,
        )?);
        Ok(Self { engine, bridge })
    }
}
