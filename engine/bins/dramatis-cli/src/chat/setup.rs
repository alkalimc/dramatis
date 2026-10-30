//! First run at a terminal: what to do when the corpus, the endpoints or a key is missing.
//!
//! Each step asks in plain sentences, checks the answer and says what is wrong with it. The
//! checks and the writes are the engine's own (the window's first-run form calls the same
//! ones), so a file written here is one the app reads back. Nothing here is asked when
//! input is not a terminal.

use std::path::PathBuf;

use anyhow::Result;
use api::endpoints::WireApi;
use api::error::ApiError;
use api::home::Home;
use api::views::{EndpointSetup, InterfaceLanguage, MonthDay};
use api::{Api, Engine};

use super::show::Names;

/// Tries at one answer before the step is given up.
const TRIES: usize = 3;

/// Where answers come from: the keyboard, or a script in tests.
pub trait Ask {
    /// One line, trimmed; `None` at the end of input.
    fn line(&mut self, prompt: &str) -> Option<String>;
    /// A line with nothing echoed; `None` when none was given.
    fn secret(&mut self, prompt: &str) -> Option<String>;
    fn say(&mut self, text: &str);
}

/// The keyboard and the screen.
pub struct Terminal;

impl Ask for Terminal {
    fn line(&mut self, prompt: &str) -> Option<String> {
        use std::io::Write;
        print!("{prompt}");
        std::io::stdout().flush().ok()?;
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    }

    fn secret(&mut self, prompt: &str) -> Option<String> {
        super::secret::read(prompt)
    }

    fn say(&mut self, text: &str) {
        println!("{text}");
    }
}

/// What the corpus step found out about a typed path, in words.
fn corpus_problem(e: &ApiError) -> &'static str {
    match e {
        ApiError::NotFound => "there is no file there",
        ApiError::Invalid { field } if field == "corpus_version" => {
            "that corpus was written for another version of this program"
        }
        ApiError::Invalid { field } if field == "corpus_requirement" => {
            "that corpus needs a feature this version does not have"
        }
        _ => "that file is not a corpus this program can read",
    }
}

/// Ask for the corpus file until one checks out, and remember it in the home directory.
/// `None`: none was given.
pub fn corpus(home: &Home, ask: &mut dyn Ask) -> Result<Option<PathBuf>> {
    ask.say(&format!(
        "No corpus (.folio file) is set up. Give its path once; it is remembered in {}.",
        home.root().display()
    ));
    for _ in 0..TRIES {
        let Some(typed) = ask.line("path to the corpus (empty to stop): ") else {
            return Ok(None);
        };
        if typed.is_empty() {
            return Ok(None);
        }
        match api::corpus::choose(&typed) {
            Ok(path) => {
                home.set_chosen_corpus(&path)?;
                return Ok(Some(path));
            }
            Err(e) => ask.say(&format!("{}; try again", corpus_problem(&e))),
        }
    }
    Ok(None)
}

/// What is missing for talking to the people, if anything.
#[derive(Debug, PartialEq, Eq)]
pub enum Gap {
    None,
    /// No endpoint is set up.
    Endpoint,
    /// The chat endpoint has no key in the keychain.
    Key(String),
}

pub async fn gap(engine: &Engine) -> Gap {
    let Ok(view) = engine.endpoints().await else {
        return Gap::None;
    };
    let Some(roles) = &view.roles else {
        return Gap::Endpoint;
    };
    match view
        .profiles
        .iter()
        .find(|p| p.profile.name == roles.chat.profile)
    {
        Some(p) if !p.has_key => Gap::Key(p.profile.name.clone()),
        _ => Gap::None,
    }
}

/// Anything but a no (or the end of input) is a yes.
fn yes(ask: &mut dyn Ask, names: &Names, prompt: &str) -> bool {
    let no = names.text("cli.answer.no");
    ask.line(prompt)
        .map(|a| a.to_lowercase())
        .is_some_and(|a| !no.split('|').any(|n| n == a))
}

/// Asked once, when the home has no answer yet: whether to check for a newer version by
/// itself. Only a yes turns it on; the answer is kept either way, and the end of input keeps
/// nothing, so the question comes again.
pub async fn update_check(engine: &Engine, names: &Names, ask: &mut dyn Ask) {
    if engine.home().update_check_asked() {
        return;
    }
    let Some(answer) = ask.line(&names.text("cli.update.ask")) else {
        return;
    };
    let on = names
        .text("cli.answer.yes")
        .split('|')
        .any(|y| y == answer.to_lowercase());
    match engine.set_auto_update_check(on).await {
        Ok(()) => ask.say(&names.text(if on {
            "cli.update.on"
        } else {
            "cli.update.off"
        })),
        Err(e) => ask.say(&names.api_error(&e)),
    }
}

