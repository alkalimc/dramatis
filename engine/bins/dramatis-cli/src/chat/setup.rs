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
use api::views::EndpointSetup;
use api::{Api, Engine};

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

fn yes(ask: &mut dyn Ask, prompt: &str) -> bool {
    !matches!(
        ask.line(prompt)
            .as_deref()
            .map(str::to_lowercase)
            .as_deref(),
        None | Some("n" | "no")
    )
}

/// What the endpoint step says about a refusal.
fn endpoint_problem(e: &ApiError) -> String {
    match e {
        ApiError::Invalid { field } => match field.as_str() {
            "name" => "the profile needs a name".into(),
            "base_url" => {
                "the address must start with http:// or https:// and have no spaces".into()
            }
            "model" => "the model id is a single word (no spaces)".into(),
            "host_model" => "the host's model id is a single word (no spaces)".into(),
            other => format!("{other} is not usable"),
        },
        ApiError::Keychain { detail } => format!("the keychain refused the key: {detail}"),
        other => other.to_string(),
    }
}

/// Offer to set up the chat endpoint (or its key) and do it. Returns whether the endpoints
/// changed. Declining is fine: the program then works as a library, searching only.
pub async fn endpoints(engine: &Engine, gap: Gap, ask: &mut dyn Ask) -> bool {
    match gap {
        Gap::None => return false,
        Gap::Key(profile) => {
            ask.say(&format!(
                "The endpoint \"{profile}\" has no API key in the keychain."
            ));
            let Some(key) = ask.secret("API key (hidden, empty to skip): ") else {
                return false;
            };
            return match engine.set_key(profile, key).await {
                Ok(()) => {
                    ask.say("key stored in the keychain");
                    true
                }
                Err(e) => {
                    ask.say(&endpoint_problem(&e));
                    false
                }
            };
        }
        Gap::Endpoint => {}
    }
    ask.say(
        "No chat model is set up. Without one you can search the corpus but not talk to anyone.",
    );
    if !yes(ask, "Set one up now? [Y/n] ") {
        return false;
    }
    let presets = engine
        .endpoints()
        .await
        .map(|v| v.presets)
        .unwrap_or_default();
    if !presets.is_empty() {
        ask.say("Starting points:");
        for (i, p) in presets.iter().enumerate() {
            ask.say(&format!("  {}  {} ({})", i + 1, p.name, p.base_url));
        }
    }
    let start = ask
        .line("Number of a starting point, or Enter to type everything: ")
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| presets.get(n.checked_sub(1)?))
        .cloned();
    for _ in 0..TRIES {
        let Some(setup) = form(&start, ask) else {
            return false;
        };
        match engine.setup_endpoint(setup).await {
            Ok(()) => {
                ask.say("saved; the endpoints file is in your home directory and holds no key");
                return true;
            }
            Err(e) => ask.say(&format!(
                "{}; let us go through it again",
                endpoint_problem(&e)
            )),
        }
    }
    false
}

fn form(start: &Option<api::endpoints::Profile>, ask: &mut dyn Ask) -> Option<EndpointSetup> {
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
        "profile name",
        Some(start.as_ref().map_or("main", |p| &p.name)),
    )?;
    let base_url = with_default(
        ask,
        "address (e.g. https://api.example.com/v1)",
        start.as_ref().map(|p| p.base_url.as_str()),
    )?;
    let wire = with_default(
        ask,
        "wire: chat, responses or messages",
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
            ask.say("the wire is chat, responses or messages; using chat");
            WireApi::Chat
        }
    };
    let model = with_default(ask, "model id", None)?;
    let host_model = with_default(
        ask,
        "a different model for the host (Enter: same as above)",
        None,
    )?;
    let key = ask.secret("API key (hidden, empty to add it later): ");
    Some(EndpointSetup {
        name,
        base_url,
        wire_api,
        model,
        host_model: Some(host_model).filter(|m| !m.is_empty()),
        key,
    })
}
