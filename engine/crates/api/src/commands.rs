//! Every command the UI can issue, declared once.
//!
//! [`commands!`] expands one list into the [`Api`] trait (what the app implements), the
//! [`Stub`] implementation (what builds before the engine is wired), [`COMMANDS`] (the
//! frozen name list) and, with the `tauri` feature, one Tauri command per entry that
//! forwards to the trait object held in app state. Adding a command is one entry here;
//! the TypeScript side follows by regeneration.

use async_trait::async_trait;

use crate::endpoints::{Profile, Role};
use crate::error::{ApiError, ApiResult};
use crate::types::{
    ChannelId, Citation, FactId, MediaFile, MessageId, Mode, PersonId, Target, TaskId, Tier,
};
use crate::views::*;

macro_rules! stub_body {
    () => {
        Err(ApiError::NotImplemented)
    };
    ($e:expr) => {
        $e
    };
}

macro_rules! commands {
    ($(
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty $(= $stub:expr)?;
    )*) => {
        /// The engine side of the seam. The app implements this over `agent`, `world` and
        /// `index`; every command's policy ("which", "whether", "enough") lives behind it.
        #[async_trait]
        pub trait Api: Send + Sync + 'static {
            $(
                $(#[$doc])*
                async fn $name(&self, $($arg: $ty),*) -> ApiResult<$ret>;
            )*
        }

        /// Answers every command with [`ApiError::NotImplemented`], except the few the
        /// UI needs to come up at all (status, wording, empty lists).
        #[derive(Debug, Default, Clone, Copy)]
        pub struct Stub;

        #[async_trait]
        #[allow(unused_variables)]
        impl Api for Stub {
            $(
                async fn $name(&self, $($arg: $ty),*) -> ApiResult<$ret> {
                    stub_body!($($stub)?)
                }
            )*
        }

        /// Command names as the UI invokes them.
        pub const COMMANDS: &[&str] = &[$(stringify!($name)),*];

        #[cfg(feature = "tauri")]
        pub(crate) mod handlers {
            use super::*;
            use crate::Handle;

            $(
                $(#[$doc])*
                #[tauri::command]
                #[specta::specta]
                async fn $name(api: tauri::State<'_, Handle>, $($arg: $ty),*) -> ApiResult<$ret> {
                    api.current().$name($($arg),*).await
                }
            )*

            pub(crate) fn collect() -> tauri_specta::Commands<tauri::Wry> {
                tauri_specta::collect_commands![$($name),*]
            }
        }
    };
}

commands! {
    // ---- app ----

    /// Which form the app is in and whether the corpus loaded.
    fn app_status() -> AppStatus = Ok(AppStatus {
        form: AppForm::Library,
        corpus: CorpusState::Missing,
        needs_setup: true,
        locked: false,
        changed_elsewhere: false,
        save: None,
        missing_save: None,
        home_dir: String::new(),
        saves_dir: String::new(),
    });
    /// The build's version, author and repository. Needs no window permission, so it
    /// reads the same in the app, the terminal and tests.
    fn app_info() -> AppInfo = Ok(AppInfo::current());
    /// The result of the last update check, from disk; no network.
    fn update_status() -> UpdateStatus;
    /// Ask GitHub for the latest release. Without `force` it asks only when automatic
    /// checks are on and the last result is over a day old, and otherwise answers
    /// `update_status`. A failed request is an `Endpoint` error and leaves the stored
    /// result alone. Nothing is downloaded or installed.
    fn check_update(force: bool) -> UpdateStatus;
    /// Turn automatic checks on or off for this home.
    fn set_auto_update_check(auto: bool) -> ();
    /// The corpus's runtime phrasing.
    fn wording() -> Wording = Ok(Wording::default());
    /// The interface language this home uses; `wording` already follows it.
    fn interface_language() -> InterfaceLanguage = Ok(InterfaceLanguage::default());
    /// Change the interface language for this home. The caller then reads `wording` again;
    /// nothing restarts.
    fn set_interface_language(language: InterfaceLanguage) -> ();
    /// The user opened the app or came back from idle. Returns the waiting panel at once
    /// (no model call). What the engine then says arrives as `MessageAdded` (streamed
    /// through `MessageDelta` first) in the host's channel: on the very first open with a
    /// chat endpoint that is the host's introduction, shown with the [`coldstart`] card;
    /// afterwards at most one line about a non-empty digest. Then at most one person's
    /// opening, in their own channel. Nothing is said while quota is quiet or exhausted.
    ///
    /// [`coldstart`]: Api::coldstart
    fn presence() -> Digest = Ok(Digest::default());
    /// The first-open recommendation card: people picked when the corpus was built,
    /// each with a reason from their material. Everyone on it is still frozen.
    fn coldstart() -> Vec<Candidate> = Ok(Vec::new());
    /// The in-world date and the local time, for the header.
    fn clock() -> Clock;

    // ---- presence and people ----

    /// The waiting panel. Pure data.
    fn digest() -> Digest = Ok(Digest::default());
    fn roster() -> Roster = Ok(Roster::default());
    fn person_page(person: PersonId) -> PersonPage;
    /// People who know about a topic, each with a reason. Disabled people never appear.
    fn find_people(topic: String) -> Vec<Candidate> = Ok(Vec::new());
    fn set_mode(target: Target, mode: Mode) -> ();

    // ---- channels ----

    fn channels() -> Vec<ChannelSummary> = Ok(Vec::new());
    /// Newest last; `before` pages backwards.
    fn history(channel: ChannelId, before: Option<MessageId>, limit: u32) -> Vec<Message>
        = Ok(Vec::new());
    /// The direct channel with a person, created on first use.
    fn open_direct(person: PersonId) -> ChannelId;
    /// The host's channel.
    fn host_channel() -> ChannelId;
    /// Append a user turn. The reply streams back as events.
    fn send_message(channel: ChannelId, text: String, image: Option<ImagePart>) -> MessageId;
    /// End the channel's conversation now: the next message starts a new one, and what is
    /// worth keeping from this one is written as facts (`WorldChanged`). Returns at once;
    /// idle conversations are wrapped up without being asked.
    fn wrapup(channel: ChannelId) -> ();
    /// Take back one of the user's own messages and everything after it in the channel, as
    /// if it was never sent: the later messages, the model's log of them, the memories
    /// and requests they led to. Returns the message's text for the input box. Refused
    /// while a reply is being written in the channel, and for a message that is not the
    /// user's. `Rewound` follows.
    fn rewind(channel: ChannelId, message: MessageId) -> String;
    /// Pass a quoted excerpt to someone else; their answer lands in their direct channel.
    fn relay(excerpt: String, from: MessageId, to: PersonId) -> ChannelId;
    /// Everything before this message has been seen.
    fn mark_read(channel: ChannelId, up_to: MessageId) -> ();
    fn create_group(members: Vec<PersonId>, topic: Option<String>) -> ChannelId;
    /// Take a group a person suggested: the `action`-th action line of `message`, a group
    /// with no channel yet. Creates it the first time and records its channel on the line;
    /// every later call returns that same group (`Invalid` for any other action). A group
    /// deleted since is made again. A live group, not archived, that already has exactly
    /// these people is that group: nothing new is made.
    fn open_offer(message: MessageId, action: u32) -> ChannelId;
    /// Take one of the people a find suggestion (the `action`-th action line of `message`)
    /// offered: their direct channel, with the reason they were found written as its first
    /// line the first time. `Invalid` when the person is not on that line.
    fn open_found(message: MessageId, action: u32, person: PersonId) -> ChannelId;
    /// Take one of the people an invitation (the `action`-th action line of `message`, made
    /// inside a group) offered: they join that group, as with `add_members`. Returns the
    /// group. `Invalid` when the person is not on that line.
    fn open_invite(message: MessageId, action: u32, person: PersonId) -> ChannelId;
    fn delete_group(channel: ChannelId) -> ();
    /// Bring people into a live group. Each must be available (`NoPersona`, `Disabled`);
    /// anyone already in it is left as is, so repeating is harmless. Each person who joins
    /// leaves a service line in the group (no model call), and `WorldChanged` follows.
    fn add_members(channel: ChannelId, persons: Vec<PersonId>) -> ();
    /// Take a person out of a live group; their messages stay in its history and a service
    /// line says so. Not in it: nothing happens. A group keeps at least one person
    /// (`Invalid` for the last). `WorldChanged` follows.
    fn remove_member(channel: ChannelId, person: PersonId) -> ();
    /// Title a channel. `None` or blank returns to the derived title.
    fn rename_channel(channel: ChannelId, name: Option<String>) -> ();
    /// Set (or, with `None`, clear) a channel's picture. Returns the stored file.
    fn set_channel_avatar(channel: ChannelId, image: Option<ImagePart>) -> Option<MediaFile>;
    /// Move a channel out of or back into the main list. The host's channel cannot be
    /// archived (`Invalid`). A message from a person in an archived channel un-archives
    /// it (`WorldChanged`); the user's own messages do not.
    fn archive_channel(channel: ChannelId, archived: bool) -> ();

    // ---- requests and case files ----

    /// Ask a person to look into a question. `turns` absent: the tier's default.
    /// `parent`: file it under a pinned question.
    fn ask(person: PersonId, question: String, turns: Option<u32>, parent: Option<TaskId>)
        -> TaskId;
    fn tasks() -> Vec<TaskSummary> = Ok(Vec::new());
    fn case_file(task: TaskId) -> CaseFile;
    /// Pin a question nobody is on yet.
    fn open_case(question: String) -> TaskId;
    fn pin_task(task: TaskId) -> ();
    fn unpin_task(task: TaskId) -> ();
    /// Write or replace the conclusion; replacing retracts the old one.
    fn write_conclusion(task: TaskId, text: String) -> FactId;

    // ---- facts ----

    fn facts(filter: FactFilter) -> FactList = Ok(FactList::default());
    /// Retract and write anew; returns the new fact.
    fn edit_fact(fact: FactId, text: String) -> FactId;
    fn delete_fact(fact: FactId) -> ();
    fn restore_fact(fact: FactId) -> ();

    // ---- corpus ----

    /// Library-mode search: the whole corpus, no memories. Empty `persons` or `templates`
    /// mean no restriction on that side.
    fn search_corpus(query: String, persons: Vec<PersonId>, templates: Vec<String>)
        -> SearchResults;
    /// One unit in full with up to `around` neighbours on each side, for reading the
    /// source in place. `NotFound` when the id is not in the corpus.
    fn read_unit(unit: String, around: u32) -> UnitView;
    /// The unit templates of the corpus, for the search filter.
    fn corpus_templates() -> Vec<String> = Ok(Vec::new());
    fn resolve_citation(citation: Citation) -> CitationView;
    fn avatar(person: PersonId) -> Option<MediaFile> = Ok(None);
    /// The user's picture: the one they set, else the corpus's player portrait (fetched
    /// and cached like a person's avatar), else `None`.
    fn user_avatar() -> Option<MediaFile> = Ok(None);
    /// Set the user's picture; `None` returns to the default. Returns what `user_avatar`
    /// now answers. Per save.
    fn set_user_avatar(image: Option<ImagePart>) -> Option<MediaFile>;

    // ---- read-aloud ----

    /// Synthesize (or reuse the cached audio for) one message's speech.
    fn tts_play(message: MessageId) -> MediaFile;
    fn tts_voices() -> Vec<String> = Ok(Vec::new());
    fn set_voice(person: PersonId, voice: Option<String>) -> ();

    // ---- quota and settings ----

    fn quota() -> QuotaStatus;
    fn set_tier(tier: Tier) -> ();
    fn settings() -> Settings = Ok(Settings::default());
    /// The daily events the host told of, today's and the last few, newest first.
    fn daily_events() -> Vec<DailyEvent> = Ok(Vec::new());
    fn update_settings(settings: Settings) -> ();

    // ---- first run ----

    /// Show the system's file dialog for a corpus. The path chosen, or `None` when the
    /// dialog was cancelled. `NotImplemented` where there is no dialog: the user types the
    /// path instead.
    fn choose_corpus() -> Option<String>;
    /// Use the `.folio` at `path` as the corpus from now on: checked, remembered in the
    /// home directory, and the app reopens on it. `NotFound` when there is no such file,
    /// `Invalid` with field `corpus` (unreadable), `corpus_version` (written for another format)
    /// or `corpus_requirement` (needs a reader feature this build lacks).
    fn set_corpus(path: String) -> ();
    /// Open a web address (the page a passage came from) in the system browser. `Invalid`
    /// with field `url` unless it is a plain http or https address; `NotImplemented` where
    /// there is no browser to hand it to.
    fn open_external(url: String) -> ();

    // ---- saves ----

    /// Every save, the most recently played first. Endpoints, keys, the corpus and the
    /// media cache are shared by all of them.
    fn saves() -> Vec<SaveInfo> = Ok(Vec::new());
    /// An empty save. `Exists` when the name is taken (ignoring case).
    fn new_save(name: String) -> ();
    /// Close the current world and open another. `Busy` while a reply or request is
    /// being written. The caller reloads what it shows. Another process (the terminal
    /// next to the app) keeps the save it has open until it is restarted.
    fn switch_save(name: String) -> ();
    /// Close the current save and leave none open: the name for the next start is cleared,
    /// an encrypted save's password is forgotten, and the caller shows the save picker.
    /// `Busy` while a reply or request is being written. Works on a locked save too.
    fn log_out() -> ();
    /// A consistent copy under a new name; works on the current save too.
    fn copy_save(from: String, to: String) -> ();
    /// Renaming the current save needs the engine idle, like a switch.
    fn rename_save(from: String, to: String) -> ();
    /// For good. Refused for the current save.
    fn delete_save(name: String) -> ();
    /// Write a single-file copy to `path`, replacing a file there. Callers confirm
    /// before overwriting.
    fn export_save(name: String, path: String) -> ();
    /// Add the world database at `path` as a save (checked, then migrated). `name`
    /// defaults to the file's name. Returns the name used. An encrypted file needs its
    /// `password` (`Locked` without one, `WrongPassword` with a wrong one); the save keeps it.
    fn import_save(path: String, name: Option<String>, password: Option<String>) -> String;

    // ---- storage ----

    /// What the app keeps on disk, by kind.
    fn storage_usage() -> StorageUsage;
    /// Empty the named caches (`Avatars`, `Audio`) and answer the usage afterwards. A kind
    /// that is not a cache is `Invalid`; saves and the corpus are never touched.
    fn clear_storage(categories: Vec<StorageKind>) -> StorageUsage;

    // ---- encryption ----

    /// Open the encrypted save `name` with its password and remember the password until
    /// this process ends (a switch to it then needs no asking). `WrongPassword` leaves
    /// everything as it was.
    fn unlock(name: String, password: String) -> ();
    /// Encrypt, re-key or decrypt the current save, which must be unlocked: `current` is
    /// its password (`None` while it is plain), `next` the password it gets (`None`
    /// removes the encryption). Needs the engine idle. A forgotten password cannot be
    /// recovered.
    fn set_password(current: Option<String>, next: Option<String>) -> ();

    // ---- endpoints ----

    fn endpoints() -> EndpointsView = Ok(EndpointsView {
        presets: crate::endpoints::presets(),
        ..EndpointsView::default()
    });
    /// The first-run form in one step: the profile (replaced if it exists, its other models
    /// kept), the chat role, the host role when `host_model` is given, and the key. Nothing
    /// is written when any part is invalid; `Invalid` names the field (`name`, `base_url`,
    /// `model`, `host_model`, `key`).
    fn setup_endpoint(setup: EndpointSetup) -> ();
    fn add_profile(profile: Profile) -> ();
    /// Replace the profile called `name` (which may rename it).
    fn update_profile(name: String, profile: Profile) -> ();
    fn remove_profile(name: String) -> ();
    /// `GET {base_url}/models`. An endpoint failure carries the endpoint's own text.
    fn fetch_models(profile: String) -> Vec<String>;
    /// Store the key in the keychain. It is never returned by any command.
    fn set_key(profile: String, key: String) -> ();
    /// `None` switches the role off (read-aloud) or back to the default (host: the chat
    /// role); the chat role itself cannot be removed.
    fn set_role(kind: RoleKind, role: Option<Role>) -> ();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_snake_case() {
        let mut seen = std::collections::BTreeSet::new();
        for name in COMMANDS {
            assert!(seen.insert(name), "duplicate {name}");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name}"
            );
        }
    }

    /// A stub future never awaits anything, so one poll finishes it.
    fn now<T>(f: impl Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => unreachable!("stub awaited"),
        }
    }

    #[test]
    fn stub_boots_in_library_form() {
        assert_eq!(now(Stub.app_status()).unwrap().form, AppForm::Library);
        assert!(!now(Stub.endpoints()).unwrap().presets.is_empty());
        assert!(now(Stub.settings()).unwrap().notifications);
        assert!(now(Stub.coldstart()).unwrap().is_empty());
        assert_eq!(now(Stub.clock()), Err(ApiError::NotImplemented));
        assert_eq!(
            now(Stub.set_key("p".into(), "k".into())),
            Err(ApiError::NotImplemented)
        );
    }
}
