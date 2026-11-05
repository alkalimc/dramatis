//! The conversion itself, apart from the command line so a test can run it.
//!
//! One path for every source: copy the file, bring its layout to the current one (the
//! ladder, or the rebuild for the request family), apply this release's rules for what a
//! save holds, check the result against the original's row counts, and only then give the
//! new file its name. The original is never opened for writing.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use rusqlite::Connection;
use world::World;

use crate::bridge::{self, Converted};
use crate::ladder::{self, Family, Source};
use crate::rules::{self, Dropped};

/// Why a conversion did not finish. A wrong password is told apart so it can be asked again.
#[derive(Debug)]
pub enum Failure {
    Locked,
    Other(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Failure {
    fn from(e: E) -> Self {
        Self::Other(e.into())
    }
}

fn world_failure(e: world::Error) -> Failure {
    match e {
        world::Error::Locked => Failure::Locked,
        other => Failure::Other(anyhow!(other)),
    }
}

/// What the tool needs of the corpus pack: its name and release, for the save's record.
#[derive(Debug, Clone)]
pub struct Pack {
    pub name: String,
    pub version: String,
}

impl Pack {
    pub fn from_folio(path: &Path) -> anyhow::Result<Self> {
        let folio = folio::Folio::open(path).with_context(|| format!("{}", path.display()))?;
        let m = folio.manifest();
        Ok(Self {
            name: m.pack.clone(),
            version: m.pack_version.trim().to_owned(),
        })
    }
}

/// What a conversion kept, converted and dropped.
#[derive(Debug)]
pub struct Report {
    pub source: Source,
    /// The steps the layout needed (empty when it was current).
    pub steps: Vec<String>,
    /// Rows per table: before, after.
    pub kept: BTreeMap<String, (i64, i64)>,
    pub converted: Converted,
    pub dropped: Dropped,
    /// Openings and sessions of removed kinds (request family only).
    pub seeds_dropped: i64,
    pub sessions_dropped: i64,
    pub pack_given: bool,
    pub dry_run: bool,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.dry_run { "would" } else { "did" };
        writeln!(
            f,
            "{} {}",
            if self.dry_run {
                "dry run:"
            } else {
                "converted:"
            },
            self.source
        )?;
        if self.steps.is_empty() {
            writeln!(f, "layout: already schema {}", world::SCHEMA_VERSION)?;
        } else {
            writeln!(
                f,
                "layout -> schema {} ({verb} apply):",
                world::SCHEMA_VERSION
            )?;
            for step in &self.steps {
                writeln!(f, "  {step}")?;
            }
        }
        for (table, (before, after)) in &self.kept {
            if before == after {
                writeln!(f, "keep    {after:>7} {table}")?;
            } else {
                writeln!(f, "keep    {after:>7} {table} (of {before})")?;
            }
        }
        if self.converted.conclusions > 0 {
            writeln!(
                f,
                "convert {:>7} conclusion memories -> plain memories",
                self.converted.conclusions
            )?;
        }
        if self.converted.asks > 0 {
            writeln!(
                f,
                "convert {:>7} meter rows ask -> direct",
                self.converted.asks
            )?;
        }
        writeln!(
            f,
            "drop    {:>7} model log segments, {} entries (prompt material)",
            self.dropped.log_segments, self.dropped.log_entries
        )?;
        for (table, n) in &self.dropped.tables {
            writeln!(f, "drop    {n:>7} {table} (not in the current layout)")?;
        }
        if self.seeds_dropped + self.sessions_dropped > 0 {
            writeln!(
                f,
                "drop    {:>7} openings and {} sessions of removed kinds",
                self.seeds_dropped, self.sessions_dropped
            )?;
        }
        if !self.dropped.settings.is_empty() {
            writeln!(
                f,
                "drop    settings keys the app does not read: {}",
                self.dropped.settings.join(", ")
            )?;
        }
        if !self.dropped.meta.is_empty() {
            writeln!(
                f,
                "drop    save_meta keys: {}",
                self.dropped.meta.join(", ")
            )?;
        }
        writeln!(
            f,
            "record  app version {}{}",
            world::meta::APP_VERSION,
            if self.pack_given {
                ", pack name and version"
            } else {
                " (no --folio: the pack is recorded when the app first opens it)"
            }
        )?;
        write!(
            f,
            "size {} -> {} bytes",
            self.bytes_before, self.bytes_after
        )
    }
}

/// Tables whose row counts change on purpose, so they are not compared for equality.
const CHANGES: &[&str] = &[
    "log_segment",
    "log_entry",
    "settings",
    "save_meta",
    "seed",
    "session",
];

fn side(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn remove_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(side(path, suffix));
    }
}

