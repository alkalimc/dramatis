//! `chat`: talk to the corpus's persons from a terminal.
//!
//! A line-based REPL over the same engine, ports and world file the app uses, through the
//! same seam commands and events: slash commands, anything else is a message to the
//! current channel, replies stream as they are generated. On a terminal the input is edited
//! by rustyline, which measures wide (CJK) characters correctly: Enter starts a new line
//! and Alt-Enter sends (a slash command runs on Enter), and Tab completes command names,
//! persons and save names. Piped input, and
//! `--plain`, is read as plain lines, one message each.

mod command;
mod complete;
mod interrupt;
mod render;
mod secret;
mod session;
mod show;
#[cfg(test)]
mod tests;
mod text;

use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use api::Api;
use api::home::Home;
use runtime::{Options, Runtime};

use command::Command;
use complete::Completions;
use interrupt::Interrupt;
use render::{Marker, Style};
use session::{Flow, Session};
use show::{Names, Show, Term};

#[derive(clap::Args)]
pub struct Args {
    /// Open the direct channel with this person at start (id, name or alias).
    person: Option<String>,
    /// Home directory: the world database, endpoints file and parameters. Defaults to
    /// $DRAMATIS_HOME (else ~/.dramatis). It is safe to have the app open on the same
    /// directory at once (WAL, busy timeout).
    #[arg(long)]
    home: Option<PathBuf>,
    /// Use a throwaway world: a temporary home that keeps the real home's endpoints and
    /// parameters and is deleted at exit.
    #[arg(long, conflicts_with = "home")]
    fresh: bool,
    /// Read plain lines even on a terminal: no line editing, no prompt.
    #[arg(long)]
    plain: bool,
    /// The user's name in the conversation. Defaults to the corpus's own.
    #[arg(long)]
    user: Option<String>,
}

/// A temporary home for `--fresh`, removed when dropped.
struct Throwaway(PathBuf);

impl Throwaway {
    /// A new directory holding copies of the real home's endpoints and parameters.
    fn new(real: &Home) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("dramatis-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        for from in [real.endpoints(), real.params()] {
            if let Some(name) = from.file_name() {
                let _ = std::fs::copy(&from, dir.join(name));
            }
        }
        Ok(Self(dir))
    }
}

impl Drop for Throwaway {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run(folio: Option<PathBuf>, args: Args) -> Result<()> {
    let mut home = args.home.as_deref().map_or_else(Home::from_env, Home::new);
    let throwaway = if args.fresh {
        let t = Throwaway::new(&home)?;
        home = Home::new(&t.0);
        Some(t)
    } else {
        None
    };
    // An encrypted save is unlocked before anything opens it.
    let password = if args.fresh {
        None
    } else {
        start_password(&home)?
    };
    let folio = folio.or_else(|| home.resolve_folio()).with_context(|| {
        format!(
            "no corpus found: pass --folio <path>, set DRAMATIS_FOLIO, or put a .folio file in {}",
            home.root().display()
        )
    })?;

