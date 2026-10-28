//! The agent crate behind the api crate's port. `agent` depends on `api` for the seam
//! types, so `api` cannot name it; this crate sits above both and joins them.
//!
//! The agent is built on first use (its retrieval index takes longer to set up than the
//! first screen may), on a blocking thread, with its own database connection: the file is
//! in WAL mode, so it interleaves with the engine's. Model-driven work runs as background
//! tokio tasks; the front end learns what happened from events, and may also wait for the
//! work to finish or abandon it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent::endpoint::Keys;
use agent::{Agent, Config, Frontend, Parts};
use api::endpoints::Endpoints;
use api::error::{ApiError, ApiResult};
use api::home::Home;
use api::params::Params;
use api::port::AgentPort;
use api::types::{ChannelId, MessageId, PersonId, TaskId};
use api::views::ImagePart;
use async_trait::async_trait;
use tokio::sync::watch;
use tokio::task::AbortHandle;

/// One model call, from the meter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub id: i64,
    /// When the call finished (epoch ms).
    pub at: i64,
    pub shape: String,
    pub model: String,
    pub uncached: i64,
    pub cached: i64,
    pub output: i64,
}

/// What the bridge is made of beyond the home and the corpus.
pub struct BridgeParts {
    pub home: Home,
    pub corpus: PathBuf,
    pub params: Params,
    pub endpoints: Endpoints,
    pub llm: llm::Llm,
    pub sink: Arc<dyn agent::Sink>,
    pub clock: Arc<dyn agent::Clock>,
    /// Where profile keys come from; `None`: the system keychain.
    pub keys: Option<Arc<dyn Keys>>,
    /// Keep background failures for [`AgentBridge::take_failures`] instead of printing
    /// them to stderr.
    pub quiet: bool,
    /// Which front end the user is on.
    pub frontend: Frontend,
}

/// The background work in flight.
#[derive(Default)]
struct Tasks {
    next: u64,
    live: HashMap<u64, AbortHandle>,
}

/// Removes a task from the books when it ends, whether it finished or was aborted.
struct Done {
    bridge: Arc<AgentBridge>,
    id: u64,
}

impl Drop for Done {
    fn drop(&mut self) {
        let mut tasks = self.bridge.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.live.remove(&self.id);
        self.bridge.busy.send_replace(tasks.live.len());
    }
}

pub struct AgentBridge {
    parts: BridgeParts,
    agent_params: agent::params::Params,
    endpoints: Mutex<Endpoints>,
    /// The agent, once built; cleared when the world it works on changes.
    agent: Mutex<Option<Arc<Agent>>>,
    /// Serialises building it.
    building: tokio::sync::Mutex<()>,
    /// The world database the agent opens: the save this process was started on.
    world: Mutex<PathBuf>,
    tasks: Mutex<Tasks>,
    failures: Mutex<Vec<String>>,
    /// How many background tasks are running.
    busy: watch::Sender<usize>,
}

/// The agent's own parameter groups, from the same optional `params.toml`.
/// Malformed or unreadable is an error, as for the api's own groups: an ignored override
/// would make a calibration run meaningless.
fn agent_params(home: &Home) -> ApiResult<agent::params::Params> {
    let path = home.params();
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| io(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(io(format!("{}: {e}", path.display()))),
    }
}

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io {
        detail: e.to_string(),
    }
}

impl AgentBridge {
    pub fn new(parts: BridgeParts) -> ApiResult<Arc<Self>> {
        Ok(Arc::new(Self {
            agent_params: agent_params(&parts.home)?,
            world: Mutex::new(parts.home.world()),
            endpoints: Mutex::new(parts.endpoints.clone()),
            parts,
            agent: Mutex::new(None),
            building: tokio::sync::Mutex::new(()),
            tasks: Mutex::default(),
            failures: Mutex::default(),
            busy: watch::channel(0).0,
        }))
    }

    fn failed(&self, what: &str, e: impl std::fmt::Display) {
        if self.parts.quiet {
            let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
            failures.push(format!("{what}: {e}"));
        } else {
            eprintln!("dramatis: {what}: {e}");
        }
    }

