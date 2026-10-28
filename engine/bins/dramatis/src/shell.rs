//! What the app wires around the engine: the tray, notifications, the corpus picker and
//! the delivery of events to the window. The engine and its ports come from `runtime`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use api::Engine;
use api::error::ApiResult;
use api::home::Home;
use api::port;
use runtime::{AgentBridge, Options, Runtime};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

/// How often idle conversations are wrapped up and due seeds looked at.
const TICK: Duration = Duration::from_secs(60);

// ---- events and notifications ----

/// Everything the engine and the agent push goes to the window; a person's unprompted
/// line also becomes a system notification when the window is not in front.
pub struct AppSink {
    app: AppHandle,
    engine: Mutex<Weak<Engine>>,
}

impl AppSink {
    fn deliver(&self, event: port::Event) {
        if let port::Event::Notification(n) = &event {
            self.notify(n);
        }
        api::emit(&self.app, event);
    }

    fn notify(&self, n: &api::events::Notification) {
        let in_front = self
            .app
            .get_webview_window("main")
            .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false));
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade();
        let Some(engine) = engine else { return };
        // The switch and the quiet hours are the user's: honoured here, at the one place
        // a notification leaves the app.
        if in_front || !engine.may_notify().unwrap_or(false) {
            return;
        }
        let name = engine
            .corpus_handle()
            .map_or_else(|| n.person.0.clone(), |c| c.display(&n.person.0));
        if let Err(e) = self
            .app
            .notification()
            .builder()
            .title(name)
            .body(&n.text)
            .show()
        {
            eprintln!("dramatis: notification failed: {e}");
        }
    }
}

impl port::Sink for AppSink {
    fn emit(&self, event: port::Event) {
        self.deliver(event);
    }
}

// ---- opening ----

/// The engine and agent bridge currently serving the window.
#[derive(Default)]
pub struct Current(Mutex<Option<(Arc<Engine>, Arc<AgentBridge>)>>);

/// Open the home and the corpus (if any) and make it what the window talks to.
pub fn open(app: &AppHandle, home: Home, folio: Option<PathBuf>) -> ApiResult<()> {
    let sink = Arc::new(AppSink {
        app: app.clone(),
        engine: Mutex::new(Weak::new()),
    });
    // A reopen on the same home (a new corpus) keeps the save unlocked: the password the
    // user entered travels in memory from the old engine to the new one.
    let password = app
        .state::<Current>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|(engine, _)| engine.open_password());
    let options = Options {
        password,
        ..Options::default()
    };
    let Runtime { engine, bridge } = Runtime::open(home, folio, sink.clone(), options)?;
    *sink.engine.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&engine);
    // Stop the previous bridge before the swap: its in-flight turns would otherwise keep
    // emitting into the new window and writing the same world.
    let previous = app
        .state::<Current>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some((_, old)) = previous {
        old.cancel();
    }
    app.state::<api::Handle>().replace(engine.clone());

    // Off the first screen's path: the retrieval index, and the network answer the form needs.
    let warm = engine.clone();
    tauri::async_runtime::spawn(async move {
        let index = warm.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || index.warm()).await;
    });
    let probe = engine.clone();
    tauri::async_runtime::spawn(async move {
        probe.probe_network().await;
    });
    // The terminal client shares the world database; notice what it writes.
    tauri::async_runtime::spawn(api::Engine::watch_external(Arc::downgrade(&engine)));
    *app.state::<Current>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = bridge.map(|bridge| (engine, bridge));
    Ok(())
}

/// Ask for a `.folio` and open it. The choice is remembered as a link in the home
/// directory, which is where the next start looks.
pub fn pick_corpus(app: &AppHandle, home: Home) {
    let app = app.clone();
    app.clone()
        .dialog()
        .file()
        .add_filter("Corpus", &["folio"])
        .pick_file(move |picked| {
            let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
                return;
            };
            if let Some(name) = path.file_name() {
                let link = home.root().join(name);
                if link != path && !link.exists() {
                    let _ = std::os::unix::fs::symlink(&path, &link);
                }
            }
            match open(&app, home, Some(path)) {
                Ok(()) => reload(&app),
                Err(e) => eprintln!("dramatis: could not open the corpus: {e}"),
            }
        });
}

fn reload(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.eval("location.reload()");
    }
}

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Window, tray and background steps. Called once from `setup`.
pub fn start(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    app.manage(Current::default());
    let home = Home::from_env();
    // The webview may read the cached media, the notes people write and the pictures the
    // user chose, and nothing else; the home can be moved by `DRAMATIS_HOME`, so the scope
    // is not static.
    let scope = app.asset_protocol_scope();
    for dir in [home.media(), home.office(), home.uploads()] {
        // The scope canonicalizes its paths, so they must exist (a fresh home has none of them yet).
        let _ = std::fs::create_dir_all(&dir);
        let _ = scope.allow_directory(dir, true);
    }
    let folio = home.resolve_folio();
    let have_corpus = folio.is_some();
    if let Err(e) = open(&handle, home.clone(), folio) {
        // Leave the placeholder engine in place (library form, nothing opened).
        eprintln!("dramatis: could not start the engine: {e}");
    }
    if !have_corpus {
        pick_corpus(&handle, home.clone());
    }

    let show_item = MenuItem::with_id(app, "show", "Show dramatis", true, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open", "Open corpus...", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &open_item, &quit_item])?;
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => show(app),
            "open" => pick_corpus(app, home.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    // The app keeps running with the window closed: conversations that went idle are
    // wrapped up and due commitments may open, without anyone looking.
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            let current = handle
                .state::<Current>()
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some((engine, bridge)) = current {
                engine.probe_network().await;
                if engine.chat_configured() {
                    bridge.tick().await;
                }
            }
        }
    });
    Ok(())
}
