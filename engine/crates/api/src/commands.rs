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
                    api.0.$name($($arg),*).await
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
    });
    /// The corpus's runtime phrasing.
    fn wording() -> Wording = Ok(Wording::default());
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
    /// Pass a quoted excerpt to someone else; their answer lands in their direct channel.
    fn relay(excerpt: String, from: MessageId, to: PersonId) -> ChannelId;
    /// Everything before this message has been seen.
    fn mark_read(channel: ChannelId, up_to: MessageId) -> ();
    fn create_group(members: Vec<PersonId>, topic: Option<String>) -> ChannelId;
    fn delete_group(channel: ChannelId) -> ();

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

    /// Library-mode search: the whole corpus, no memories.
    fn search_corpus(query: String, persons: Vec<PersonId>) -> SearchResults;
    fn resolve_citation(citation: Citation) -> CitationView;
    fn avatar(person: PersonId) -> Option<MediaFile> = Ok(None);

    // ---- read-aloud ----

    /// Synthesize (or reuse the cached audio for) one message's speech.
    fn tts_play(message: MessageId) -> MediaFile;
    fn tts_voices() -> Vec<String> = Ok(Vec::new());
    fn set_voice(person: PersonId, voice: Option<String>) -> ();

    // ---- quota and settings ----

    fn quota() -> QuotaStatus;
    fn set_tier(tier: Tier) -> ();
    fn settings() -> Settings = Ok(Settings::default());
    fn update_settings(settings: Settings) -> ();

    // ---- endpoints ----

    fn endpoints() -> EndpointsView = Ok(EndpointsView {
        presets: crate::endpoints::presets(),
        ..EndpointsView::default()
    });
    fn add_profile(profile: Profile) -> ();
    /// Replace the profile called `name` (which may rename it).
    fn update_profile(name: String, profile: Profile) -> ();
    fn remove_profile(name: String) -> ();
    /// `GET {base_url}/models`. An endpoint failure carries the endpoint's own text.
    fn fetch_models(profile: String) -> Vec<String>;
    /// Store the key in the keychain. It is never returned by any command.
    fn set_key(profile: String, key: String) -> ();
    /// `None` switches the role off (only meaningful for read-aloud).
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
