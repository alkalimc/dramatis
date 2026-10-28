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
            ..Ports::default()
        };
        let bridge = folio.as_ref().map(|path| {
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
            })
        });
        if let Some(b) = &bridge {
            ports.agent = Arc::new(bridge::Port(b.clone()));
        }
        let engine = Arc::new(Engine::open(home, folio, ports)?);
        Ok(Self { engine, bridge })
    }
}