/// Where the converted file is built before it takes its name.
fn part_of(dest: &Path) -> PathBuf {
    side(dest, &format!(".part-{}", std::process::id()))
}

/// The file an in-place conversion writes first, beside the original.
pub fn in_place_part(save: &Path) -> PathBuf {
    let stem = save.file_stem().and_then(|s| s.to_str()).unwrap_or("save");
    save.with_file_name(format!("{stem}.converted.sqlite"))
}

pub fn backup_of(save: &Path) -> PathBuf {
    side(save, ".bak")
}

/// Put the converted file in the original's place; the original (with the side files of
/// its journal) is kept as `<file>.bak`. Returns the path of the converted save.
pub fn swap_in(save: &Path, converted: &Path) -> anyhow::Result<PathBuf> {
    let bak = backup_of(save);
    if bak.exists() {
        bail!(
            "{} already exists; the converted save stays at {}",
            bak.display(),
            converted.display()
        );
    }
    std::fs::rename(save, &bak)?;
    // A journal left beside the original belongs to the original.
    for suffix in ["-wal", "-shm"] {
        if side(save, suffix).exists() {
            std::fs::rename(side(save, suffix), side(&bak, suffix))?;
        }
    }
    if let Err(e) = std::fs::rename(converted, save) {
        let _ = std::fs::rename(&bak, save);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::rename(side(&bak, suffix), side(save, suffix));
        }
        return Err(e).context("putting the converted save in place");
    }
    Ok(save.to_path_buf())
}

/// Copy the database file and the side files that are part of its state.
fn copy_raw(src: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::copy(src, to).with_context(|| format!("copying {}", src.display()))?;
    for suffix in ["-wal", "-shm"] {
        if side(src, suffix).is_file() {
            std::fs::copy(side(src, suffix), side(to, suffix))?;
        }
    }
    Ok(())
}

fn counts(conn: &Connection) -> rusqlite::Result<BTreeMap<String, i64>> {
    let mut out = BTreeMap::new();
    for name in rules::tables(conn)? {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |r| {
            r.get(0)
        })?;
        out.insert(name, n);
    }
    Ok(out)
}

/// What the file is: refuses what is not a world database and what a newer app wrote.
fn inspect(conn: &Connection) -> Result<Source, Failure> {
    let schema: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let quick: String = conn
        .pragma_query_value(None, "quick_check", |r| r.get(0))
        .map_err(|_| anyhow!("not a database"))?;
    if quick != "ok" {
        return Err(anyhow!("the database is damaged: {quick}").into());
    }
    let have = rules::tables(conn)?;
    if schema < 1
        || !world::CORE_TABLES
            .iter()
            .all(|t| have.iter().any(|h| h == t))
    {
        return Err(anyhow!("not a world database").into());
    }
    if schema > world::SCHEMA_VERSION {
        return Err(anyhow!(
            "the save has schema {schema}, newer than this tool knows ({}): it was made by a newer app, use a newer dramatis-migrate",
            world::SCHEMA_VERSION
        )
        .into());
    }
    Ok(Source {
        schema,
        family: ladder::family(conn)?,
    })
}