/// What the endpoint step says about a refusal.
fn endpoint_problem(e: &ApiError, names: &Names) -> String {
    match e {
        ApiError::Invalid { field } => match field.as_str() {
            "name" => names.text("cli.setup.bad.name"),
            "base_url" => names.text("cli.setup.bad.base_url"),
            "model" => names.text("cli.setup.bad.model"),
            "host_model" => names.text("cli.setup.bad.host_model"),
            other => names.say("cli.setup.bad.other", &[("field", &other)]),
        },
        ApiError::Keychain { detail } => names.say("cli.setup.bad.keychain", &[("detail", detail)]),
        other => names.api_error(other),
    }
}

/// Offer to set up the chat endpoint (or its key) and do it. Returns whether the endpoints
/// changed. Declining is fine: the program then works as a library, searching only.
pub async fn endpoints(engine: &Engine, names: &Names, gap: Gap, ask: &mut dyn Ask) -> bool {
    match gap {
        Gap::None => return false,
        Gap::Key(profile) => {
            ask.say(&names.say("cli.setup.no_key", &[("profile", &profile)]));
            let Some(key) = ask.secret(&names.text("cli.setup.key_prompt")) else {
                return false;
            };
            return match engine.set_key(profile, key).await {
                Ok(()) => {
                    ask.say(&names.text("cli.setup.key_stored"));
                    true
                }
                Err(e) => {
                    ask.say(&endpoint_problem(&e, names));
                    false
                }
            };
        }
        Gap::Endpoint => {}
    }
    ask.say(&names.text("cli.setup.no_model"));
    if !yes(ask, names, &names.text("cli.setup.offer")) {
        return false;
    }
    let presets = engine
        .endpoints()
        .await
        .map(|v| v.presets)
        .unwrap_or_default();
    if !presets.is_empty() {
        ask.say(&names.text("cli.setup.presets"));
        for (i, p) in presets.iter().enumerate() {
            ask.say(&format!("  {}  {} ({})", i + 1, p.name, p.base_url));
        }
    }
    let start = ask
        .line(&names.text("cli.setup.preset_prompt"))
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| presets.get(n.checked_sub(1)?))
        .cloned();
    for _ in 0..TRIES {
        let Some(setup) = form(&start, names, ask) else {
            return false;
        };
        match engine.setup_endpoint(setup).await {
            Ok(()) => {
                ask.say(&names.text("cli.setup.saved"));
                return true;
            }
            Err(e) => ask.say(&names.say(
                "cli.setup.again",
                &[("problem", &endpoint_problem(&e, names))],
            )),
        }
    }
    false
}

fn form(
    start: &Option<api::endpoints::Profile>,
    names: &Names,
    ask: &mut dyn Ask,
) -> Option<EndpointSetup> {
    let with_default = |ask: &mut dyn Ask, label: &str, default: Option<&str>| {
        let shown = default.map_or(String::new(), |d| format!(" [{d}]"));
        let typed = ask.line(&format!("{label}{shown}: "))?;
        Some(if typed.is_empty() {
            default.unwrap_or_default().to_owned()
        } else {
            typed
        })
    };
    let name = with_default(
        ask,
        &names.text("cli.setup.field.name"),
        Some(start.as_ref().map_or("main", |p| &p.name)),
    )?;
    let base_url = with_default(
        ask,
        &names.text("cli.setup.field.address"),
        start.as_ref().map(|p| p.base_url.as_str()),
    )?;
    let wire = with_default(
        ask,
        &names.text("cli.setup.field.wire"),
        Some(match start.as_ref().map(|p| p.wire_api) {
            Some(WireApi::Messages) => "messages",
            Some(WireApi::Responses) => "responses",
            _ => "chat",
        }),
    )?;
    let wire_api = match wire.as_str() {
        "responses" => WireApi::Responses,
        "messages" => WireApi::Messages,
        "chat" => WireApi::Chat,
        _ => {
            ask.say(&names.text("cli.setup.bad_wire"));
            WireApi::Chat
        }
    };
    let model = with_default(ask, &names.text("cli.setup.field.model"), None)?;
    let host_model = with_default(ask, &names.text("cli.setup.field.host_model"), None)?;
    let key = ask.secret(&names.text("cli.setup.field.key"));
    Some(EndpointSetup {
        name,
        base_url,
        wire_api,
        model,
        host_model: Some(host_model).filter(|m| !m.is_empty()),
        key,
    })
}