    let term = Arc::new(Term::new(Box::new(std::io::stdout()), Style::detect()));
    let show = Arc::new(Show::new(term.clone()));
    let tokio = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let leftover = throwaway.as_ref().map(|t| t.0.clone());
    tokio.block_on(async move {
        let interrupt = Interrupt::install(leftover)?;
        let options = Options {
            quiet: true,
            frontend: runtime::Frontend::Terminal,
            password,
            ..Options::default()
        };
        let runtime = Runtime::open(home.clone(), Some(folio.clone()), show.clone(), options)
            .map_err(|e| anyhow::anyhow!("cannot open {}: {e}", folio.display()))?;
        let (engine, bridge) = (runtime.engine, runtime.bridge);
        let Some(bridge) = bridge.filter(|_| engine.corpus_handle().is_some()) else {
            bail!("cannot read {} as a corpus", folio.display());
        };
        let names = Arc::new(
            Names::load(&engine)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        );
        let scene = engine
            .wording()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .scene_marker;
        show.bind(names.clone(), Marker::new(&(scene.open, scene.close)));
        println!("{}", header(&engine, &folio, &home, args.fresh).await);
        let completions = Completions::default();
        let typing = Typing {
            tip: names.text("cli.typing.more"),
            style: term.style,
            completions: completions.clone(),
        };
        let intro = intro_lines(&names);
        let mut session = Session::new(
            engine,
            bridge,
            term,
            show,
            names,
            completions,
            interrupt.clone(),
        )
        .await;
        if let Some(name) = args.user.as_deref() {
            session.set_user(name).await;
        }
        if let Some(person) = args.person {
            session.handle(Command::Talk(person)).await;
        }
        repl(session, args.plain, typing, intro, interrupt).await
    })
}

/// The password of the save the process starts on, asked for (hidden, three tries) when
/// that save is encrypted.
fn start_password(home: &Home) -> Result<Option<String>> {
    let path = home.world();
    if !world::cipher::is_encrypted(&path) {
        return Ok(None);
    }
    for _ in 0..3 {
        let prompt = format!("password for {}: ", home.current_save());
        let Some(password) = secret::read(&prompt) else {
            bail!("the save is encrypted and no password was given");
        };
        match world::check_file(&path, Some(&password)) {
            Ok(()) => return Ok(Some(password)),
            Err(world::Error::Locked) => eprintln!("wrong password"),
            Err(e) => bail!("cannot open {}: {e}", path.display()),
        }
    }
    bail!("wrong password three times")
}

/// The line printed at start: which corpus, which world, and the models in use.
async fn header(engine: &api::Engine, folio: &Path, home: &Home, fresh: bool) -> String {
    use api::Api;
    let model = |role: Option<&api::endpoints::Role>| {
        role.map_or_else(
            || "none".to_owned(),
            |r| format!("{}/{}", r.profile, r.model),
        )
    };
    let roles = engine.endpoints().await.ok().and_then(|e| e.roles);
    let chat = roles.as_ref().map(|r| &r.chat);
    let host = roles.as_ref().and_then(|r| r.host.as_ref()).or(chat);
    format!(
        "folio {} | world {} | chat {} | host {}",
        folio.file_name().map_or_else(
            || folio.display().to_string(),
            |n| n.to_string_lossy().into()
        ),
        if fresh {
            "throwaway".to_owned()
        } else {
            home.world().display().to_string()
        },
        model(chat),
        model(host),
    )
}

/// Enter in the editor: a slash command runs at once, anything else gets a new line, so a
/// message can span lines. Sending a message is Alt-Enter.
struct EnterKey;

impl rustyline::ConditionalEventHandler for EnterKey {
    fn handle(
        &self,
        _: &rustyline::Event,
        _: rustyline::RepeatCount,
        _: bool,
        ctx: &rustyline::EventContext,
    ) -> Option<rustyline::Cmd> {
        Some(if is_command_line(ctx.line()) {
            rustyline::Cmd::AcceptLine
        } else {
            rustyline::Cmd::Newline
        })
    }
}

/// A single line that starts with a slash: a command (or `//text`), run on Enter.
fn is_command_line(line: &str) -> bool {
    line.starts_with('/') && !line.contains('\n')
}

/// What the editor shows after a draft that has run onto a second line, since Enter no
/// longer sends: the key that does.
#[derive(Clone)]
struct Typing {
    tip: String,
    style: Style,
    completions: Completions,
}

/// A hint is display only: the right arrow must not paste it into the draft.
struct Tip(String);

impl rustyline::hint::Hint for Tip {
    fn display(&self) -> &str {
        &self.0
    }
    fn completion(&self) -> Option<&str> {
        None
    }
}

impl rustyline::hint::Hinter for Typing {
    type Hint = Tip;
    fn hint(&self, line: &str, pos: usize, _: &rustyline::Context<'_>) -> Option<Tip> {
        (pos == line.len() && line.contains('\n')).then(|| Tip(format!("  ({})", self.tip)))
    }
}

impl rustyline::highlight::Highlighter for Typing {
    fn highlight_hint<'h>(&self, hint: &'h str) -> std::borrow::Cow<'h, str> {
        self.style.dim(hint).into()
    }
}

impl rustyline::completion::Completer for Typing {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        Ok(self.completions.complete(line, pos))
    }
}
impl rustyline::validate::Validator for Typing {}
impl rustyline::Helper for Typing {}

type Editor = rustyline::Editor<Typing, rustyline::history::DefaultHistory>;

