//! The desktop app: the window, the tray and the engine behind the `api` seam, in one
//! process that opens no port. Closing the window hides it and takes the app out of the Dock (macOS), leaving the tray
//! icon; the app quits from the tray.
//!
//! Startup opens the corpus from `$DRAMATIS_FOLIO`, else the one chosen in the
//! window, else the newest `.folio` in `$DRAMATIS_HOME` (default `~/.dramatis`), else the window asks for a file and the home
//! remembers the choice. Debug builds load the UI from the Vite dev server (`devUrl`); builds
//! with the `custom-protocol` feature (what `tauri build` enables) embed `ui/dist`, which
//! must be built first. See the README's "Desktop app" section.

// Release builds are GUI apps; no console window on platforms that would open one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod presence;
mod shell;

#[cfg(target_os = "macos")]
use tauri::RunEvent;
use tauri::{Manager, WindowEvent};

fn main() {
    let bindings = api::builder();
    tauri::Builder::default()
        .menu(shell::app_menu)
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // The placeholder until `shell::start` opens the real engine.
        .manage(api::Handle::new(api::Stub))
        .invoke_handler(bindings.invoke_handler())
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                shell::hide(window.app_handle());
            }
            WindowEvent::Focused(true) => shell::seen(window.app_handle()),
            _ => {}
        })
        .setup(move |app| {
            bindings.mount_events(app);
            shell::start(app)
        })
        .build(tauri::generate_context!())
        .expect("the app failed to start")
        .run(|app, event| {
            // Clicking the Dock icon brings the hidden window back.
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { .. } = event {
                shell::show(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
