//! `$DRAMATIS_HOME` (default `~/.dramatis`): everything the app keeps on this machine.
//!
//! ```text
//! endpoints.toml   profiles and roles (no secrets; keys live in the keychain)
//! params.toml      optional: calibration register overrides, same dotted names
//! saves/<name>.sqlite  one world per save: everything that changes with play
//! current_save     the name of the save a process opens when it starts
//! office/          notes written by people looking into a request
//! media/           avatars and read-aloud audio: a bounded cache, never archived
//! prompts/         optional local overrides, `<subject>.yaml` = { slot: body }
//! <pack>.folio     the corpus, unless `DRAMATIS_FOLIO` points elsewhere
//! ```

use std::io;
use std::path::{Path, PathBuf};

/// Overrides the home directory.
pub const HOME_ENV: &str = "DRAMATIS_HOME";
/// The save a fresh home starts with (and the old single `world.sqlite` becomes).
pub const DEFAULT_SAVE: &str = "default";
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

    /// Create the directory layout (idempotent) and move a pre-saves `world.sqlite` into
    /// it as the default save.
    pub fn create(&self) -> io::Result<()> {
        for dir in [
            self.office(),
            self.avatars(),
            self.tts(),
            self.prompts(),
            self.saves(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        self.adopt_legacy_world()
    }

    /// Before saves, the one world was `world.sqlite` in the root. It is moved (never
    /// copied or deleted) to become the default save, once: after that the root file is
    /// not looked at again.
    fn adopt_legacy_world(&self) -> io::Result<()> {
        let old = self.root.join("world.sqlite");
        let new = self.save_path(DEFAULT_SAVE);
        if !old.is_file() || new.exists() {
            return Ok(());
        }
        // Fold the write-ahead log into the file so the three files need not move
        // together; when another process holds the file this fails and they all move.
        if let Ok(conn) = rusqlite::Connection::open(&old) {
            let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
        std::fs::rename(&old, &new)?;
        for suffix in ["-wal", "-shm"] {
            let side = self.root.join(format!("world.sqlite{suffix}"));
            if side.exists() {
                std::fs::rename(
                    &side,
                    self.root.join(format!("saves/default.sqlite{suffix}")),
                )?;
            }
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

    /// The world database of the current save.
    pub fn world(&self) -> PathBuf {
        self.save_path(&self.current_save())
    }

    pub fn saves(&self) -> PathBuf {
        self.root.join("saves")
    }

    pub fn save_path(&self, name: &str) -> PathBuf {
        self.saves().join(format!("{name}.sqlite"))
    }

    /// The save a process opens at start (the default when the recorded one is gone). Each process reads this once; switching in one
    /// does not move another that is already running.
    pub fn current_save(&self) -> String {
        std::fs::read_to_string(self.root.join("current_save"))
            .map(|s| s.trim().to_owned())
            .ok()
            .filter(|s| !s.is_empty() && self.save_path(s).is_file())
            .unwrap_or_else(|| DEFAULT_SAVE.to_owned())
    }

    /// Record the save the next process opens (written whole, then renamed in).
    pub fn set_current_save(&self, name: &str) -> io::Result<()> {
        let tmp = self
            .root
            .join(format!("current_save.{}.tmp", std::process::id()));
        std::fs::write(&tmp, name)?;
        std::fs::rename(&tmp, self.root.join("current_save"))
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
        assert!(home.saves().is_dir());
        assert_eq!(home.folio_in_home(), None);
        std::fs::write(root.join("pack.folio"), b"").unwrap();
        std::fs::write(root.join("notes.txt"), b"").unwrap();
        assert_eq!(home.folio_in_home(), Some(root.join("pack.folio")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn current_save_defaults_and_follows_the_file() {
        let root = std::env::temp_dir().join(format!("dramatis-cur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        home.create().unwrap();
        assert_eq!(home.current_save(), DEFAULT_SAVE);
        assert_eq!(home.world(), root.join("saves/default.sqlite"));
        // A name with no file behind it is ignored.
        home.set_current_save("second").unwrap();
        assert_eq!(home.current_save(), DEFAULT_SAVE);
        std::fs::write(home.save_path("second"), b"").unwrap();
        assert_eq!(home.world(), root.join("saves/second.sqlite"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_old_world_file_becomes_the_default_save_once() {
        let root = std::env::temp_dir().join(format!("dramatis-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let conn = world::open(root.join("world.sqlite")).unwrap();
        conn.execute("INSERT INTO settings VALUES ('user.name', '\"Doc\"')", [])
            .unwrap();
        drop(conn);
        let home = Home::new(&root);
        home.create().unwrap();
        assert!(!root.join("world.sqlite").exists());
        let moved = world::open(home.world()).unwrap();
        let n: i64 = moved
            .query_row(
                "SELECT count(*) FROM settings WHERE key = 'user.name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        drop(moved);
        // A stray root file later is left alone.
        std::fs::write(root.join("world.sqlite"), b"x").unwrap();
        home.create().unwrap();
        assert!(root.join("world.sqlite").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