    /// The failures of background work since the last call (only with `quiet`): the
    /// endpoint's own text for a turn that ended without a reply.
    pub fn take_failures(&self) -> Vec<String> {
        std::mem::take(&mut *self.failures.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The agent, built if nobody has yet.
    pub async fn agent(&self) -> ApiResult<Arc<Agent>> {
        if let Some(a) = self.built() {
            return Ok(a);
        }
        let _building = self.building.lock().await;
        if let Some(a) = self.built() {
            return Ok(a);
        }
        let parts = self.agent_parts()?;
        let agent = tokio::task::spawn_blocking(move || Agent::new(parts))
            .await
            .map_err(io)?
            .map(Arc::new)
            .map_err(ApiError::from)?;
        *self.agent.lock().unwrap_or_else(|e| e.into_inner()) = Some(agent.clone());
        Ok(agent)
    }

    fn built(&self) -> Option<Arc<Agent>> {
        self.agent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn agent_parts(&self) -> ApiResult<Parts> {
        let p = &self.parts;
        let path = self.world.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // Opening would create an empty database under the old name: a save another
        // process removed or renamed must not come back as a blank one.
        if !path.is_file() {
            return Err(ApiError::NotFound);
        }
        let world = world::World::open(path).map_err(io)?;
        let index = index::Index::open(&p.corpus, p.params.retrieve.clone()).map_err(io)?;
        Ok(Parts {
            world,
            index,
            config: Config {
                frontend: p.frontend,
                world: p.params.world.clone(),
                find_people: p.params.find_people.clone(),
                agent: self.agent_params.clone(),
                office: p.home.office(),
            },
            endpoints: self
                .endpoints
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            keys: p.keys.clone(),
            llm: p.llm.clone(),
            sink: p.sink.clone(),
            clock: p.clock.clone(),
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
        // Held across the spawn so the task cannot end before it is on the books.
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let id = tasks.next;
        tasks.next += 1;
        let done = Done {
            bridge: self.clone(),
            id,
        };
        let handle = tokio::spawn(async move {
            let _done = done;
            match me.agent().await {
                Ok(a) => {
                    if let Err(e) = f(a).await {
                        me.failed(what, e);
                    }
                }
                Err(e) => me.failed(what, e),
            }
        });
        tasks.live.insert(id, handle.abort_handle());
        self.busy.send_replace(tasks.live.len());
    }

    /// Wait until no background work is running: every reply, relay and request started
    /// so far has ended.
    pub async fn idle(&self) {
        let _ = self.busy.subscribe().wait_for(|n| *n == 0).await;
    }

    /// Abandon the background work in flight. What was already stored stays stored.
    pub fn cancel(&self) {
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for handle in tasks.live.values() {
            handle.abort();
        }
    }

    /// One scheduled step: idle conversations wrap up, due seeds may open.
    pub async fn tick(self: &Arc<Self>) {
        if let Ok(a) = self.agent().await
            && let Err(e) = a.tick().await
        {
            self.failed("tick", e);
        }
    }

    /// The model calls recorded after meter row `after` (0 for all). A maintainer's view
    /// of the meter, not part of the seam: the app shows spending per shape instead.
    pub async fn calls_after(&self, after: i64) -> ApiResult<Vec<Call>> {
        self.agent()
            .await?
            .with_world(|w| {
                let mut s = w.prepare(
                    "SELECT id, at, shape, model, uncached, cached, output FROM meter \
                     WHERE id > ?1 ORDER BY id",
                )?;
                s.query_map([after], |r| {
                    Ok(Call {
                        id: r.get(0)?,
                        at: r.get(1)?,
                        shape: r.get(2)?,
                        model: r.get(3)?,
                        uncached: r.get(4)?,
                        cached: r.get(5)?,
                        output: r.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(io)
    }
}

fn parse<T: std::str::FromStr>(s: &str) -> ApiResult<T> {
    s.parse().map_err(|_| ApiError::NotFound)
}

/// The same bridge, held by the engine as its port.
pub struct Port(pub Arc<AgentBridge>);

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
        let id = MessageId(posted.message.0.to_string());
        self.0
            .spawn("reply", move |a| async move { a.respond(posted).await });
        Ok(id)
    }

    async fn relay(&self, excerpt: String, _from: MessageId, to: PersonId) -> ApiResult<ChannelId> {
        let agent = self.0.agent().await?;
        let posted = agent.relay(&excerpt, &world::PersonId(to.0))?;
        let ch = ChannelId(posted.channel.0.to_string());
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
        let id = TaskId(asked.task.0.to_string());
        self.0.spawn(
            "request",
            move |a| async move { a.run_request(asked).await },
        );
        Ok(id)
    }

    async fn rewind(&self, channel: ChannelId, message: MessageId) -> ApiResult<String> {
        let agent = self.0.agent().await?;
        let ch = world::ChannelId(parse(&channel.0)?);
        let id = world::MessageId(parse(&message.0)?);
        Ok(agent.rewind(ch, id)?)
    }

    async fn wrapup(&self, channel: ChannelId) -> ApiResult<()> {
        let ch = world::ChannelId(parse(&channel.0)?);
        self.0
            .spawn("wrap-up", move |a| async move { a.wrapup(ch).await });
        Ok(())
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
        if let Some(a) = self.0.built() {
            a.set_endpoints(endpoints.clone());
        }
    }

    fn idle(&self) -> bool {
        self.0
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .is_empty()
    }

    fn switch_world(&self, path: &std::path::Path) {
        *self.0.world.lock().unwrap_or_else(|e| e.into_inner()) = path.to_owned();
        *self.0.agent.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(tag: &str, params: Option<&str>) -> Home {
        let root =
            std::env::temp_dir().join(format!("dramatis-bridge-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root);
        home.create().unwrap();
        if let Some(text) = params {
            std::fs::write(home.params(), text).unwrap();
        }
        home
    }

    #[test]
    fn the_agents_parameters_follow_the_file_and_a_bad_one_is_an_error() {
        let absent = home_with("absent", None);
        assert_eq!(agent_params(&absent).unwrap(), Default::default());

        // Keys of other crates' groups sit in the same file.
        let mixed = home_with(
            "mixed",
            Some("[roll]\nk = 100.0\n[media]\ncache_gb = 0.5\n"),
        );
        assert_eq!(agent_params(&mixed).unwrap().roll.k, 100.0);

        let bad = home_with("bad", Some("[roll]\nk = \"many\"\n"));
        assert!(matches!(agent_params(&bad), Err(ApiError::Io { .. })));
        for h in [absent, mixed, bad] {
            std::fs::remove_dir_all(h.root()).unwrap();
        }
    }
}
