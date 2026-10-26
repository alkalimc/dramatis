//! The seam between the engine and the UI.
//!
//! The UI and the engine share one process; the UI reaches the engine only through the
//! commands in [`commands`] and hears from it only through [`events`]. Those types exist
//! once, here: the TypeScript side (`ui/src/api.gen.ts`) is generated from them, and CI
//! fails if the committed file differs from a fresh export.
//!
//! No user-visible sentence is composed on this side. Errors are codes, activities and
//! tool actions are typed; the UI turns them into words through i18n keys and the
//! corpus's wording.
//!
//! Also here, because both the app and the offline tools read them: the tool schemas the
//! model sees ([`tools`]) and the endpoints file ([`endpoints`]).

pub mod commands;
pub mod convert;
pub mod corpus;
pub mod display;
pub mod endpoints;
pub mod engine;
pub mod error;
pub mod events;
pub mod home;
pub mod media;
pub mod meta;
pub mod params;
pub mod port;
pub mod tools;
pub mod types;
pub mod views;

use std::sync::{Arc, RwLock};

pub use commands::{Api, COMMANDS, Stub};
pub use engine::{Engine, Ports};
pub use error::{ApiError, ApiResult};
pub use events::EVENTS;

/// The implementation the app manages as Tauri state; every command forwards to it. It
/// can be replaced while the app runs (a different corpus was opened); commands already
/// in flight finish against the one they started with.
#[derive(Clone)]
pub struct Handle(Arc<RwLock<Arc<dyn Api>>>);

impl Handle {
    pub fn new(api: impl Api) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(api))))
    }

    pub fn replace(&self, api: Arc<dyn Api>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = api;
    }

    pub fn current(&self) -> Arc<dyn Api> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Command and event registration for the app, and the source of the TypeScript export.
#[cfg(feature = "tauri")]
pub fn builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(commands::handlers::collect())
        .events(events::collect())
        // Types here serialize and deserialize alike; one TS name per type, not three.
        .disable_serde_phases()
}

/// Write the TypeScript bindings. Deterministic: same types, same bytes.
#[cfg(feature = "tauri")]
pub fn export_bindings(path: &std::path::Path) -> Result<(), specta_typescript::Error> {
    builder().export(
        specta_typescript::Typescript::default().header(
            "// Generated from engine/crates/api by `cargo test -p dramatis`. Do not edit.\n",
        ),
        path,
    )
}

/// Deliver one engine event to the window as its typed Tauri event.
#[cfg(feature = "tauri")]
pub fn emit<H: tauri::Emitter<tauri::Wry> + tauri::Manager<tauri::Wry>>(
    app: &H,
    event: port::Event,
) {
    use port::Event::*;
    use tauri_specta::Event as _;
    let sent = match event {
        MessageDelta(e) => e.emit(app),
        MessageAdded(e) => e.emit(app),
        ToolCalled(e) => e.emit(app),
        WorldChanged(e) => e.emit(app),
        ModeChanged(e) => e.emit(app),
        RosterChanged(e) => e.emit(app),
        QuotaChanged(e) => e.emit(app),
        Notification(e) => e.emit(app),
    };
    if let Err(e) = sent {
        eprintln!("dramatis: could not deliver an event: {e}");
    }
}
