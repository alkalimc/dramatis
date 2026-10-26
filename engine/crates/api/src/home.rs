//! `$DRAMATIS_HOME` (default `~/.dramatis`): everything the app keeps on this machine.
//!
//! ```text
//! endpoints.toml   profiles and roles (no secrets; keys live in the keychain)
//! params.toml      optional: calibration register overrides, same dotted names
//! world.sqlite     everything that changes with play
//! office/          notes written by people looking into a request
//! media/           avatars and read-aloud audio: a bounded cache, never archived
//! prompts/         optional local overrides, `<subject>.yaml` = { slot: body }
//! <pack>.folio     the corpus, unless `DRAMATIS_FOLIO` points elsewhere
//! ```

use std::io;
use std::path::{Path, PathBuf};

/// Overrides the home directory.
pub const HOME_ENV: &str = "DRAMATIS_HOME";
/// Overrides which corpus is opened.
pub const FOLIO_ENV: &str = "DRAMATIS_FOLIO";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$DRAMATIS_HOME`, else `~/.dramatis`.
    pub fn from_env() -> Self {
        if let Some(dir) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Self::new(dir);
        }
        let user = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
        Self::new(Path::new(&user).join(".dramatis"))
    }

    /// Create the directory layout (idempotent).
    pub fn create(&self) -> io::Result<()> {
        for dir in [self.office(), self.avatars(), self.tts(), self.prompts()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn endpoints(&self) -> PathBuf {
        self.root.join("endpoints.toml")
    }

    pub fn params(&self) -> PathBuf {
        self.root.join("params.toml")
    }

    pub fn world(&self) -> PathBuf {
        self.root.join("world.sqlite")
    }

    pub fn office(&self) -> PathBuf {
        self.root.join("office")
    }

    pub fn media(&self) -> PathBuf {
        self.root.join("media")
    }

    pub fn avatars(&self) -> PathBuf {
        self.media().join("avatar")
    }

    pub fn tts(&self) -> PathBuf {
        self.media().join("tts")
    }

    pub fn prompts(&self) -> PathBuf {
        self.root.join("prompts")
    }

    /// The corpus to open at startup: `DRAMATIS_FOLIO`, else the `.folio` in the home
    /// directory. `None` means the app must ask the user for a file.
    pub fn resolve_folio(&self) -> Option<PathBuf> {
        std::env::var_os(FOLIO_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.folio_in_home())
    }

    /// The `.folio` in the home directory; the newest when there are several.
    pub fn folio_in_home(&self) -> Option<PathBuf> {
        let entries = std::fs::read_dir(&self.root).ok()?;
        entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "folio") && p.is_file())
            .max_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_folio_discovery() {
        let root = std::env::temp_dir().join(format!("dramatis-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        home.create().unwrap();
        assert!(home.tts().is_dir() && home.avatars().is_dir() && home.office().is_dir());
        assert_eq!(home.folio_in_home(), None);
        std::fs::write(root.join("pack.folio"), b"").unwrap();
        std::fs::write(root.join("notes.txt"), b"").unwrap();
        assert_eq!(home.folio_in_home(), Some(root.join("pack.folio")));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