/// Whether this start should walk the player through the first-run questions: a save is
/// open and has not seen them yet (the same flag the app reads, so answering or skipping
/// in either front end is enough).
pub async fn needs_first_run(engine: &Engine, _home: &Home) -> bool {
    engine
        .app_status()
        .await
        .is_ok_and(|s| s.save.is_some() && s.needs_setup)
}

/// The first-run questions, each optional: the interface language (only while the home has
/// none: it is shared with the app), a name, a birthday and a short bio. Empty answers skip.
pub async fn first_run(engine: &Engine, names: &Names, ask: &mut dyn Ask) {
    if engine.home().interface_language().is_none() {
        ask.say(&names.text("cli.first.language"));
        let chosen = match ask
            .line(&names.text("cli.first.language_prompt"))
            .as_deref()
            .map(str::trim)
        {
            Some("2" | "en" | "english") => InterfaceLanguage::English,
            _ => InterfaceLanguage::Corpus,
        };
        let reloaded = async {
            engine.set_interface_language(chosen).await?;
            names.reload(engine).await
        }
        .await;
        if let Err(e) = reloaded {
            ask.say(&names.api_error(&e));
            return;
        }
    }
    let Ok(mut settings) = engine.settings().await else {
        return;
    };
    if let Some(name) = ask
        .line(&names.text("cli.first.name"))
        .filter(|n| !n.is_empty())
    {
        settings.user_name = Some(name);
    }
    if let Some(typed) = ask
        .line(&names.text("cli.first.birthday"))
        .filter(|b| !b.is_empty())
    {
        let parsed = typed
            .split_once('-')
            .and_then(|(m, d)| Some((m.trim().parse::<u8>().ok()?, d.trim().parse::<u8>().ok()?)))
            .map(|(month, day)| MonthDay { month, day });
        match parsed {
            Some(day) => settings.birthday = Some(day),
            None => ask.say(&names.text("cli.birthday.invalid")),
        }
    }
    if let Some(bio) = ask
        .line(&names.say("cli.first.bio", &[("max", &world::settings::BIO_MAX)]))
        .filter(|b| !b.is_empty())
    {
        if bio.chars().count() > world::settings::BIO_MAX {
            ask.say(&names.say("cli.bio.too_long", &[("max", &world::settings::BIO_MAX)]));
        } else {
            settings.bio = Some(bio);
        }
    }
    let saved = engine.update_settings(settings).await;
    if saved.is_ok() {
        avatar_step(engine, names, ask).await;
    }
    match saved {
        Ok(()) => {
            if let Err(e) = names.refresh_user(engine).await {
                ask.say(&names.api_error(&e));
            }
        }
        // A birthday the engine refuses (month 13) is the one thing it can refuse here.
        Err(ApiError::Invalid { .. }) => ask.say(&names.text("cli.birthday.invalid")),
        Err(e) => ask.say(&names.api_error(&e)),
    }
}

/// Optional: the path to an image file for the user's picture, through the same command the
/// app uses. Empty keeps the default portrait; a path that fails says so and keeps it too.
pub(super) async fn avatar_step(engine: &Engine, names: &Names, ask: &mut dyn Ask) {
    let Some(typed) = ask
        .line(&names.text("cli.first.avatar"))
        .map(|p| p.trim().trim_matches(['"', '\'']).to_owned())
        .filter(|p| !p.is_empty())
    else {
        return;
    };
    let Ok(bytes) = std::fs::read(&typed) else {
        return ask.say(&names.text("cli.first.avatar.unreadable"));
    };
    let Some(ext) = api::media::image_ext(&bytes) else {
        return ask.say(&names.text("cli.first.avatar.invalid"));
    };
    let image = api::views::ImagePart {
        mime: api::media::image_mime(ext).to_owned(),
        data_base64: api::media::base64_encode(&bytes),
    };
    match engine.set_user_avatar(Some(image)).await {
        Ok(_) => {}
        Err(ApiError::Invalid { .. }) => ask.say(&names.text("cli.first.avatar.invalid")),
        Err(e) => ask.say(&names.api_error(&e)),
    }
}
