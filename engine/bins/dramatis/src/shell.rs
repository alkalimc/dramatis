//! What the app wires around the engine: the tray, notifications, the corpus picker and
//! the delivery of events to the window. The engine and its ports come from `runtime`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
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

use crate::presence;

/// How often idle conversations are wrapped up and due seeds looked at.
const TICK: Duration = Duration::from_secs(60);

// ---- events and notifications ----

/// Everything the engine and the agent push goes to the window. A message from the host
/// or a person (an opening, a reply, a due item, a group line) also becomes one system
/// notification while the window is hidden or not in front.
pub struct AppSink {
    app: AppHandle,
    engine: Mutex<Weak<Engine>>,
}

/// State that outlives a reopened corpus: the ids already notified, and how many arrivals
/// nobody has looked at yet (the tray's number).
#[derive(Default)]
pub struct Arrivals {
    recent: Mutex<presence::Recent>,
    unseen: AtomicU32,
}

impl AppSink {
    fn deliver(&self, event: port::Event) {
        // The `Notification` event the agent sends for an opening is not used: the same
        // line arrives as a message, which covers every kind of arrival once.
        if let port::Event::MessageAdded(added) = &event {
            self.notify(&added.message);
        }
        api::emit(&self.app, event);
    }

    fn notify(&self, m: &api::views::Message) {
        if matches!(m.author, api::types::Author::User) {
            return;
        }
        let window = self.app.get_webview_window("main");
        let flag = |f: Option<bool>| f.unwrap_or(false);
        let (visible, focused) = (
            flag(window.as_ref().and_then(|w| w.is_visible().ok())),
            flag(window.as_ref().and_then(|w| w.is_focused().ok())),
        );
        if !presence::unattended(visible, focused) {
            return;
        }
        let engine = self
            .engine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade();
        let Some(engine) = engine else { return };
        // The switch and the quiet hours are the user's: honoured here, at the one place
        // a notification leaves the app.
        if !engine.may_notify().unwrap_or(false) {
            return;
        }
        let corpus = engine.corpus_handle();
        let (host_name, scene) = corpus.as_ref().map_or_else(
            || (String::from("host"), ("*".to_owned(), "*".to_owned())),
            |c| {
                let host = c.manifest.wording.get("host.name").cloned();
                (
                    host.unwrap_or_else(|| "host".into()),
                    c.manifest.scene_marker.clone(),
                )
            },
        );
        let style = presence::Style {
            host_name: &host_name,
            scene: (&scene.0, &scene.1),
        };
        let arrivals = self.app.state::<Arrivals>();
        let found = presence::arrival(
            &m.author,
            &m.id.0,
            &m.text,
            &style,
            |id| {
                corpus
                    .as_ref()
                    .map_or_else(|| id.to_owned(), |c| c.display(id))
            },
            &mut arrivals.recent.lock().unwrap_or_else(|e| e.into_inner()),
        );
        let Some(found) = found else { return };
        let unseen = arrivals.unseen.fetch_add(1, Ordering::Relaxed) + 1;
        set_badge(&self.app, unseen);
        if let Err(e) = self
            .app
            .notification()
            .builder()
            .title(found.title)
            .body(found.body)
            .show()
        {
            eprintln!("dramatis: notification failed: {e}");
        }
    }
}

const TRAY: &str = "main";

/// The number beside the tray icon and in its tooltip.
fn set_badge(app: &AppHandle, unseen: u32) {
    let Some(tray) = app.tray_by_id(TRAY) else {
        return;
    };
    let _ = tray.set_title(presence::badge(unseen));
    let _ = tray.set_tooltip(Some(match unseen {
        0 => "dramatis".to_owned(),
        n => format!("dramatis ({n} new)"),
    }));
}

/// The window came to the front: what arrived is seen.
pub fn seen(app: &AppHandle) {
    if app.state::<Arrivals>().unseen.swap(0, Ordering::Relaxed) > 0 {
        set_badge(app, 0);
    }
}

/// Put the Dock icon in step with the window: shown while it is open, the menu bar alone
/// otherwise.
pub fn sync_dock(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let visible = app
            .get_webview_window("main")
            .is_some_and(|w| w.is_visible().unwrap_or(false));
        let policy = match presence::dock_for(visible) {
            presence::Dock::Shown => tauri::ActivationPolicy::Regular,
            presence::Dock::Hidden => tauri::ActivationPolicy::Accessory,
        };
        if let Err(e) = app.set_activation_policy(policy) {
            eprintln!("dramatis: could not change the Dock icon: {e}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Close to the menu bar: hide the window and leave the Dock.
pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    sync_dock(app);
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
    // Choosing a corpus in the window reopens the home on it, after the command returns.
    let again = {
        let (app, home) = (app.clone(), home.clone());
        Arc::new(move || {
            let (app, home) = (app.clone(), home.clone());
            tauri::async_runtime::spawn(async move {
                let folio = home.resolve_folio();
                match open(&app, home, folio) {
                    Ok(()) => reload(&app),
                    Err(e) => eprintln!("dramatis: could not open the corpus: {e}"),
                }
            });
        })
    };
    let dialog = {
        let app = app.clone();
        Arc::new(move || {
            app.dialog()
                .file()
                .add_filter("Corpus", &["folio"])
                .blocking_pick_file()
                .and_then(|p| p.into_path().ok())
                .map(|p| p.display().to_string())
        })
    };
    let options = Options {
        password,
        restart: Some(again),
        choose_file: Some(dialog),
        open_url: Some(Arc::new(open_in_browser)),
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

/// Ask for a `.folio` and open it. The choice is remembered in the home directory, which
/// is where the next start looks.
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
            if let Err(e) = home.set_chosen_corpus(&path) {
                eprintln!("dramatis: could not remember the corpus: {e}");
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
        // The Dock icon must be back before the window is asked to take the focus.
        sync_dock(app);
        let _ = w.set_focus();
    }
    seen(app);
}

/// Window, tray and background steps. Called once from `setup`.
pub fn start(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    app.manage(Current::default());
    app.manage(Arrivals::default());
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
    // With no corpus the window asks for one itself (a file dialog or a typed path).
    let folio = home.resolve_folio();
    if let Err(e) = open(&handle, home.clone(), folio) {
        // Leave the placeholder engine in place (library form, nothing opened).
        eprintln!("dramatis: could not start the engine: {e}");
    }

    let show_item = MenuItem::with_id(app, "show", "Show dramatis", true, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open", "Open corpus...", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &open_item, &quit_item])?;
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("dramatis")
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
    // The window starts shown, so the Dock icon starts shown.
    sync_dock(&handle);

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
                    let shown = handle
                        .get_webview_window("main")
                        .is_some_and(|w| w.is_visible().unwrap_or(false));
                    bridge.tick(shown).await;
                }
            }
        }
    });
    Ok(())
}

/// Hand an http(s) address to the system's default browser; the engine has already checked
/// the address, and it goes as one argument, never through a shell.
fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    if let Err(e) = command.arg(url).spawn() {
        eprintln!("dramatis: could not open the browser: {e}");
    }
}
