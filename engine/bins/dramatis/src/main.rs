//! The desktop app: the window, the tray and the engine behind the `api` seam, in one
//! process that opens no port. Closing the window hides it; the app quits from the tray.
//!
//! Startup opens the corpus from `$DRAMATIS_FOLIO`, else the newest `.folio` in
//! `$DRAMATIS_HOME` (default `~/.dramatis`), else asks for a file and links it into the
//! home directory. Debug builds load the UI from the Vite dev server (`devUrl`); builds
//! with the `custom-protocol` feature (what `tauri build` enables) embed `ui/dist`, which
//! must be built first. See the README's "Desktop app" section.

// Release builds are GUI apps; no console window on platforms that would open one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod shell;

use tauri::{RunEvent, WindowEvent};

fn main() {
    let bindings = api::builder();
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // The placeholder until `shell::start` opens the real engine.
        .manage(api::Handle::new(api::Stub))
        .invoke_handler(bindings.invoke_handler())
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            bindings.mount_events(app);
            shell::start(app)
        })
        .build(tauri::generate_context!())
        .expect("the app failed to start")
        .run(|app, event| {
            // Clicking the Dock icon brings the hidden window back.
            if let RunEvent::Reopen { .. } = event {
                shell::show(app);
            }
        });
}
