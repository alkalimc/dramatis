//! `chat`: talk to the corpus's persons from a terminal.
//!
//! A line-based REPL over the same engine, ports and world file the app uses, through the
//! same seam commands and events: slash commands, anything else is a message to the
//! current channel, replies stream as they are generated. On a terminal the line is edited by rustyline, which measures wide (CJK)
//! characters correctly; piped input is read as plain lines.

mod command;
mod render;
mod session;
mod show;
#[cfg(test)]
mod tests;

use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use api::Api;
use api::home::Home;
use runtime::{Options, Runtime};

use command::{Command, parse};
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
    let _throwaway = if args.fresh {
        let t = Throwaway::new(&home)?;
        home = Home::new(&t.0);
        Some(t)
    } else {
        None
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
    tokio.block_on(async move {
        let options = Options {
            quiet: true,
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
        let mut session = Session::new(engine, bridge, term, show, names).await;
        if let Some(name) = args.user.as_deref() {
            session.set_user(name).await;
        }
        if let Some(person) = args.person {
            session.handle(Command::Talk(person)).await;
        }
        repl(session, args.plain).await
    })
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

/// Where the next line comes from.
enum Input {
    /// Line editing on a terminal; the editor travels to a blocking thread and back.
    Edit(Box<rustyline::DefaultEditor>),
    /// Lines of a pipe or file, read on a thread so a Ctrl-C can be heard while waiting.
    Lines(tokio::sync::mpsc::UnboundedReceiver<String>),
}

impl Input {
    fn open(plain: bool) -> Result<Self> {
        if !plain && std::io::stdin().is_terminal() {
            return Ok(Self::Edit(Box::new(rustyline::DefaultEditor::new()?)));
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
    async fn next(&mut self, prompt: String) -> Option<String> {
        match self {
            Self::Lines(rx) => tokio::select! {
                l = rx.recv() => l,
                _ = tokio::signal::ctrl_c() => None,
            },
            Self::Edit(slot) => {
                use rustyline::error::ReadlineError;
                let mut editor =
                    std::mem::replace(slot, Box::new(rustyline::DefaultEditor::new().ok()?));
                let (editor, line) = tokio::task::spawn_blocking(move || {
                    let line = editor.readline(&prompt);
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

async fn repl(mut session: Session, plain: bool) -> Result<()> {
    let mut input = Input::open(plain)?;
    let interactive = matches!(input, Input::Edit(_));
    if interactive {
        println!("type /help for commands, /quit or Ctrl-D to leave");
    }
    loop {
        let prompt = if interactive {
            session.prompt()
        } else {
            String::new()
        };
        let Some(line) = input.next(prompt).await else {
            println!();
            return Ok(());
        };
        if session.handle(parse(&line)).await == Flow::Quit {
            return Ok(());
        }
    }
}
