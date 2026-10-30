//! Read-side shapes the screens render, and the few request shapes commands take.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::endpoints::{Profile, Roles};
use crate::types::{
    Author, ChannelId, Citation, Confidence, FactId, MediaFile, MessageId, Mode, PersonId,
    PersonRef, Target, Tier, Timestamp,
};

// ---- app ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AppStatus {
    pub form: AppForm,
    pub corpus: CorpusState,
    /// The first-run questions (name, birthday, bio, all optional) have not been seen on
    /// this save: show them.
    pub needs_setup: bool,
    /// The open save is encrypted and not unlocked: only `unlock` (or switching to another
    /// save) works until then.
    pub locked: bool,
    /// The open save's file was replaced, renamed, deleted or encrypted by another window
    /// or process, so this one stopped using it: only `switch_save` to the same name
    /// (reopen) or another save works until then.
    pub changed_elsewhere: bool,
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

/// Who made the app and which build this is, from the build itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AppInfo {
    pub version: String,
    pub author: String,
    /// The author's own page.
    pub author_url: Option<String>,
    pub repository: String,
    pub license: String,
    /// Who built the open corpus, when it says so.
    pub corpus_credits: Option<CorpusCredits>,
}

/// A corpus's credit line, from its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CorpusCredits {
    pub by: Option<String>,
    /// The person's own page: an https address, or nothing.
    pub url: Option<String>,
    /// The project's page when it is not this app's: an https address, or nothing.
    pub project: Option<String>,
    pub licence: Option<String>,
}

impl AppInfo {
    pub fn current() -> Self {
        // Cargo's `authors` entry reads `name (page)`.
        let raw = env!("CARGO_PKG_AUTHORS");
        let (author, author_url) = match raw.split_once(" (") {
            Some((name, rest)) => (name, rest.strip_suffix(')')),
            None => (raw, None),
        };
        Self {
            version: crate::update::VERSION.into(),
            author: author.into(),
            author_url: author_url.map(Into::into),
            repository: crate::update::REPOSITORY.into(),
            license: env!("CARGO_PKG_LICENSE").into(),
            corpus_credits: None,
        }
    }
}

/// What the last check for a newer release found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct UpdateStatus {
    pub current: String,
    /// The newest release's version; `None` before the first successful check.
    pub latest: Option<String>,
    /// Its release page.
    pub url: Option<String>,
    /// `latest` is higher than `current`.
    pub newer: bool,
    pub checked_at: Option<Timestamp>,
    /// Whether the app checks by itself, at most once a day.
    pub auto: bool,
    /// Whether the user has answered the question about checking by themselves.
    pub asked: bool,
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

/// Which words the interface speaks. The person's choice, kept in the home, so the app and
/// the terminal agree. Model-facing text is not affected: characters keep speaking the
/// corpus's language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceLanguage {
    /// The corpus's own wording (its language).
    #[default]
    Corpus,
    /// The built-in English defaults; only the corpus's names and its model-facing text stay.
    English,
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
    /// not talked to, asked, forwarded to or put in a group.
    pub has_persona: bool,
    /// Whether the user has talked with them before, whatever their mode is now.
    pub talked: bool,
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
    /// The user's favorites.
    Favorites,
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
    /// Messages from the host or a person after the read cursor, which is saved with the
    /// world: the badge is this number.
    pub unread: u32,
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
    /// Who wrote the original, when this is a forwarded copy in a person's or the host's
    /// direct channel. Copies kept in the favorites carry no mark: they show their
    /// original author as the author.
    #[serde(default)]
    pub forwarded_from: Option<Author>,
    /// The earlier message in this channel this one answers.
    #[serde(default)]
    pub reply: Option<Quote>,
    /// In the favorites, when the saved message was first written; `at` is when it was
    /// saved.
    #[serde(default)]
    pub original_at: Option<Timestamp>,
    /// A line from the harness that gives context (why someone was found); shown as a
    /// quiet note between the bubbles, not as something the user said.
    #[serde(default)]
    pub service: bool,
    pub at: Timestamp,
}

/// What a reply quotes: the original's author and the start of its text. The original
/// may lie outside the loaded history, so the quote carries what it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Quote {
    pub id: MessageId,
    pub author: Author,
    /// The original's text, cut to `QUOTE_CHARS`; scene lines are still in it.
    pub text: String,
    /// The original carried a picture.
    pub has_picture: bool,
}

/// Most characters of the original a [`Quote`] carries.
pub const QUOTE_CHARS: usize = 300;

/// Where a message is forwarded to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForwardTo {
    /// The person's direct channel; they answer it.
    Person { id: PersonId },
    /// The host's channel; it answers.
    Host,
    /// The user's favorites: kept, nobody answers.
    Favorites,
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
    Wrapup,
    Digest,
    FindPeople {
        topic: String,
        /// Who was found, so a front end can offer to open their channels.
        #[serde(default)]
        persons: Vec<PersonRef>,
    },
    CreateGroup {
        members: Vec<PersonRef>,
        /// The new group, so a front end can offer to open it. Absent on older rows.
        #[serde(default)]
        channel: Option<ChannelId>,
        /// Why the group exists, in a few words. It becomes the group's topic.
        #[serde(default)]
        purpose: Option<String>,
    },
    /// A person's suggestion, made inside a group, to bring colleagues into that group.
    /// Nothing changes until the user takes one of them.
    InviteToGroup {
        channel: ChannelId,
        persons: Vec<PersonRef>,
    },
    SetMode {
        target: Target,
        mode: Mode,
    },
    /// The host set a reminder: the person brings it up when it falls due.
    Promise {
        person: PersonRef,
        text: String,
        /// The due time as the host wrote it.
        due: String,
    },
    /// The host read the last lines of a conversation.
    Recent,
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

// ---- facts ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Fact,
    Commitment,
    Hurt,
}

/// How a commitment ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FactOutcome {
    /// It was brought up when due.
    Kept,
    /// It expired before anyone could bring it up.
    Lapsed,
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
    /// How a commitment ended; none while it is pending.
    pub outcome: Option<FactOutcome>,
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

/// A unit read in place: the unit and the units next to it in the same source, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UnitView {
    pub unit: Passage,
    pub before: Vec<Passage>,
    pub after: Vec<Passage>,
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
    /// Most openings a day this tier allows.
    pub visits_per_day: u32,
    /// Most daily events a day this tier allows.
    pub events_max: u32,
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
    /// Daily events the host may post per day: 0 turns them off. Without a choice of the
    /// user's it is the tier's default; a larger number is cut to the tier's most.
    pub events_per_day: u32,
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
            events_per_day: 1,
        }
    }
}

/// A happening of the day the host told of: the host's channel history holds the post
/// itself; this is the log of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct DailyEvent {
    /// The local date it belongs to, `YYYY-MM-DD`.
    pub day: String,
    pub at: Timestamp,
    pub title: String,
    pub text: String,
    pub persons: Vec<PersonRef>,
    /// The host's post, where its offers are.
    pub message: Option<MessageId>,
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
