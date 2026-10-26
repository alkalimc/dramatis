//! The agent crate behind the api crate's port. `agent` depends on `api` for the seam
//! types, so `api` cannot name it; this binary sits above both and joins them.
//!
//! The agent is built on first use (its retrieval index takes longer to set up than the
//! first screen may), on a blocking thread, with its own database connection: the file is
//! in WAL mode, so it interleaves with the engine's.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent::{Agent, Config, Parts};
use api::endpoints::Endpoints;
use api::error::{ApiError, ApiResult};
use api::home::Home;
use api::params::Params;
use api::port::AgentPort;
use api::types::{ChannelId, MessageId, PersonId, TaskId};
use api::views::ImagePart;
use async_trait::async_trait;
use tokio::sync::OnceCell;

pub struct Bridge {
    home: Home,
    corpus: PathBuf,
    params: Params,
    agent_params: agent::params::Params,
    endpoints: Mutex<Endpoints>,
    llm: llm::Llm,
    sink: Arc<dyn agent::Sink>,
    clock: Arc<dyn agent::Clock>,
    agent: OnceCell<Arc<Agent>>,
}

/// The agent's own parameter groups, from the same optional `params.toml`.
fn agent_params(home: &Home) -> agent::params::Params {
    std::fs::read_to_string(home.params())
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn failed(what: &str, e: impl std::fmt::Display) {
    eprintln!("dramatis: {what}: {e}");
}

impl Bridge {
    pub fn new(
        home: Home,
        corpus: PathBuf,
        params: Params,
        endpoints: Endpoints,
        llm: llm::Llm,
        sink: Arc<dyn agent::Sink>,
        clock: Arc<dyn agent::Clock>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_params: agent_params(&home),
            home,
            corpus,
            params,
            endpoints: Mutex::new(endpoints),
            llm,
            sink,
            clock,
            agent: OnceCell::new(),
        })
    }

    /// The agent, built if nobody has yet.
    pub async fn agent(&self) -> ApiResult<Arc<Agent>> {
        self.agent
            .get_or_try_init(|| async {
                let parts = self.parts()?;
                tauri::async_runtime::spawn_blocking(move || Agent::new(parts))
                    .await
                    .map_err(|e| ApiError::Io {
                        detail: e.to_string(),
                    })?
                    .map(Arc::new)
                    .map_err(ApiError::from)
            })
            .await
            .cloned()
    }

    fn parts(&self) -> ApiResult<Parts> {
        let io = |e: &dyn std::fmt::Display| ApiError::Io {
            detail: e.to_string(),
        };
        let world = world::World::open(self.home.world()).map_err(|e| io(&e))?;
        let index =
            index::Index::open(&self.corpus, self.params.retrieve.clone()).map_err(|e| io(&e))?;
        Ok(Parts {
            world,
            index,
            config: Config {
                world: self.params.world.clone(),
                find_people: self.params.find_people.clone(),
                agent: self.agent_params.clone(),
                office: self.home.office(),
            },
            endpoints: self
                .endpoints
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            keys: None,
            llm: self.llm.clone(),
            sink: self.sink.clone(),
            clock: self.clock.clone(),
        })
    }

    /// Run `f` in the background on the agent; failures go to stderr, since whatever the
    /// user needs to see already arrived as events.
    fn spawn<F, Fut>(self: &Arc<Self>, what: &'static str, f: F)
    where
        F: FnOnce(Arc<Agent>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = agent::Result<()>> + Send + 'static,
    {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            match me.agent().await {
                Ok(a) => {
                    if let Err(e) = f(a).await {
                        failed(what, e);
                    }
                }
                Err(e) => failed(what, e),
            }
        });
    }

    /// One scheduled step: idle conversations wrap up, due seeds may open.
    pub async fn tick(self: &Arc<Self>) {
        if let Ok(a) = self.agent().await
            && let Err(e) = a.tick().await
        {
            failed("tick", e);
        }
    }
}

fn parse<T: std::str::FromStr>(s: &str) -> ApiResult<T> {
    s.parse().map_err(|_| ApiError::NotFound)
}

/// The same bridge, held by the engine as its port and by the shell for ticks.
pub struct Port(pub Arc<Bridge>);

#[async_trait]
impl AgentPort for Port {
    async fn send_message(
        &self,
        channel: ChannelId,
        text: String,
        image: Option<ImagePart>,
    ) -> ApiResult<MessageId> {
        let agent = self.0.agent().await?;
        let ch = world::ChannelId(parse(&channel.0)?);
        let posted = agent.post(ch, &text, image.map(Into::into))?;
        let id = api::types::MessageId(posted.message.0.to_string());
        self.0
            .spawn("reply", move |a| async move { a.respond(posted).await });
        Ok(id)
    }

    async fn relay(&self, excerpt: String, _from: MessageId, to: PersonId) -> ApiResult<ChannelId> {
        let agent = self.0.agent().await?;
        let posted = agent.relay(&excerpt, &world::PersonId(to.0))?;
        let ch = api::types::ChannelId(posted.channel.0.to_string());
        self.0
            .spawn("relay", move |a| async move { a.respond(posted).await });
        Ok(ch)
    }

    async fn ask(
        &self,
        person: PersonId,
        question: String,
        turns: Option<u32>,
        parent: Option<TaskId>,
    ) -> ApiResult<TaskId> {
        let agent = self.0.agent().await?;
        let parent = parent.map(|p| parse(&p.0).map(world::TaskId)).transpose()?;
        let asked = agent.ask(&world::PersonId(person.0), &question, turns, parent)?;
        let id = api::types::TaskId(asked.task.0.to_string());
        self.0.spawn(
            "request",
            move |a| async move { a.run_request(asked).await },
        );
        Ok(id)
    }

    async fn presence(&self, opened: bool) -> ApiResult<()> {
        self.0.spawn("presence", move |a| async move {
            a.presence(opened).await.map(drop)
        });
        Ok(())
    }

    async fn on_event(&self) -> ApiResult<()> {
        self.0.spawn(
            "event seeds",
            |a| async move { a.on_event().await.map(drop) },
        );
        Ok(())
    }

    async fn on_rename(&self) -> ApiResult<()> {
        self.0
            .spawn("rename", |a| async move { a.on_rename().await });
        Ok(())
    }

    fn set_endpoints(&self, endpoints: &Endpoints) {
        *self.0.endpoints.lock().unwrap_or_else(|e| e.into_inner()) = endpoints.clone();
        if let Some(a) = self.0.agent.get() {
            a.set_endpoints(endpoints.clone());
        }
    }
}
