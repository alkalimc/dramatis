//! Read-side shapes the screens render, and the few request shapes commands take.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::endpoints::{Profile, Roles};
use crate::types::{
    Author, ChannelId, Citation, Confidence, FactId, MediaFile, MessageId, Mode, PersonId,
    PersonRef, Target, TaskId, Tier, Timestamp,
};

// ---- app ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AppStatus {
    pub form: AppForm,
    pub corpus: CorpusState,
    /// No user name yet: show the first-run form.
    pub needs_setup: bool,
    /// The open save is encrypted and not unlocked: only `unlock` (or switching to another
    /// save) works until then.
    pub locked: bool,
    /// The save this process has open. `None`: none (first run, the last one is gone, or
    /// the user logged out), so the save picker shows.
    pub save: Option<String>,
    /// The save recorded as the last one used when it no longer exists (renamed or deleted
    /// elsewhere), so the picker can say so.
    pub missing_save: Option<String>,
    /// The home directory, where endpoints, parameters and the corpus link live.
    pub home_dir: String,
    /// The folder the saves are files in.
    pub saves_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AppForm {
    /// No chat endpoint, or no network: roster and search only.
    Library,
    /// Host channel and the waiting panel.
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CorpusState {
    Missing,
    Loaded {
        name: String,
    },
    /// Refused: unknown format version. The user needs a newer app.
    UnsupportedVersion {
        format_version: u32,
    },
    /// Refused: the corpus requires a reader feature this build lacks.
    MissingRequirement {
        requirement: String,
    },
}

/// Runtime phrasing from the corpus manifest, keyed like the UI's i18n keys; a missing
/// key falls back to the UI's neutral default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Wording {
    pub entries: BTreeMap<String, String>,
    pub scene_marker: SceneMarker,
}

/// A line wrapped in these is scene description, rendered apart from speech.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SceneMarker {
    pub open: String,
    pub close: String,
}

impl Default for SceneMarker {
    fn default() -> Self {
        Self {
            open: "*".into(),
            close: "*".into(),
        }
    }
}

// ---- clock ----

/// The header's clock. Derived from the local wall clock on every call, never stored.
///
/// The in-world year is the local year minus an offset that belongs to the corpus, not to
/// this code: it is read from the folio manifest key `clock.year_offset`, written by the
/// pack. Month and day are the local ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Clock {
    pub in_world: InWorldDate,
    /// Local time, `HH:MM`.
    pub local_time: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InWorldDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

// ---- presence ----

/// The waiting panel: pure data, no model call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct Digest {
    pub items: Vec<Waiting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Waiting {
    pub person: PersonRef,
    /// Where a click goes.
    pub channel: ChannelId,
    pub activity: Activity,
}

/// Something a person did that the user has not seen, or that is due.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Activity {
    Messaged {
        at: Timestamp,
    },
    Replied {
        task: TaskId,
        question: String,
        at: Timestamp,
    },
    CommitmentDue {
        fact: FactId,
        text: String,
        due: Timestamp,
    },
    CameBy {
        at: Timestamp,
    },
    Birthday,
}

/// A person offered for something, with one line from the material that matched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Candidate {
    pub person: PersonRef,
    pub mode: Mode,
    pub reason: String,
    pub citation: Option<Citation>,
}

