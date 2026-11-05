//! Bring a save written by any earlier release to the format the app opens.
//!
//! The app opens only the latest format and carries no conversion code. This tool knows
//! every released format (1.0.0 to 1.1.1) and brings a save from any of them up to the
//! current one, then applies this release's rules for what a save holds: the chat records
//! and what was generated from them stay, prompt material and anything the app no longer
//! reads goes (see `--help` for the table).
//!
//! It works on a copy and writes a new file; the original is not touched. With
//! `--in-place` the original is kept beside it as `<file>.bak` once the new file is in
//! place. `--dry-run` runs the same conversion on a copy and prints what it did, keeping
//! nothing. Close the app and the terminal client first. An encrypted save asks for its
//! password, which is read without echo and goes nowhere else.

mod bridge;
mod convert;
mod ladder;
mod rules;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Parser)]
#[command(version, about, after_long_help = rules::POLICY)]
struct Args {
    /// The save to convert (a `.sqlite` file in the saves folder).
    save: PathBuf,
    /// Where to write the converted save. Default: `<name>.v2.sqlite` next to the original.
    #[arg(long, conflicts_with = "in_place")]
    out: Option<PathBuf>,
    /// Replace the original with the converted save, keeping it as `<file>.bak`.
    #[arg(long)]
    in_place: bool,
    /// Do everything on a copy and print what would be kept, converted and dropped;
    /// write nothing.
    #[arg(long, conflicts_with_all = ["out", "in_place"])]
    dry_run: bool,
    /// The corpus pack (`.folio`) the save was played with, to record its name and
    /// version in the save.
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
        match convert::convert(
            &args.save,
            &out,
            key.as_deref(),
            pack.as_ref(),
            args.dry_run,
        ) {
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

    if args.dry_run {
        println!("{report}");
        println!("nothing was written");
        return Ok(());
    }
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
