//! What the app wires around the engine: the ports it supplies, the tray, notifications
//! and the corpus picker.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use agent::endpoint::{Http, Keychain as AgentKeys, Transport};
use api::endpoints::{KEYCHAIN_SERVICE, Profile, keychain_account};
use api::error::{ApiError, ApiResult};
use api::home::Home;
use api::params::Params;
use api::port::{self, KeyStore, LlmPort, LocalClock, NetPort, Speech};
use api::{Engine, Ports};
use async_trait::async_trait;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

use crate::runtime::{Bridge, Port};

/// How often idle conversations are wrapped up and due seeds looked at.
const TICK: Duration = Duration::from_secs(60);
/// How long a reachability probe waits for the endpoint's host.
const PROBE: Duration = Duration::from_millis(1500);

// ---- ports ----

/// The system keychain, under the same service and accounts the agent reads.
struct Keys;

fn entry(profile: &str) -> ApiResult<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_account(profile)).map_err(keychain)
}

fn keychain(e: keyring::Error) -> ApiError {
    ApiError::Keychain {
        detail: e.to_string(),
    }
}

impl KeyStore for Keys {
    fn set(&self, profile: &str, key: &str) -> ApiResult<()> {
        entry(profile)?.set_password(key).map_err(keychain)
    }

    fn has(&self, profile: &str) -> bool {
        entry(profile).is_ok_and(|e| e.get_password().is_ok())
    }

    fn delete(&self, profile: &str) -> ApiResult<()> {
        match entry(profile)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain(e)),
        }
    }

    fn rename(&self, from: &str, to: &str) -> ApiResult<()> {
        match entry(from)?.get_password() {
            Ok(key) => {
                self.set(to, &key)?;
                self.delete(from)
            }
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain(e)),
        }
    }
}

/// `GET /models` through the agent's transport. Read-aloud (`voices`, `speech`) waits for
/// the shared client crate: until it lands they answer "not implemented".
struct Llm {
    http: Http,
}

#[async_trait]
impl LlmPort for Llm {
    async fn models(&self, profile: &Profile) -> ApiResult<Vec<String>> {
        let target = agent::endpoint::target(profile, &AgentKeys)?;
        Ok(self.http.models(&target).await?)
    }

    async fn voices(&self, _: &Profile, _: &str) -> ApiResult<Vec<String>> {
        Err(ApiError::NotImplemented)
    }

    async fn speech(&self, _: &Profile, _: &str, _: Option<&str>, _: &str) -> ApiResult<Speech> {
        Err(ApiError::NotImplemented)
    }
}

/// Reachability is a TCP connect to the endpoint's host: nothing is sent. Plain fetches
/// (avatars) wait for the shared client crate and answer "no network" until then.
struct Net;

#[async_trait]
impl NetPort for Net {
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
        Err("no http client wired".into())
    }

    async fn reachable(&self, base_url: &str) -> bool {
        let Some((host, port)) = host_port(base_url) else {
            return false;
        };
        matches!(
            tokio::time::timeout(PROBE, tokio::net::TcpStream::connect((host.as_str(), port)))
                .await,
            Ok(Ok(_))
        )
    }
}

fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let default = if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        80
    };
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || p.chars().all(|c| c.is_ascii_digit()) => {
            Some((h.trim_matches(['[', ']']).to_owned(), p.parse().ok()?))
        }
        _ => Some((authority.trim_matches(['[', ']']).to_owned(), default)),
    }
}

/// The agent reads the same local clock.
struct Local;

impl agent::Clock for Local {
    fn now(&self) -> world::clock::Now {
        port::Clock::now(&LocalClock)
    }
}

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

impl agent::Sink for AppSink {
    fn emit(&self, event: agent::Event) {
        use agent::Event as A;
        use port::Event as P;
        self.deliver(match event {
            A::MessageDelta(e) => P::MessageDelta(e),
            A::MessageAdded(e) => P::MessageAdded(e),
            A::ToolCalled(e) => P::ToolCalled(e),
            A::WorldChanged(e) => P::WorldChanged(e),
            A::ModeChanged(e) => P::ModeChanged(e),
            A::QuotaChanged(e) => P::QuotaChanged(e),
            A::Notification(e) => P::Notification(e),
        });
    }
}

// ---- opening ----

/// The engine and agent bridge currently serving the window.
#[derive(Default)]
pub struct Current(Mutex<Option<(Arc<Engine>, Arc<Bridge>)>>);

/// Open the home and the corpus (if any) and make it what the window talks to.
pub fn open(app: &AppHandle, home: Home, folio: Option<PathBuf>) -> ApiResult<()> {
    let params = Params::load(&home.params()).map_err(|detail| ApiError::Io { detail })?;
    let sink = Arc::new(AppSink {
        app: app.clone(),
        engine: Mutex::new(Weak::new()),
    });
    let mut ports = Ports {
        llm: Arc::new(Llm {
            http: Http::default(),
        }),
        keys: Arc::new(Keys),
        net: Arc::new(Net),
        sink: sink.clone(),
        ..Ports::default()
    };
    let bridge = folio.as_ref().map(|path| {
        Bridge::new(
            home.clone(),
            path.clone(),
            params.clone(),
            api::endpoints::Endpoints::default(),
            sink.clone(),
            Arc::new(Local),
        )
    });
    if let Some(b) = &bridge {
        ports.agent = Arc::new(Port(b.clone()));
    }
    let engine = Arc::new(Engine::open(home, folio, ports)?);
    *sink.engine.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&engine);
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
    if let Some(bridge) = bridge {
        *app.state::<Current>()
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((engine, bridge));
    }
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