// ---- roster ----

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct Roster {
    pub total: u32,
    pub enabled: u32,
    /// In display order; empty groups are omitted.
    pub groups: Vec<RosterGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct RosterGroup {
    pub band: RosterBand,
    pub rows: Vec<RosterRow>,
}

/// Grouping by trust and contact, never a number on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RosterBand {
    Close,
    Acquainted,
    NeverTalked,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct RosterRow {
    pub person: PersonRef,
    pub mode: Mode,
    /// Whether the corpus has a persona for them. Without one they can be read about but
    /// not talked to, asked, relayed to or put in a group.
    pub has_persona: bool,
    pub recent: Option<Activity>,
    /// People they often appear with in the corpus.
    pub companions: Vec<PersonRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct PersonPage {
    pub person: PersonRef,
    /// One descriptive line from the corpus profile.
    pub summary: Option<String>,
    pub trust: u8,
    pub mode: Mode,
    /// See [`RosterRow::has_persona`].
    pub has_persona: bool,
    pub last_seen: Option<Timestamp>,
    pub tasks_done: u32,
    pub companions: Vec<PersonRef>,
    /// Read-aloud voice; `None` = endpoint default.
    pub voice: Option<String>,
    pub channel: Option<ChannelId>,
}

// ---- channels ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Direct,
    Group,
    /// The host's own channel.
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    User,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ChannelSummary {
    pub id: ChannelId,
    pub kind: ChannelKind,
    pub origin: Origin,
    pub participants: Vec<PersonRef>,
    pub topic: Option<String>,
    /// The title the user gave it; `None`: derive one from the members.
    pub name: Option<String>,
    /// The picture the user gave it; `None`: compose one from the members' avatars.
    pub avatar: Option<MediaFile>,
    /// Out of the main list. Still fully working; a person's new message clears it.
    pub archived: bool,
    pub mode: Mode,
    pub last_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Message {
    pub id: MessageId,
    pub channel: ChannelId,
    pub author: Author,
    /// As written, scene lines included; the UI splits on the scene marker.
    pub text: String,
    /// Visible action lines, in call order.
    pub actions: Vec<ToolAction>,
    /// The picture the user attached, as a `data:` URL.
    pub attachment: Option<MediaFile>,
    /// A picture was attached but is gone; show a placeholder.
    pub picture_lost: bool,
    pub cites: Vec<Citation>,
    /// Low: render the thin-archive marker.
    pub confidence: Option<Confidence>,
    /// A retelling of something said elsewhere.
    pub relayed: bool,
    /// The request this message answers.
    pub task: Option<TaskId>,
    pub note: Option<MediaFile>,
    pub at: Timestamp,
}

/// An image the user attaches; sent to the model as a multimodal part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ImagePart {
    /// `image/png`, `image/jpeg`, ...
    pub mime: String,
    pub data_base64: String,
}

/// One tool call as the UI shows it: typed, so each kind gets its own i18n line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolAction {
    Search {
        query: String,
    },
    GetChunk {
        chunk_id: String,
    },
    RequestJoin {
        person: Option<PersonRef>,
        topic: Option<String>,
    },
    Remember {
        text: String,
    },
    Report,
    Wrapup,
    Digest,
    FindPeople {
        topic: String,
        /// Who was found, so a front end can offer to open their channels.
        #[serde(default)]
        persons: Vec<PersonRef>,
    },
    Ask {
        person: PersonRef,
        question: String,
    },
    CreateGroup {
        members: Vec<PersonRef>,
        /// The new group, so a front end can offer to open it. Absent on older rows.
        #[serde(default)]
        channel: Option<ChannelId>,
    },
    SetMode {
        target: Target,
        mode: Mode,
    },
    Forget {
        fact: FactId,
    },
    ListChannels {
        /// The conversations listed, so a front end can offer to open each.
        #[serde(default)]
        channels: Vec<ChannelRef>,
    },
}

/// A conversation named in a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ChannelRef {
    pub id: ChannelId,
    pub kind: ChannelKind,
    pub members: Vec<PersonRef>,
}

// ---- tasks and case files ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct TaskSummary {
    pub id: TaskId,
    pub question: String,
    /// A pinned question may have nobody on it yet.
    pub assignee: Option<PersonRef>,
    pub parent: Option<TaskId>,
    pub status: TaskStatus,
    pub pinned: bool,
    pub channel: Option<ChannelId>,
    pub created_at: Timestamp,
}

/// A pinned question and everything that hangs off it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CaseFile {
    pub task: TaskSummary,
    /// Each request made under it, with its reply once there is one.
    pub trail: Vec<TrailEntry>,
    pub conclusion: Option<FactView>,
    /// Messages elsewhere that responded to the conclusion.
    pub reactions: Vec<MessageRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct TrailEntry {
    pub task: TaskSummary,
    pub reply: Option<Message>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct MessageRef {
    pub channel: ChannelId,
    pub message: MessageId,
    pub author: Author,
    pub excerpt: String,
}

// ---- facts ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Fact,
    Commitment,
    Conclusion,
    Hurt,
}

