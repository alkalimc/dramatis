//! The engine behind the seam: every command that needs no model call, answered over the
//! world database and the corpus, and every command that does, handed to the ports.
//!
//! One [`Engine`] per opened home directory. It owns its own connections (the agent opens
//! its own too; the database uses a rollback journal and a busy timeout, so the two take
//! turns).

mod chat;
mod endpoints;
mod external;
mod library;
mod people;
mod saves;
mod storage;
mod tasks;
mod uploads;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use world::World;
use world::clock::Now;

use crate::commands::Api;
use crate::convert::{self, world_err};
use crate::corpus::{self, Corpus};
use crate::display::{self, Namer};
use crate::endpoints::{Endpoints, Profile, Role};
use crate::error::{ApiError, ApiResult};
use crate::events::{QuotaChanged, WorldChange, WorldChanged};
use crate::home::{Home, Start};
use crate::lock::SaveLock;
use crate::media::MediaCache;
use crate::params::Params;
use crate::port::{
    AgentPort, Clock, Event, KeyStore, LlmPort, LocalClock, MemoryKeys, NetPort, NoSink, Offline,
    Seen, Sink, Unwired,
};
use crate::types::*;
use crate::views::*;

/// Everything the engine reaches outside its own files.
#[derive(Clone)]
pub struct Ports {
    pub agent: Arc<dyn AgentPort>,
    pub llm: Arc<dyn LlmPort>,
    pub keys: Arc<dyn KeyStore>,
    pub net: Arc<dyn NetPort>,
    pub sink: Arc<dyn Sink>,
    pub clock: Arc<dyn Clock>,
    /// What `sink` has announced; see [`Seen`]. Wire the same one into a [`Tracking`]
    /// sink that every event source shares.
    pub seen: Arc<Seen>,
    /// Asked for when the engine itself cannot take effect (a corpus was chosen): the
    /// front end opens the home again. Library form: nothing happens.
    pub restart: Arc<dyn Fn() + Send + Sync>,
    /// Shows the system's file dialog for a corpus and answers the path chosen (`None`:
    /// cancelled). Absent where there is no dialog; the user types the path instead.
    pub choose_file: Option<ChooseFile>,
    /// Opens a web address in the system browser. Absent where there is none.
    pub open_url: Option<OpenUrl>,
}

/// See [`Ports::open_url`].
pub type OpenUrl = Arc<dyn Fn(&str) + Send + Sync>;

/// See [`Ports::choose_file`].
pub type ChooseFile = Arc<dyn Fn() -> Option<String> + Send + Sync>;

impl Default for Ports {
    /// Nothing wired: library form, no network, keys in memory.
    fn default() -> Self {
        Self {
            agent: Arc::new(Unwired),
            llm: Arc::new(Unwired),
            keys: Arc::new(MemoryKeys::default()),
            net: Arc::new(Offline),
            sink: Arc::new(NoSink),
            clock: Arc::new(LocalClock),
            seen: Arc::default(),
            restart: Arc::new(|| {}),
            choose_file: None,
            open_url: None,
        }
    }
}

/// Avatar downloads in flight at once.
const AVATAR_FETCHES: usize = 3;

pub struct Engine {
    home: Home,
    params: Params,
    ports: Ports,
    world: Mutex<World>,
    /// The name of the save `world` is; empty while none is open (`world` is then an empty
    /// stand-in, as for a locked save).
    save: Mutex<String>,
    /// The recorded last save that was not there at start, for the picker to mention.
    missing_save: Mutex<Option<String>>,
    /// The open save is encrypted and was not unlocked: `world` is an empty stand-in.
    locked: AtomicBool,
    /// The shared lock telling other processes this one has the open save (see `lock.rs`).
    held: Mutex<Option<SaveLock>>,
    /// The file of the open save was replaced, renamed, deleted or encrypted by another
    /// process: `world` is an empty stand-in, so nothing is written to the old file.
    elsewhere: AtomicBool,
    /// Passwords entered in this process, by save name. Memory only, never written.
    passwords: Mutex<HashMap<String, String>>,
    corpus: Option<Arc<Corpus>>,
    state: CorpusState,
    endpoints: Mutex<Endpoints>,
    media: MediaCache,
    /// Bounds concurrent avatar downloads: a roster opens dozens at once, and the source
    /// site is a volunteer-run server.
    fetches: tokio::sync::Semaphore,
    /// The last reachability probe of the chat endpoint; assumed up until one says otherwise.
    online: AtomicBool,
    /// What other processes have written since the last poll; see `external.rs`.
    watch: Mutex<external::Watch>,
}

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io {
        detail: e.to_string(),
    }
}

