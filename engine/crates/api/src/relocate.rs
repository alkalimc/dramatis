//! Moving the home directory: checking the new folder and copying what is kept into it.
//! The old folder is only read. Caches (`media/`) and diagnostics (`logs/`) are not carried
//! over: they are made again.

use std::path::{Path, PathBuf};

use crate::error::{ApiError, ApiResult};
use crate::home::{Home, expand_tilde};
use crate::saves;

fn io(e: impl std::fmt::Display) -> ApiError {
    ApiError::Io {
        detail: e.to_string(),
    }
}

fn invalid() -> ApiError {
    ApiError::Invalid {
        field: "path".into(),
    }
}

/// What stays behind: caches, diagnostics, and the files a running process keeps beside
/// the data (locks, half-written files). Saves are copied by their own rule.
fn skipped(name: &str) -> bool {
    matches!(name, "media" | "logs" | "saves")
        || [".lock", ".tmp", ".part"].iter().any(|s| name.ends_with(s))
}

/// The folder `typed` names, made if it is not there. It must be an absolute path (after
/// `~/`), not the folder in use, and not inside it.
pub fn check_target(from: &Home, typed: &str) -> ApiResult<PathBuf> {
    let typed = typed.trim();
    if typed.is_empty() || typed.chars().any(char::is_control) {
        return Err(invalid());
    }
    let path = expand_tilde(typed);
    // Judged by the words first, so that nothing is made inside the folder in use.
    if !path.is_absolute() || path.starts_with(from.root()) {
        return Err(invalid());
    }
    std::fs::create_dir_all(&path).map_err(|_| invalid())?;
    // A folder that cannot be written is no home.
    let probe = path.join(format!(".dramatis-write-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|_| invalid())?;
    let _ = std::fs::remove_file(&probe);
    let new = std::fs::canonicalize(&path).map_err(|_| invalid())?;
    let old = std::fs::canonicalize(from.root()).unwrap_or_else(|_| from.root().to_owned());
    if new.starts_with(&old) {
        return Err(invalid());
    }
    Ok(path)
}

/// Whether `to` already holds something `copy_home` would write: a top-level name of the
/// home, or any save when there are saves to bring.
fn clashes(from: &Home, to: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(from.root()) else {
        return false;
    };
    let named = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !skipped(n))
        .any(|n| to.join(n).exists());
    named || (!saves::all_names(from).is_empty() && !saves::all_names(&Home::new(to)).is_empty())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Copy what the home keeps into `to`: saves (each a consistent copy, under its own
/// password from `key_of`), then everything else at the top level. `Exists` when `to`
/// already holds any of it. A failure takes back what this call wrote there.
pub fn copy_home(from: &Home, to: &Path, key_of: &dyn Fn(&str) -> Option<String>) -> ApiResult<()> {
    if clashes(from, to) {
        return Err(ApiError::Exists);
    }
    let dest = Home::new(to);
    let mut written: Vec<PathBuf> = Vec::new();
    let result = (|| -> ApiResult<()> {
        for name in saves::all_names(from) {
            saves::copy_into(from, &name, &dest, key_of(&name).as_deref())?;
            written.push(dest.save_path(&name));
        }
        for entry in std::fs::read_dir(from.root()).map_err(io)? {
            let entry = entry.map_err(io)?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if skipped(&name) {
                continue;
            }
            let target = to.join(&name);
            written.push(target.clone());
            let kind = entry.file_type().map_err(io)?;
            if kind.is_dir() {
                copy_tree(&entry.path(), &target).map_err(io)?;
            } else if kind.is_file() {
                std::fs::copy(entry.path(), &target).map_err(io)?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        // Only paths this call created, none of which existed before (see `clashes`).
        for path in written {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
        // The folder of saves goes only if it is empty again.
        let _ = std::fs::remove_dir(dest.saves());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dramatis-move-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn kept(root: &Path) -> Home {
        let home = Home::new(root);
        home.create().unwrap();
        saves::create(&home, "default").unwrap();
        std::fs::write(root.join("endpoints.toml"), "x").unwrap();
        std::fs::write(root.join("a.folio"), "corpus").unwrap();
        std::fs::write(root.join("uploads/pic"), "p").unwrap();
        std::fs::write(root.join("media/avatar/cached"), "c").unwrap();
        std::fs::write(root.join("logs/turn.log"), "l").unwrap();
        std::fs::write(root.join("current_save.1.tmp"), "").unwrap();
        home
    }

    #[test]
    fn copying_takes_what_is_kept_and_leaves_the_old_folder_whole() {
        let base = scratch("copy");
        let old = base.join("old");
        let new = base.join("new");
        let home = kept(&old);
        let target = check_target(&home, new.to_str().unwrap()).unwrap();
        copy_home(&home, &target, &|_| None).unwrap();
        assert!(new.join("saves/default.sqlite").is_file());
        assert_eq!(
            std::fs::read_to_string(new.join("a.folio")).unwrap(),
            "corpus"
        );
        assert!(new.join("uploads/pic").is_file() && new.join("endpoints.toml").is_file());
        // Caches, logs and half-written files stay behind.
        assert!(!new.join("media").exists() && !new.join("logs").exists());
        assert!(!new.join("current_save.1.tmp").exists());
        // The old folder lost nothing.
        assert!(old.join("saves/default.sqlite").is_file());
        assert!(old.join("media/avatar/cached").is_file() && old.join("endpoints.toml").is_file());
        // A second copy would overwrite: refused, and the first is untouched.
        assert_eq!(copy_home(&home, &new, &|_| None), Err(ApiError::Exists));
        assert!(new.join("saves/default.sqlite").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_failed_copy_takes_back_only_what_it_wrote() {
        let base = scratch("fail");
        let old = base.join("old");
        let new = base.join("new");
        let home = kept(&old);
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("mine.txt"), "keep").unwrap();
        // A save that is not a database fails the copy, after others may have been copied.
        std::fs::write(home.save_path("zzz"), b"not a database").unwrap();
        let r = copy_home(&home, &new, &|_| None);
        assert!(r.is_err());
        assert!(new.join("mine.txt").is_file());
        assert!(!new.join("saves").exists() && !new.join("endpoints.toml").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_target_must_be_a_folder_apart_from_the_old_one() {
        let base = scratch("target");
        let home = kept(&base.join("old"));
        let bad = |p: &str| check_target(&home, p);
        assert!(bad("").is_err() && bad("relative/dir").is_err());
        assert!(bad(base.join("old").to_str().unwrap()).is_err());
        assert!(bad(base.join("old/inside").to_str().unwrap()).is_err());
        // A file is no folder.
        std::fs::write(base.join("file"), "").unwrap();
        assert!(bad(base.join("file").to_str().unwrap()).is_err());
        assert!(bad(base.join("fresh/deeper").to_str().unwrap()).is_ok());
        assert!(base.join("fresh/deeper").is_dir());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_pointer_decides_the_next_start_and_a_missing_folder_is_reported() {
        let base = scratch("pointer");
        let config = base.join("config");
        let chosen = base.join("chosen");
        std::fs::create_dir_all(&chosen).unwrap();
        let first = Home::locate(Some(config.clone()));
        assert!(first.can_relocate());
        assert_eq!(first.source(), crate::home::Source::Default);
        first.choose_root(Some(&chosen)).unwrap();
        let next = Home::locate(Some(config.clone()));
        assert_eq!(next.root(), chosen);
        assert_eq!(next.source(), crate::home::Source::Chosen);
        // The folder goes away (an unplugged drive): the default is used and the loss is said.
        std::fs::remove_dir_all(&chosen).unwrap();
        let gone = Home::locate(Some(config.clone()));
        assert_eq!(gone.root(), crate::home::default_root());
        assert_eq!(gone.unavailable(), Some(chosen.as_path()));
        // Back to the default removes the pointer, and nothing else.
        gone.choose_root(None).unwrap();
        assert!(Home::locate(Some(config)).unavailable().is_none());
        // A home fixed from outside is not moved by the app.
        assert!(Home::new(&base).choose_root(Some(&base)).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