fn run(
    src: &Path,
    part: &Path,
    key: Option<&str>,
    pack: Option<&Pack>,
    dry_run: bool,
) -> Result<Report, Failure> {
    copy_raw(src, part)?;
    let mut conn = world::cipher::connect_existing(part, key).map_err(world_failure)?;
    // One file at rest, as the app keeps it; a leftover write-ahead log is folded in.
    let _ = conn.pragma_update_and_check(None, "journal_mode", "DELETE", |_| Ok(()));
    let source = inspect(&conn)?;
    let before = counts(&conn)?;
    let steps = ladder::pending(&source);
    let mut report = Report {
        source,
        steps: if source.family == Family::Ladder && source.schema == world::SCHEMA_VERSION {
            Vec::new()
        } else {
            steps
        },
        kept: BTreeMap::new(),
        converted: Converted::default(),
        dropped: Dropped::default(),
        seeds_dropped: 0,
        sessions_dropped: 0,
        pack_given: pack.is_some(),
        dry_run,
        bytes_before: std::fs::metadata(src)?.len(),
        bytes_after: 0,
    };

    match source.family {
        Family::Ladder => {
            conn.pragma_update(None, "foreign_keys", true)?;
            ladder::climb(&mut conn, source.schema)?;
        }
        Family::Request => report.converted = bridge::rebuild(&mut conn)?,
    }
    conn.pragma_update(None, "foreign_keys", true)?;
    report.dropped = rules::apply(&conn)?;
    world::meta::write(
        &conn,
        world::meta::APP_VERSION,
        pack.map(|p| (p.name.as_str(), p.version.as_str())),
    )
    .map_err(world_failure)?;

    // Every row the player's content lives in came across; only what was meant to go went.
    let after = counts(&conn)?;
    for (table, &was) in &before {
        let Some(&now) = after.get(table) else {
            continue;
        };
        let expected = match table.as_str() {
            "settings" => was - report.dropped.settings.len() as i64,
            "seed" | "session" if source.family == Family::Request => {
                if table == "seed" {
                    report.seeds_dropped = was - now
                } else {
                    report.sessions_dropped = was - now
                }
                now
            }
            "seed" | "session" => was,
            t if CHANGES.contains(&t) => continue,
            _ => was,
        };
        if now != expected {
            return Err(
                anyhow!("{table} changed size during the conversion: {was} -> {now}").into(),
            );
        }
    }
    for (table, &now) in &after {
        let was = before.get(table).copied().unwrap_or(0);
        if !CHANGES.contains(&table.as_str())
            || table == "settings"
            || table == "seed"
            || table == "session"
        {
            report.kept.insert(table.clone(), (was, now));
        }
    }
    let broken: i64 = conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
        r.get(0)
    })?;
    if broken > 0 {
        return Err(
            anyhow!("{broken} rows point at rows that are not there after the conversion").into(),
        );
    }
    conn.execute_batch("VACUUM")?;
    let quick: String = conn.pragma_query_value(None, "quick_check", |r| r.get(0))?;
    if quick != "ok" {
        return Err(anyhow!("the converted save failed its integrity check: {quick}").into());
    }
    drop(conn);
    // Opened again as the app will, and found as it should be.
    world::check_file(part, key).map_err(world_failure)?;
    let w = World::open_with(part, key).map_err(world_failure)?;
    world::meta::probe(&w).map_err(world_failure)?;
    drop(w);
    report.bytes_after = std::fs::metadata(part)?.len();
    Ok(report)
}

/// Convert the save at `src` into a new file `dest`, under the same password. `src` is only
/// read (from a copy); nothing is written at `dest` unless the conversion succeeded. With
/// `dry_run` the whole conversion runs on the copy and is reported, and nothing is kept.
pub fn convert(
    src: &Path,
    dest: &Path,
    key: Option<&str>,
    pack: Option<&Pack>,
    dry_run: bool,
) -> Result<Report, Failure> {
    if !dry_run && dest.exists() {
        return Err(anyhow!("{} already exists", dest.display()).into());
    }
    let part = part_of(dest);
    remove_files(&part);
    let done = run(src, &part, key, pack, dry_run);
    let done = done.and_then(|report| {
        if !dry_run {
            std::fs::rename(&part, dest)?;
        }
        Ok(report)
    });
    remove_files(&part);
    done
}
