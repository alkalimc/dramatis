//! Saves on disk: `$DRAMATIS_HOME/saves/<name>.sqlite`, one world each. Everything here
//! works on files and knows nothing of which save a process has open; the engine adds
//! that rule (the open save is not deleted, and a switch needs the agent idle).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use world::clock::Now;

use crate::convert::{self, world_err};
use crate::error::{ApiError, ApiResult};
use crate::home::Home;
use crate::views::SaveInfo;

/// Longest save name, in characters.
const NAME_MAX: usize = 48;

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io {
        detail: e.to_string(),
    }
}

fn invalid(field: &str) -> ApiError {
    ApiError::Invalid {
        field: field.into(),
    }
}

/// A name that is safe as a file name on every platform and reads as a name: no path
/// separators, no leading dot or edge spaces, no control characters.
pub fn valid_name(name: &str) -> bool {
    let n = name.chars().count();
    (1..=NAME_MAX).contains(&n)
        && name == name.trim()
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

fn check_name(name: &str) -> ApiResult<()> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(invalid("name"))
    }
}

/// The save called `name`, which must exist.
pub fn path_of(home: &Home, name: &str) -> ApiResult<PathBuf> {
    check_name(name)?;
    let path = home.save_path(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Whether a save of this name exists, ignoring case: two saves differing only by case
/// would be one on a case-insensitive file system.
fn taken(home: &Home, name: &str) -> bool {
    let want = name.to_lowercase();
    names(home).iter().any(|n| n.to_lowercase() == want)
}

fn names(home: &Home) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home.saves()) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".sqlite")?;
            (valid_name(stem) && e.path().is_file()).then(|| stem.to_owned())
        })
        .collect()
}

