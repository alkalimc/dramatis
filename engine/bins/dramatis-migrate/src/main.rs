//! Convert a save written by an earlier release (1.1.0, schema 7) to the current format.
//!
//! The app carries only general compatibility: a schema ladder, a version record in every
//! save, and a log that is dropped when it was written by another build. What is specific
//! to one old release lives here, outside the app: this tool upgrades the schema, moves the
//! rollover summaries out of the old prompt logs, strips the prompt material, and stamps
//! the save. Every message, memory, event, bond, setting and picture is kept.
//!
//! It works on a copy and writes a new file; the original is not touched. With
//! `--in-place` the original is kept beside it as `<file>.bak` once the new file is in
//! place. Close the app and the terminal client first. An encrypted save asks for its
//! password, which is read without echo and goes nowhere else.

mod convert;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// The save to convert (a `.sqlite` file in the saves folder).
    save: PathBuf,
    /// Where to write the converted save. Default: `<name>.v2.sqlite` next to the original.
    #[arg(long, conflicts_with = "in_place")]
    out: Option<PathBuf>,
    /// Replace the original with the converted save, keeping it as `<file>.bak`.
    #[arg(long)]
    in_place: bool,
    /// The corpus pack (`.folio`) the save was played with. Lets the tool carry the
    /// summaries of earlier conversations over and record the pack in the save.
    #[arg(long)]
    folio: Option<PathBuf>,
}

/// How many times the password is asked for.
const TRIES: usize = 3;

fn default_out(save: &Path) -> PathBuf {
    let stem = save.file_stem().and_then(|s| s.to_str()).unwrap_or("save");
    save.with_file_name(format!("{stem}.v2.sqlite"))
}

fn ask() -> Option<String> {
    let answer = rpassword::prompt_password("password: ").ok()?;
    (!answer.is_empty()).then_some(answer)
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !args.save.is_file() {
        bail!("{} is not a file", args.save.display());
    }
    let pack = args
        .folio
        .as_deref()
        .map(convert::Pack::from_folio)
        .transpose()
        .context("reading the corpus pack")?;
    let out = match (&args.out, args.in_place) {
        (Some(out), _) => out.clone(),
        (None, true) => convert::in_place_part(&args.save),
        (None, false) => default_out(&args.save),
    };

    let encrypted = world::cipher::is_encrypted(&args.save);
    let mut key: Option<String> = None;
    let mut tries = 0;
    let report = loop {
        if encrypted && key.is_none() {
            key = ask();
            if key.is_none() {
                bail!("the save is encrypted and no password was given");
            }
        }
        match convert::convert(&args.save, &out, key.as_deref(), pack.as_ref()) {
            Ok(report) => break report,
            Err(convert::Failure::Locked) => {
                tries += 1;
                if !encrypted || tries >= TRIES {
                    bail!("wrong password");
                }
                eprintln!("wrong password");
                key = None;
            }
            Err(convert::Failure::Other(e)) => return Err(e),
        }
    };

    let final_path = if args.in_place {
        convert::swap_in(&args.save, &out)?
    } else {
        out
    };
    println!("{report}");
    println!("written: {}", final_path.display());
    if args.in_place {
        println!(
            "the original is kept as {}",
            convert::backup_of(&args.save).display()
        );
    }
    Ok(())
}
