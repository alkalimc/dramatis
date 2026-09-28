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
pub mod endpoints;
pub mod error;
pub mod events;
pub mod tools;
pub mod types;
pub mod views;

use std::sync::Arc;

pub use commands::{Api, COMMANDS, Stub};
pub use error::{ApiError, ApiResult};
pub use events::EVENTS;

/// The implementation the app manages as Tauri state; every command forwards to it.
#[derive(Clone)]
pub struct Handle(pub Arc<dyn Api>);

impl Handle {
    pub fn new(api: impl Api) -> Self {
        Self(Arc::new(api))
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