fn ms(t: SystemTime) -> Option<i64> {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

/// Every save, the most recently played first. A write-ahead log newer than the file
/// counts: that is where play lands until a checkpoint.
pub fn list(home: &Home, current: &str, now: Now) -> Vec<SaveInfo> {
    let mut rows: Vec<(Option<i64>, SaveInfo)> = names(home)
        .into_iter()
        .map(|name| {
            let path = home.save_path(&name);
            let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let wal = PathBuf::from(format!("{}-wal", path.display()));
            let at = [modified(&path), modified(&wal)]
                .into_iter()
                .flatten()
                .max()
                .and_then(ms);
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            let info = SaveInfo {
                current: name == current,
                name,
                size_kb: u32::try_from(size.div_ceil(1024)).unwrap_or(u32::MAX),
                played_at: at.map(|a| convert::ts(a, now.offset_min)),
            };
            (at, info)
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    rows.into_iter().map(|(_, info)| info).collect()
}

/// A path next to `final_path` to build a file in before it takes its name, so a save
/// never exists half-written.
fn staging(final_path: &Path) -> PathBuf {
    PathBuf::from(format!(
        "{}.part-{}",
        final_path.display(),
        std::process::id()
    ))
}

/// Remove a database file and the side files SQLite keeps beside it.
fn remove_files(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

/// An empty save.
pub fn create(home: &Home, name: &str) -> ApiResult<()> {
    check_name(name)?;
    if taken(home, name) {
        return Err(ApiError::Exists);
    }
    let path = home.save_path(name);
    let part = staging(&path);
    let built = world::World::open(&part).map(drop).map_err(world_err);
    if built.is_ok() {
        std::fs::rename(&part, &path).map_err(io)?;
    }
    remove_files(&part);
    built
}

/// A consistent copy of `from` as a new save `to`.
pub fn copy(home: &Home, from: &str, to: &str) -> ApiResult<()> {
    let src = path_of(home, from)?;
    check_name(to)?;
    if taken(home, to) {
        return Err(ApiError::Exists);
    }
    let dest = home.save_path(to);
    let part = staging(&dest);
    let copied = world::copy_file(&src, &part).map_err(world_err);
    if copied.is_ok() {
        std::fs::rename(&part, &dest).map_err(io)?;
    }
    remove_files(&part);
    copied
}

/// Give a save another name (its files move together).
pub fn rename(home: &Home, from: &str, to: &str) -> ApiResult<()> {
    let src = path_of(home, from)?;
    check_name(to)?;
    // Changing only the case of a name is allowed: it is the same save.
    if from != to && taken(home, to) && from.to_lowercase() != to.to_lowercase() {
        return Err(ApiError::Exists);
    }
    let dest = home.save_path(to);
    std::fs::rename(&src, &dest).map_err(io)?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", src.display()));
        if side.exists() {
            std::fs::rename(&side, format!("{}{suffix}", dest.display())).map_err(io)?;
        }
    }
    Ok(())
}

/// Delete a save for good.
pub fn delete(home: &Home, name: &str) -> ApiResult<()> {
    remove_files(&path_of(home, name)?);
    Ok(())
}

/// Write a consistent single-file copy of the save to `dest`, replacing a file there.
pub fn export(home: &Home, name: &str, dest: &Path) -> ApiResult<()> {
    let src = path_of(home, name)?;
    if dest.is_dir() || dest.file_name().is_none() {
        return Err(invalid("path"));
    }
    let part = staging(dest);
    let _ = std::fs::remove_file(&part);
    let copied = world::copy_file(&src, &part).map_err(world_err);
    if copied.is_ok() {
        std::fs::rename(&part, dest).map_err(io)?;
    }
    remove_files(&part);
    copied
}

/// Bring a world database from `file` in as a new save, migrated to this build's schema.
/// `name` defaults to the file's name without its extension. Returns the name used.
pub fn import(home: &Home, file: &Path, name: Option<&str>) -> ApiResult<String> {
    let name = match name {
        Some(n) => n.to_owned(),
        None => file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned(),
    };
    check_name(&name)?;
    if taken(home, &name) {
        return Err(ApiError::Exists);
    }
    if !file.is_file() {
        return Err(ApiError::NotFound);
    }
    world::check_file(file).map_err(|e| match e {
        world::Error::Invalid(_) | world::Error::TooNew { .. } => invalid("file"),
        other => world_err(other),
    })?;
    let dest = home.save_path(&name);
    let part = staging(&dest);
    let built = world::copy_file(file, &part)
        .and_then(|()| world::World::open(&part).map(drop))
        .map_err(world_err);
    if built.is_ok() {
        std::fs::rename(&part, &dest).map_err(io)?;
    }
    remove_files(&part);
    built.map(|()| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> Home {
        let root =
            std::env::temp_dir().join(format!("dramatis-saves-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root);
        home.create().unwrap();
        create(&home, "default").unwrap();
        home
    }

    fn done(home: &Home) {
        std::fs::remove_dir_all(home.root()).unwrap();
    }

    fn marker(home: &Home, name: &str, value: &str) {
        let w = world::World::open(home.save_path(name)).unwrap();
        w.execute(
            "INSERT OR REPLACE INTO settings VALUES ('user.name', ?1)",
            [format!("\"{value}\"")],
        )
        .unwrap();
    }

    fn marked(home: &Home, name: &str) -> Option<String> {
        let w = world::World::open(home.save_path(name)).unwrap();
        w.query_row(
            "SELECT value FROM settings WHERE key = 'user.name'",
            [],
            |r| r.get(0),
        )
        .ok()
    }

    fn at() -> Now {
        Now {
            ms: 0,
            offset_min: 0,
        }
    }

    #[test]
    fn names_are_file_safe() {
        for ok in ["default", "story 2", "café", "a.b", "x-y_z"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            " a",
            "a ",
            ".hidden",
            "a/b",
            "a\\b",
            "a:b",
            "a\nb",
            &"x".repeat(49),
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn create_copy_rename_delete() {
        let h = home("ops");
        create(&h, "one").unwrap();
        assert_eq!(create(&h, "ONE"), Err(ApiError::Exists));
        assert_eq!(create(&h, "../x"), Err(invalid("name")));
        marker(&h, "one", "Doc");
        copy(&h, "one", "two").unwrap();
        assert_eq!(copy(&h, "one", "two"), Err(ApiError::Exists));
        assert_eq!(copy(&h, "nope", "three"), Err(ApiError::NotFound));
        assert_eq!(marked(&h, "two").as_deref(), Some("\"Doc\""));
        // The copy is its own world.
        marker(&h, "two", "Other");
        assert_eq!(marked(&h, "one").as_deref(), Some("\"Doc\""));

        rename(&h, "two", "second").unwrap();
        assert_eq!(rename(&h, "one", "second"), Err(ApiError::Exists));
        assert_eq!(path_of(&h, "two"), Err(ApiError::NotFound));
        assert_eq!(marked(&h, "second").as_deref(), Some("\"Other\""));
        rename(&h, "one", "One").unwrap();

        let all: Vec<_> = list(&h, "One", at())
            .into_iter()
            .map(|s| (s.name, s.current))
            .collect();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&("One".into(), true)));
        assert!(all.contains(&("default".into(), false)));

        delete(&h, "second").unwrap();
        assert_eq!(delete(&h, "second"), Err(ApiError::NotFound));
        assert_eq!(list(&h, "One", at()).len(), 2);
        done(&h);
    }

    #[test]
    fn export_replaces_and_import_validates_and_migrates() {
        let h = home("io");
        create(&h, "one").unwrap();
        marker(&h, "one", "Doc");
        let out = h.root().join("out.sqlite");
        std::fs::write(&out, b"old").unwrap();
        export(&h, "one", &out).unwrap();
        assert!(!h.root().join("out.sqlite.part").exists());
        assert_eq!(export(&h, "one", h.root()), Err(invalid("path")));

        assert_eq!(import(&h, &out, None).unwrap(), "out");
        assert_eq!(marked(&h, "out").as_deref(), Some("\"Doc\""));
        assert_eq!(import(&h, &out, Some("out")), Err(ApiError::Exists));
        assert_eq!(import(&h, &out, Some("renamed")).unwrap(), "renamed");

        let junk = h.root().join("junk.sqlite");
        std::fs::write(&junk, b"not a database").unwrap();
        assert_eq!(import(&h, &junk, None), Err(invalid("file")));
        assert_eq!(
            import(&h, &h.root().join("absent.db"), Some("x")),
            Err(ApiError::NotFound)
        );
        // Nothing half-built is left behind.
        let left: Vec<_> = std::fs::read_dir(h.saves())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("part"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        done(&h);
    }
}