impl Engine {
    /// Open the home directory and, when there is one, the corpus. A corpus that is
    /// absent, refused or unreadable does not fail the open: the state says why and the
    /// corpus commands answer `NotFound`.
    pub fn open(home: Home, folio: Option<PathBuf>, ports: Ports) -> ApiResult<Self> {
        Self::open_with_password(home, folio, ports, None)
    }

    /// [`Engine::open`] with the password of the current save, if it is encrypted. Without
    /// the right one the engine still comes up, locked (see `AppStatus::locked`), so a
    /// front end can ask for it.
    pub fn open_with_password(
        home: Home,
        folio: Option<PathBuf>,
        ports: Ports,
        password: Option<String>,
    ) -> ApiResult<Self> {
        home.create().map_err(io)?;
        let params = Params::load(&home.params()).map_err(io)?;
        let mut passwords = HashMap::new();
        let mut held = None;
        let (save, missing, world, locked) = match home.start_save() {
            Start::Open(save) => {
                held = Some(SaveLock::shared(&home.save_path(&save))?);
                match World::open_with(home.save_path(&save), password.as_deref()) {
                    Ok(w) => {
                        passwords.extend(password.map(|p| (save.clone(), p)));
                        (save, None, w, false)
                    }
                    Err(world::Error::Locked) => {
                        (save, None, World::in_memory().map_err(world_err)?, true)
                    }
                    Err(e) => return Err(world_err(e)),
                }
            }
            Start::Pick { missing } => (
                String::new(),
                missing,
                World::in_memory().map_err(world_err)?,
                false,
            ),
        };
        let endpoints = match std::fs::read_to_string(home.endpoints()) {
            Ok(text) => Endpoints::from_toml(&text).map_err(io)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Endpoints::default(),
            Err(e) => return Err(io(e)),
        };

        let (corpus, state) = match folio {
            None => (None, CorpusState::Missing),
            Some(path) => match Corpus::open(&path, params.retrieve.clone(), &home.prompts()) {
                Ok(c) => {
                    let state = CorpusState::Loaded { name: c.name() };
                    (Some(Arc::new(c)), state)
                }
                Err(e) => (None, corpus::refusal(&e)),
            },
        };
        ports.agent.set_endpoints(&endpoints);
        let media = MediaCache::new(home.media(), params.media.cache_bytes());
        Ok(Self {
            home,
            params,
            ports,
            world: Mutex::new(world),
            save: Mutex::new(save),
            missing_save: Mutex::new(missing),
            locked: AtomicBool::new(locked),
            held: Mutex::new(held),
            elsewhere: AtomicBool::new(false),
            passwords: Mutex::new(passwords),
            corpus,
            state,
            endpoints: Mutex::new(endpoints),
            media,
            fetches: tokio::sync::Semaphore::new(AVATAR_FETCHES),
            online: AtomicBool::new(true),
            watch: Mutex::default(),
        })
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    pub fn corpus_handle(&self) -> Option<Arc<Corpus>> {
        self.corpus.clone()
    }

    /// Whether a chat role is configured: the app is in full form only with one.
    pub fn chat_configured(&self) -> bool {
        self.has_chat()
    }

    /// Build the retrieval index now (it takes longer than the first screen may).
    pub fn warm(&self) {
        if let Some(c) = &self.corpus {
            c.warm();
        }
    }

    /// Probe the chat endpoint's host and remember the answer for `app_status`: with no
    /// network the app falls back to library form.
    pub async fn probe_network(&self) -> bool {
        let e = self.current_endpoints();
        let base = e
            .roles
            .as_ref()
            .and_then(|r| e.profile(&r.chat.profile))
            .map(|p| p.base_url.clone());
        let up = match base {
            Some(b) => self.ports.net.reachable(&b).await,
            None => true,
        };
        self.online.store(up, Ordering::Relaxed);
        up
    }

    /// Quiet hours and the notification switch, for whoever raises a system notification.
    pub fn may_notify(&self) -> ApiResult<bool> {
        let now = self.now();
        let w = self.w();
        if !world::settings::notifications(&w).map_err(world_err)? {
            return Ok(false);
        }
        let quiet = world::settings::quiet_hours(&w, &self.params.world.seed).map_err(world_err)?;
        Ok(!quiet.is_some_and(|q| world::clock::in_quiet_hours(&q, now)))
    }

    // ---- shared helpers ----

    pub(crate) fn now(&self) -> Now {
        self.ports.clock.now()
    }

    pub(crate) fn w(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn corpus(&self) -> ApiResult<&Arc<Corpus>> {
        self.corpus.as_ref().ok_or(ApiError::NotFound)
    }

    pub(crate) fn current_endpoints(&self) -> Endpoints {
        self.endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn emit(&self, e: Event) {
        self.ports.sink.emit(e);
    }

    pub(crate) fn changed(&self, changes: Vec<WorldChange>) {
        if !changes.is_empty() {
            self.emit(Event::WorldChanged(WorldChanged { changes }));
        }
    }

    /// How the user is named in corpus text right now.
    pub(crate) fn namer(&self) -> ApiResult<Namer> {
        let name = world::settings::user_name(&self.w()).map_err(world_err)?;
        let (placeholder, you) = match &self.corpus {
            Some(c) => (
                c.manifest.user_placeholder.clone(),
                c.manifest.wording.get(display::USER_YOU).cloned(),
            ),
            None => (String::new(), None),
        };
        Ok(Namer::new(&placeholder, name.as_deref(), you.as_deref()))
    }

    pub(crate) fn person_ref(&self, id: &str) -> PersonRef {
        PersonRef {
            pronoun: self
                .corpus
                .as_ref()
                .and_then(|c| c.person(id))
                .and_then(|p| p.pronoun),
            id: PersonId(id.to_owned()),
            name: self
                .corpus
                .as_ref()
                .map(|c| c.display(id))
                .unwrap_or_else(|| id.to_owned()),
        }
    }

    pub(crate) fn has_chat(&self) -> bool {
        self.endpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .roles
            .is_some()
    }

    /// The person must be on the roster, have a persona and not be disabled.
    pub(crate) fn require_available(&self, person: &PersonId) -> ApiResult<()> {
        let corpus = self.corpus()?;
        if corpus.person(&person.0).is_none() {
            return Err(ApiError::NotFound);
        }
        if !corpus.has_persona(&person.0) {
            return Err(ApiError::NoPersona {
                person: person.clone(),
            });
        }
        world::bond::ensure_available(&self.w(), &convert::world_person(person)).map_err(world_err)
    }

    // ---- app ----

    fn status_impl(&self) -> AppStatus {
        let full = self.corpus.is_some()
            && self.has_chat()
            && self.ports.agent.wired()
            && self.online.load(Ordering::Relaxed);
        AppStatus {
            form: if full {
                AppForm::Full
            } else {
                AppForm::Library
            },
            corpus: self.state.clone(),
            needs_setup: world::settings::user_name(&self.w())
                .ok()
                .flatten()
                .is_none(),
            locked: self.locked.load(Ordering::Relaxed),
            changed_elsewhere: self.elsewhere.load(Ordering::Relaxed),
            save: Some(self.open_save()).filter(|s| !s.is_empty()),
            missing_save: self
                .missing_save
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            home_dir: self.home.root().display().to_string(),
            saves_dir: self.home.saves().display().to_string(),
        }
    }

    fn wording_impl(&self) -> Wording {
        let Some(c) = &self.corpus else {
            return Wording::default();
        };
        let entries = match self.home.interface_language().unwrap_or_default() {
            InterfaceLanguage::Corpus => c.manifest.wording.clone(),
            InterfaceLanguage::English => english_entries(&c.manifest.wording),
        };
        Wording {
            entries,
            scene_marker: SceneMarker {
                open: c.manifest.scene_marker.0.clone(),
                close: c.manifest.scene_marker.1.clone(),
            },
        }
    }

    fn clock_impl(&self) -> ApiResult<Clock_> {
        let year_offset = self.corpus.as_ref().map_or(0, |c| c.manifest.year_offset);
        let now = self.now();
        let d = world::clock::in_world_date(now, year_offset as i32);
        Ok(Clock_ {
            in_world: InWorldDate {
                year: d.year,
                month: d.month,
                day: d.day,
            },
            local_time: world::clock::local_time(now),
        })
    }

    // ---- quota and settings ----

    pub(crate) fn quota_view(&self) -> ApiResult<QuotaStatus> {
        let now = self.now();
        let w = self.w();
        let tier = world::settings::intensity(&w).map_err(world_err)?;
        let status =
            world::quota::status(&w, tier, now.ms, &self.params.world.quota).map_err(world_err)?;
        convert::quota_status(&w, &status, now, &self.params.world.quota).map_err(world_err)
    }

    fn set_tier_impl(&self, tier: Tier) -> ApiResult<()> {
        world::settings::set(
            &self.w(),
            world::settings::INTENSITY,
            &convert::world_tier(tier),
        )
        .map_err(world_err)?;
        let status = self.quota_view()?;
        self.emit(Event::QuotaChanged(QuotaChanged { status }));
        Ok(())
    }

    fn settings_impl(&self) -> ApiResult<Settings> {
        let w = self.w();
        Ok(Settings {
            user_name: world::settings::user_name(&w).map_err(world_err)?,
            birthday: world::settings::user_birthday(&w)
                .map_err(world_err)?
                .map(|d| MonthDay {
                    month: d.month,
                    day: d.day,
                }),
            bio: world::settings::user_bio(&w).map_err(world_err)?,
            quiet_hours: world::settings::quiet_hours(&w, &self.params.world.seed)
                .map_err(world_err)?
                .map(|q| QuietHours {
                    from: q.from,
                    to: q.to,
                }),
            notifications: world::settings::notifications(&w).map_err(world_err)?,
            events_per_day: world::settings::events_per_day(&w, &self.params.world.events)
                .map_err(world_err)?,
        })
    }

    async fn update_settings_impl(&self, s: Settings) -> ApiResult<()> {
        let invalid = |field: &str| ApiError::Invalid {
            field: field.into(),
        };
        let birthday = match s.birthday {
            Some(d) => Some(
                world::MonthDay::parse(&format!("{:02}-{:02}", d.month, d.day))
                    .ok_or_else(|| invalid("birthday"))?,
            ),
            None => None,
        };
        if let Some(q) = &s.quiet_hours
            && !(valid_time(&q.from) && valid_time(&q.to))
        {
            return Err(invalid("quiet_hours"));
        }
        if s.bio
            .as_ref()
            .is_some_and(|b| b.trim().chars().count() > world::settings::BIO_MAX)
        {
            return Err(invalid("bio"));
        }
        let events = s.events_per_day.min(self.params.world.events.max_per_day);
        let renamed = {
            let w = self.w();
            // Both sit in the cached prefix, so a change of either starts new segments.
            let renamed =
                world::settings::set_user_name(&w, s.user_name.as_deref()).map_err(world_err)?;
            let rebio = world::settings::set_user_bio(&w, s.bio.as_deref()).map_err(world_err)?;
            match birthday {
                Some(d) => world::settings::set(&w, world::settings::USER_BIRTHDAY, &d),
                None => world::settings::unset(&w, world::settings::USER_BIRTHDAY),
            }
            .map_err(world_err)?;
            let quiet = s.quiet_hours.map(|q| world::params::QuietHours {
                from: q.from,
                to: q.to,
            });
            world::settings::set(&w, world::settings::QUIET_HOURS, &quiet).map_err(world_err)?;
            world::settings::set(&w, world::settings::NOTIFICATIONS, &s.notifications)
                .map_err(world_err)?;
            world::settings::set(&w, world::settings::EVENTS_PER_DAY, &events)
                .map_err(world_err)?;
            renamed || rebio
        };
        if renamed {
            // Segments are never rewritten, so a new name reaches the model by rolling over.
            self.ports.agent.on_rename().await?;
        }
        Ok(())
    }
}

/// `HH:MM`, 24-hour.
fn valid_time(s: &str) -> bool {
    s.split_once(':').is_some_and(|(h, m)| {
        h.len() == 2
            && m.len() == 2
            && h.parse::<u8>().is_ok_and(|h| h < 24)
            && m.parse::<u8>().is_ok_and(|m| m < 60)
    })
}

/// What of the corpus's wording stays when the interface speaks English: its names, the
/// facet that tells genders apart, the corpus language's own name, and everything the
/// model reads (characters keep speaking the corpus's language).
fn english_entries(all: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    const KEEP: [&str; 4] = ["host.name", "org.name", "user.title", "lang.corpus"];
    const KEEP_PREFIX: [&str; 3] = ["harness.", "tool.", "gender."];
    all.iter()
        .filter(|(k, _)| KEEP.contains(&k.as_str()) || KEEP_PREFIX.iter().any(|p| k.starts_with(p)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

type Clock_ = crate::views::Clock;

#[async_trait]
impl Api for Engine {
    async fn app_status(&self) -> ApiResult<AppStatus> {
        Ok(self.status_impl())
    }
    async fn wording(&self) -> ApiResult<Wording> {
        Ok(self.wording_impl())
    }
    async fn interface_language(&self) -> ApiResult<InterfaceLanguage> {
        Ok(self.home.interface_language().unwrap_or_default())
    }
    async fn set_interface_language(&self, language: InterfaceLanguage) -> ApiResult<()> {
        self.home
            .set_interface_language(language)
            .map_err(|e| ApiError::Io {
                detail: e.to_string(),
            })
    }
    async fn presence(&self) -> ApiResult<Digest> {
        self.presence_impl().await
    }
    async fn coldstart(&self) -> ApiResult<Vec<Candidate>> {
        self.coldstart_impl()
    }
    async fn clock(&self) -> ApiResult<Clock_> {
        self.clock_impl()
    }
    async fn digest(&self) -> ApiResult<Digest> {
        self.digest_impl()
    }
    async fn roster(&self) -> ApiResult<Roster> {
        self.roster_impl()
    }
    async fn person_page(&self, person: PersonId) -> ApiResult<PersonPage> {
        self.person_page_impl(&person)
    }
    async fn find_people(&self, topic: String) -> ApiResult<Vec<Candidate>> {
        self.find_people_impl(&topic)
    }
    async fn set_mode(&self, target: Target, mode: Mode) -> ApiResult<()> {
        self.set_mode_impl(target, mode)
    }
    async fn channels(&self) -> ApiResult<Vec<ChannelSummary>> {
        self.channels_impl()
    }
    async fn history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u32,
    ) -> ApiResult<Vec<Message>> {
        self.history_impl(&channel, before.as_ref(), limit)
    }
    async fn open_direct(&self, person: PersonId) -> ApiResult<ChannelId> {
        self.open_direct_impl(&person)
    }
    async fn host_channel(&self) -> ApiResult<ChannelId> {
        self.host_channel_impl()
    }
    async fn send_message(
        &self,
        channel: ChannelId,
        text: String,
        image: Option<ImagePart>,
    ) -> ApiResult<MessageId> {
        self.send_message_impl(channel, text, image).await
    }
    async fn rewind(&self, channel: ChannelId, message: MessageId) -> ApiResult<String> {
        self.rewind_impl(channel, message).await
    }

    async fn wrapup(&self, channel: ChannelId) -> ApiResult<()> {
        self.wrapup_impl(channel).await
    }
    async fn relay(&self, excerpt: String, from: MessageId, to: PersonId) -> ApiResult<ChannelId> {
        self.relay_impl(excerpt, from, to).await
    }
    async fn mark_read(&self, channel: ChannelId, up_to: MessageId) -> ApiResult<()> {
        self.mark_read_impl(&channel, &up_to)
    }
    async fn create_group(
        &self,
        members: Vec<PersonId>,
        topic: Option<String>,
    ) -> ApiResult<ChannelId> {
        self.create_group_impl(members, topic)
    }
    async fn open_offer(&self, message: MessageId, action: u32) -> ApiResult<ChannelId> {
        self.open_offer_impl(&message, action).await
    }
    async fn open_found(
        &self,
        message: MessageId,
        action: u32,
        person: PersonId,
    ) -> ApiResult<ChannelId> {
        self.open_found_impl(&message, action, &person).await
    }
    async fn open_invite(
        &self,
        message: MessageId,
        action: u32,
        person: PersonId,
    ) -> ApiResult<ChannelId> {
        self.open_invite_impl(&message, action, &person).await
    }
    async fn delete_group(&self, channel: ChannelId) -> ApiResult<()> {
        self.delete_group_impl(&channel)
    }
    async fn add_members(&self, channel: ChannelId, persons: Vec<PersonId>) -> ApiResult<()> {
        self.add_members_impl(&channel, persons).await
    }
    async fn remove_member(&self, channel: ChannelId, person: PersonId) -> ApiResult<()> {
        self.remove_member_impl(&channel, &person).await
    }
    async fn rename_channel(&self, channel: ChannelId, name: Option<String>) -> ApiResult<()> {
        self.rename_channel_impl(&channel, name)
    }
    async fn set_channel_avatar(
        &self,
        channel: ChannelId,
        image: Option<ImagePart>,
    ) -> ApiResult<Option<MediaFile>> {
        self.set_channel_avatar_impl(&channel, image)
    }
    async fn archive_channel(&self, channel: ChannelId, archived: bool) -> ApiResult<()> {
        self.archive_channel_impl(&channel, archived)
    }
    async fn ask(
        &self,
        person: PersonId,
        question: String,
        turns: Option<u32>,
        parent: Option<TaskId>,
    ) -> ApiResult<TaskId> {
        self.ask_impl(person, question, turns, parent).await
    }
    async fn tasks(&self) -> ApiResult<Vec<TaskSummary>> {
        self.tasks_impl()
    }
    async fn case_file(&self, task: TaskId) -> ApiResult<CaseFile> {
        self.case_file_impl(&task)
    }
    async fn open_case(&self, question: String) -> ApiResult<TaskId> {
        self.open_case_impl(question).await
    }
    async fn pin_task(&self, task: TaskId) -> ApiResult<()> {
        self.pin_impl(&task, true).await
    }
    async fn unpin_task(&self, task: TaskId) -> ApiResult<()> {
        self.pin_impl(&task, false).await
    }
    async fn write_conclusion(&self, task: TaskId, text: String) -> ApiResult<FactId> {
        self.write_conclusion_impl(&task, &text).await
    }
    async fn facts(&self, filter: FactFilter) -> ApiResult<FactList> {
        self.facts_impl(filter)
    }
    async fn edit_fact(&self, fact: FactId, text: String) -> ApiResult<FactId> {
        self.edit_fact_impl(&fact, &text)
    }
    async fn delete_fact(&self, fact: FactId) -> ApiResult<()> {
        self.retract_impl(&fact, true)
    }
    async fn restore_fact(&self, fact: FactId) -> ApiResult<()> {
        self.retract_impl(&fact, false)
    }
    async fn search_corpus(
        &self,
        query: String,
        persons: Vec<PersonId>,
        templates: Vec<String>,
    ) -> ApiResult<SearchResults> {
        self.search_impl(&query, &persons, &templates)
    }
    async fn read_unit(&self, unit: String, around: u32) -> ApiResult<UnitView> {
        self.read_unit_impl(&unit, around)
    }
    async fn corpus_templates(&self) -> ApiResult<Vec<String>> {
        self.templates_impl()
    }
    async fn resolve_citation(&self, citation: Citation) -> ApiResult<CitationView> {
        self.resolve_impl(&citation)
    }
    async fn avatar(&self, person: PersonId) -> ApiResult<Option<MediaFile>> {
        self.avatar_impl(&person).await
    }
    async fn user_avatar(&self) -> ApiResult<Option<MediaFile>> {
        self.user_avatar_impl().await
    }
    async fn set_user_avatar(&self, image: Option<ImagePart>) -> ApiResult<Option<MediaFile>> {
        self.set_user_avatar_impl(image).await
    }
    async fn storage_usage(&self) -> ApiResult<StorageUsage> {
        self.storage_usage_impl()
    }
    async fn clear_storage(&self, categories: Vec<StorageKind>) -> ApiResult<StorageUsage> {
        self.clear_storage_impl(&categories)
    }
    async fn tts_play(&self, message: MessageId) -> ApiResult<MediaFile> {
        self.tts_play_impl(&message).await
    }
    async fn tts_voices(&self) -> ApiResult<Vec<String>> {
        self.tts_voices_impl().await
    }
    async fn set_voice(&self, person: PersonId, voice: Option<String>) -> ApiResult<()> {
        self.set_voice_impl(&person, voice)
    }
    async fn quota(&self) -> ApiResult<QuotaStatus> {
        self.quota_view()
    }
    async fn set_tier(&self, tier: Tier) -> ApiResult<()> {
        self.set_tier_impl(tier)
    }
    async fn ship_events(&self) -> ApiResult<Vec<ShipEvent>> {
        self.ship_events_impl()
    }
    async fn settings(&self) -> ApiResult<Settings> {
        self.settings_impl()
    }
    async fn update_settings(&self, settings: Settings) -> ApiResult<()> {
        self.update_settings_impl(settings).await
    }
    async fn saves(&self) -> ApiResult<Vec<SaveInfo>> {
        Ok(self.saves_impl())
    }
    async fn new_save(&self, name: String) -> ApiResult<()> {
        crate::saves::create(&self.home, &name)
    }
    async fn log_out(&self) -> ApiResult<()> {
        self.log_out_impl()
    }
    async fn choose_corpus(&self) -> ApiResult<Option<String>> {
        let ask = self
            .ports
            .choose_file
            .clone()
            .ok_or(ApiError::NotImplemented)?;
        // The dialog blocks until the user answers.
        tokio::task::spawn_blocking(move || ask()).await.map_err(io)
    }
    async fn open_external(&self, url: String) -> ApiResult<()> {
        self.open_external_impl(&url)
    }
    async fn set_corpus(&self, path: String) -> ApiResult<()> {
        self.set_corpus_impl(&path)
    }
    async fn switch_save(&self, name: String) -> ApiResult<()> {
        self.switch_save_impl(&name)
    }
    async fn copy_save(&self, from: String, to: String) -> ApiResult<()> {
        crate::saves::copy(&self.home, &from, &to, self.password_of(&from).as_deref())
    }
    async fn rename_save(&self, from: String, to: String) -> ApiResult<()> {
        self.rename_save_impl(&from, &to)
    }
    async fn delete_save(&self, name: String) -> ApiResult<()> {
        self.delete_save_impl(&name)
    }
    async fn export_save(&self, name: String, path: String) -> ApiResult<()> {
        self.export_save_impl(&name, &path)
    }
    async fn import_save(
        &self,
        path: String,
        name: Option<String>,
        password: Option<String>,
    ) -> ApiResult<String> {
        self.import_save_impl(&path, name.as_deref(), password)
    }
    async fn unlock(&self, name: String, password: String) -> ApiResult<()> {
        self.unlock_impl(&name, password)
    }
    async fn set_password(&self, current: Option<String>, next: Option<String>) -> ApiResult<()> {
        self.set_password_impl(current, next)
    }
    async fn endpoints(&self) -> ApiResult<EndpointsView> {
        Ok(self.endpoints_view())
    }
    async fn setup_endpoint(&self, setup: EndpointSetup) -> ApiResult<()> {
        self.setup_endpoint_impl(setup)
    }
    async fn add_profile(&self, profile: Profile) -> ApiResult<()> {
        self.add_profile_impl(profile)
    }
    async fn update_profile(&self, name: String, profile: Profile) -> ApiResult<()> {
        self.update_profile_impl(&name, profile)
    }
    async fn remove_profile(&self, name: String) -> ApiResult<()> {
        self.remove_profile_impl(&name)
    }
    async fn fetch_models(&self, profile: String) -> ApiResult<Vec<String>> {
        self.fetch_models_impl(&profile).await
    }
    async fn set_key(&self, profile: String, key: String) -> ApiResult<()> {
        self.set_key_impl(&profile, &key)
    }
    async fn set_role(&self, kind: RoleKind, role: Option<Role>) -> ApiResult<()> {
        self.set_role_impl(kind, role)
    }
}

#[cfg(test)]
mod tests;