/// Who can recall a fact. Shown on every row: the only place visibility surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Audience {
    World,
    Participants {
        channel: ChannelId,
        persons: Vec<PersonRef>,
    },
    /// One person's own observation (`self` in the world model).
    Own {
        person: PersonRef,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum About {
    Task { id: TaskId },
    Fact { id: FactId },
    Person { id: PersonId },
    User,
    Citation { citation: Citation },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct FactView {
    pub id: FactId,
    pub author: Author,
    pub audience: Audience,
    pub kind: FactKind,
    pub about: Option<About>,
    pub due: Option<Timestamp>,
    pub delivered: bool,
    pub text: String,
    pub retracted: bool,
    pub created_at: Timestamp,
}

/// The world drawer's filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FactScope {
    #[default]
    All,
    World,
    Own,
    Commitment,
    Hurt,
    Deleted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FactFilter {
    pub scope: FactScope,
    /// Only facts about, or recalled by, this person.
    pub person: Option<PersonId>,
    pub task: Option<TaskId>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct FactList {
    pub total: u32,
    pub deleted: u32,
    pub items: Vec<FactView>,
}

// ---- corpus ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Passage {
    pub citation: Citation,
    pub template: String,
    pub header: String,
    pub text: String,
    pub persons: Vec<PersonRef>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct SearchResults {
    pub hits: Vec<Passage>,
    pub confidence: Confidence,
    /// The name the query was normalised to, so the UI can say what it searched for.
    pub understood_as: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CitationStatus {
    /// The chunk id still exists.
    Current,
    /// The page changed; this is the best-overlapping passage of the new version.
    Updated,
    /// Nothing in the current corpus matches; only the stored links remain.
    Gone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CitationView {
    pub status: CitationStatus,
    pub passage: Option<Passage>,
    pub source_url: Option<String>,
    pub revision_url: Option<String>,
}

// ---- quota ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum QuotaBand {
    /// Below the quiet threshold: everything as usual.
    Open,
    /// Unprompted speech and interjections are off; user-initiated calls proceed.
    Quiet,
    /// No new calls until `release_at`.
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct QuotaWindow {
    /// Fraction of the limit used, 0 and up.
    pub used: f64,
    /// When enough usage rolls out of the window to leave the current band.
    pub release_at: Option<Timestamp>,
}

/// The call shapes spending is broken down by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CallShape {
    Direct,
    Group,
    Opening,
    Ask,
    Host,
    Wrapup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ShapeSpend {
    pub shape: CallShape,
    /// Calls in the window.
    pub calls: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct QuotaStatus {
    pub tier: Tier,
    pub band: QuotaBand,
    /// The rolling hour; `None` on the tier without a window.
    pub window: Option<QuotaWindow>,
    pub spent: Vec<ShapeSpend>,
}

// ---- settings ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MonthDay {
    pub month: u8,
    pub day: u8,
}

/// Local wall-clock times, `HH:MM`; `from` after `to` spans midnight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct QuietHours {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct Settings {
    pub user_name: Option<String>,
    pub birthday: Option<MonthDay>,
    /// A few lines the user writes about themselves; characters are told them. At most
    /// `world::settings::BIO_MAX` characters.
    pub bio: Option<String>,
    /// `None`: no quiet hours. The engine fills the register default on first run.
    pub quiet_hours: Option<QuietHours>,
    pub notifications: bool,
}

impl Default for Settings {
    /// Notifications are on until the user turns them off.
    fn default() -> Self {
        Self {
            user_name: None,
            birthday: None,
            bio: None,
            quiet_hours: None,
            notifications: true,
        }
    }
}

// ---- saves ----

/// One save: a whole world (channels, memories, bonds, requests, usage). Endpoints, keys,
/// the corpus and the media cache are shared by every save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SaveInfo {
    pub name: String,
    pub size_kb: u32,
    /// When the file last changed; `None` when the file system does not say.
    pub played_at: Option<Timestamp>,
    /// The save this process has open.
    pub current: bool,
    /// Protected by a password.
    pub encrypted: bool,
}

// ---- storage ----

/// What takes room on disk, by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// Every save file, pictures inside them included. Not clearable.
    Saves,
    /// Fetched portraits: fetched again when needed.
    Avatars,
    /// Read-aloud audio: synthesised again when needed.
    Audio,
    /// Pictures the user chose (their group pictures): cannot be fetched again. Not clearable.
    Uploads,
    /// The corpus file. Not clearable.
    Corpus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StorageCategory {
    pub kind: StorageKind,
    /// Kilobytes (1024 bytes), rounded up.
    pub kb: u32,
    pub files: u32,
    /// A cache that `clear_storage` may empty.
    pub clearable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SaveSize {
    pub name: String,
    pub kb: u32,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StorageUsage {
    /// Everything below, once: the pictures inside the current save are part of `Saves`.
    pub total_kb: u32,
    pub categories: Vec<StorageCategory>,
    pub saves: Vec<SaveSize>,
    /// Pictures attached to messages in the current save, and the room they take.
    pub pictures: u32,
    pub picture_kb: u32,
    /// The size the avatar and audio caches are trimmed to (`media.cache_gb`).
    pub cache_limit_kb: u32,
}

// ---- endpoints ----

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct EndpointsView {
    pub profiles: Vec<ProfileView>,
    pub roles: Option<Roles>,
    /// Starting points for "add".
    pub presets: Vec<Profile>,
}

/// The first-run form: one endpoint, the model the people talk with, optionally another for
/// the host, and the key. Written together or not at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct EndpointSetup {
    /// The profile's name; one that exists is updated.
    pub name: String,
    pub base_url: String,
    pub wire_api: crate::endpoints::WireApi,
    /// The model of the chat role.
    pub model: String,
    /// A different model for the host; `None`: the host uses the chat role.
    pub host_model: Option<String>,
    /// Goes to the keychain and nowhere else. `None`: leave the profile's key as it is.
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ProfileView {
    pub profile: Profile,
    /// A key is in the keychain. The key itself never crosses this seam.
    pub has_key: bool,
}

/// The roles the endpoints drawer can set. The persona role is maintainer-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RoleKind {
    Chat,
    /// The in-world assistant; absent, it uses the chat role.
    Host,
    Tts,
}