/// The line editor with the key scheme: Enter newline, Alt-Enter send.
fn editor(typing: &Typing) -> rustyline::Result<Editor> {
    use rustyline::{Cmd, EventHandler, KeyCode, KeyEvent, Modifiers};
    // A list, not a cycle: with several matches the user sees what they are.
    let config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::List)
        .build();
    let mut editor = Editor::with_config(config)?;
    editor.set_helper(Some(typing.clone()));
    editor.bind_sequence(
        KeyEvent(KeyCode::Enter, Modifiers::NONE),
        EventHandler::Conditional(Box::new(EnterKey)),
    );
    editor.bind_sequence(KeyEvent(KeyCode::Enter, Modifiers::ALT), Cmd::AcceptLine);
    // Ctrl-J is the same byte as a line feed: it must not send either.
    editor.bind_sequence(KeyEvent::ctrl('J'), Cmd::Newline);
    Ok(editor)
}

/// Where the next line comes from.
enum Input {
    /// Line editing on a terminal; the editor travels to a blocking thread and back.
    Edit(Box<Editor>, Typing),
    /// Lines of a pipe or file, read on a thread so a Ctrl-C can be heard while waiting.
    Lines(tokio::sync::mpsc::UnboundedReceiver<String>),
}

impl Input {
    fn open(plain: bool, typing: Typing) -> Result<Self> {
        if !plain && std::io::stdin().is_terminal() {
            return Ok(Self::Edit(Box::new(editor(&typing)?), typing));
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self::Lines(rx))
    }

    /// The next line, or `None` at the end of input. `Some("")` for an abandoned line.
    /// `initial` is text the line starts with, for the user to edit.
    async fn next(
        &mut self,
        prompt: String,
        initial: Option<String>,
        interrupt: &Interrupt,
    ) -> Option<String> {
        match self {
            Self::Lines(rx) => tokio::select! {
                l = rx.recv() => l,
                _ = interrupt.wait() => None,
            },
            Self::Edit(slot, typing) => {
                use rustyline::error::ReadlineError;
                let mut editor = std::mem::replace(slot, Box::new(editor(typing).ok()?));
                let (editor, line) = tokio::task::spawn_blocking(move || {
                    let line = match &initial {
                        Some(text) => editor.readline_with_initial(&prompt, (text, "")),
                        None => editor.readline(&prompt),
                    };
                    if let Ok(l) = &line
                        && !l.trim().is_empty()
                    {
                        let _ = editor.add_history_entry(l.as_str());
                    }
                    (editor, line)
                })
                .await
                .ok()?;
                *slot = editor;
                match line {
                    Ok(l) => Some(l),
                    Err(ReadlineError::Interrupted) => Some(String::new()),
                    Err(_) => None,
                }
            }
        }
    }
}

/// The lines printed when an interactive chat starts: how to send, and how to leave.
fn intro_lines(names: &Names) -> [String; 2] {
    [names.text("cli.typing"), names.text("cli.start")]
}

async fn repl(
    mut session: Session,
    plain: bool,
    typing: Typing,
    intro: [String; 2],
    interrupt: Arc<Interrupt>,
) -> Result<()> {
    let mut input = Input::open(plain, typing)?;
    let interactive = matches!(input, Input::Edit(..));
    if interactive {
        for line in intro {
            println!("{line}");
        }
    }
    // Anyone due to come by does so now, as the app would on opening.
    session.arrivals(true).await;
    loop {
        let prompt = if interactive {
            session.prompt()
        } else {
            String::new()
        };
        if interactive {
            interrupt.clear();
        }
        let initial = session.take_prefill();
        if !interactive && let Some(text) = &initial {
            // No editor to put it in: show it, to be typed again.
            println!("{text}");
        }
        let Some(line) = input.next(prompt, initial, &interrupt).await else {
            println!();
            return Ok(());
        };
        if session.input(&line).await == Flow::Quit {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod keys {
    use super::is_command_line;

    #[test]
    fn enter_runs_a_slash_command_and_otherwise_starts_a_new_line() {
        assert!(is_command_line("/help"));
        assert!(is_command_line("//literal slash"));
        assert!(!is_command_line("hello"));
        assert!(!is_command_line(""));
        // Once a message spans lines, a slash on the first line no longer makes it a command.
        assert!(!is_command_line("/not a command\nsecond line"));
    }
}
