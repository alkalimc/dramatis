//! The desktop app: the window, and the engine behind the `api` seam, in one process.
//!
//! Debug builds load the UI from the Vite dev server (`devUrl`); builds with the
//! `custom-protocol` feature (what `tauri build` enables) embed `ui/dist`, which must be
//! built first. See the README's "Desktop app" section.

// Release builds are GUI apps; no console window on platforms that would open one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let bindings = api::builder();
    tauri::Builder::default()
        .manage(api::Handle::new(api::Stub))
        .invoke_handler(bindings.invoke_handler())
        .setup(move |app| {
            bindings.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the app failed to start");
}
