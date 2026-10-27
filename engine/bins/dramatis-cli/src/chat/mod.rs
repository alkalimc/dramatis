//! `chat`: talk to the corpus's persons from a terminal.
//!
//! A line-based REPL over the same agent the app uses: slash commands, anything else is a
//! message to the current channel, replies stream as they are generated. The terminal's
//! own line editing is used; a full-screen interface would add a rendering layer and
//! input handling that nothing here needs.

mod command;
mod render;
mod session;
mod show;
#[cfg(test)]
mod tests;

use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::sync::Arc;

use agent::{Agent, Config, Parts};
use anyhow::{Context, Result, anyhow};
use api::endpoints::Endpoints;
use api::home::Home;

use command::{Command, parse};
use render::{Marker, Style};
use session::{Flow, Session};
use show::{Show, Term};

#[derive(clap::Args)]
pub struct Args {
    /// Open the direct channel with this person at start (id, name or alias).
    person: Option<String>,
    /// Endpoints file. Defaults to $DRAMATIS_HOME/endpoints.toml.
    #[arg(long)]
    endpoints: Option<PathBuf>,
    /// World database to use. Defaults to $DRAMATIS_HOME/cli/world.sqlite, which the app
    /// never touches.
    #[arg(long)]
    world: Option<PathBuf>,
    /// Use a throwaway in-memory world.
    #[arg(long, conflicts_with = "world")]
    fresh: bool,
    /// The user's name in the conversation. Defaults to the corpus's own.
    #[arg(long)]
    user: Option<String>,
}

/// The local UTC offset at each call, as the app supplies it.
struct Local;

impl agent::Clock for Local {
    fn now(&self) -> world::clock::Now {
        api::port::Clock::now(&api::port::LocalClock)
    }
}

pub fn run(folio: Option<PathBuf>, args: Args) -> Result<()> {
    let home = Home::from_env();
    let folio = folio.or_else(|| home.resolve_folio()).with_context(|| {
        format!(
            "no corpus found: pass --folio <path>, set DRAMATIS_FOLIO, or put a .folio file in {}",
            home.root().display()
        )
    })?;
    let endpoints_path = args.endpoints.clone().unwrap_or_else(|| home.endpoints());
    let endpoints = std::fs::read_to_string(&endpoints_path)
        .map_err(|e| anyhow!("cannot read endpoints {}: {e}", endpoints_path.display()))
        .and_then(|t| {
            Endpoints::from_toml(&t)
                .map_err(|e| anyhow!("bad endpoints {}: {e}", endpoints_path.display()))
        })?;
    let params = api::params::Params::load(&home.params()).map_err(|e| anyhow!(e))?;
    let agent_params: agent::params::Params = std::fs::read_to_string(home.params())
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();

    let (world, world_label) = if args.fresh {
        (world::World::in_memory()?, "in memory".to_owned())
    } else {
        let path = match &args.world {
            Some(p) => p.clone(),
            None => {
                let dir = home.root().join("cli");
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
                dir.join("world.sqlite")
            }
        };
        (world::World::open(&path)?, path.display().to_string())
    };
    let index = index::Index::open(&folio, params.retrieve.clone())
        .with_context(|| format!("cannot open {}", folio.display()))?;
    let marker = Marker::new(&index.folio().manifest().scene_marker);

    let term = Arc::new(Term::new(Box::new(std::io::stdout()), Style::detect()));
    let show = Arc::new(Show::new(term.clone(), marker.clone()));
    let agent = Arc::new(Agent::new(Parts {
        world,
        index,
        config: Config {
            world: params.world.clone(),
            find_people: params.find_people.clone(),
            agent: agent_params,
            office: home.office(),
        },
        endpoints: endpoints.clone(),
        keys: None,
        llm: llm::Llm::new(Arc::new(llm::Http::new()?), llm::Retry::default()),
        sink: show.clone(),
        clock: Arc::new(Local),
    })?);
    show.bind(&agent);

    let model = |role: agent::Result<agent::endpoint::ChatRole>| match role {
        Ok(r) => format!("{}/{}", r.profile.name, r.model.id),
        Err(_) => "none".to_owned(),
    };
    println!(
        "folio {} | world {} | chat {} | host {}",
        folio.file_name().map_or_else(
            || folio.display().to_string(),
            |n| n.to_string_lossy().into()
        ),
        world_label,
        model(agent::endpoint::ChatRole::resolve(&endpoints)),
        model(agent::endpoint::ChatRole::resolve_host(&endpoints)),
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let mut session = Session::new(agent, term, show);
        if let Some(name) = args.user.as_deref() {
            session.set_user(name).await;
        }
        if let Some(person) = args.person {
            session.handle(Command::Talk(person)).await;
        }
        repl(session).await
    })
}

/// Read lines on a thread of their own, so a Ctrl-C can be heard while waiting for one.
async fn repl(mut session: Session) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
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
    let interactive = std::io::stdin().is_terminal();
    if interactive {
        println!("type /help for commands, /quit or Ctrl-D to leave");
    }
    loop {
        if interactive {
            session.prompt();
        }
        let line = tokio::select! {
            l = rx.recv() => l,
            _ = tokio::signal::ctrl_c() => None,
        };
        let Some(line) = line else {
            println!();
            return Ok(());
        };
        if session.handle(parse(&line)).await == Flow::Quit {
            return Ok(());
        }
    }
}
