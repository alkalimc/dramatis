//! `$DRAMATIS_HOME` (default `~/.dramatis`): everything the app keeps on this machine.
//!
//! ```text
//! endpoints.toml   profiles and roles (no secrets; keys live in the keychain)
//! params.toml      optional: calibration register overrides, same dotted names
//! saves/<name>.sqlite  one world per save: everything that changes with play
//! current_save     the name of the save a process opens when it starts (empty: none)
//! interface_language  `corpus` or `en`: the words the interface speaks (absent: never chosen)
//! update_check     `on` or `off`: automatic update checks (absent: off, and not yet asked)
//! update_cache.json  the last successful update check: time, version, release page
//! corpus_path      optional: the full path of the corpus the user chose in the app
//! uploads/         pictures the user chose (theirs, a group's), named by content hash
//! media/           avatars and read-aloud audio: a bounded cache, never archived
//! prompts/         optional local overrides, `<subject>.yaml` = { slot: body }
//! <pack>.folio     the corpus, unless `DRAMATIS_FOLIO` points elsewhere
//! ```

use std::io;
use std::path::{Path, PathBuf};

use crate::update::Checked;
use crate::views::InterfaceLanguage;

/// Overrides the home directory.
pub const HOME_ENV: &str = "DRAMATIS_HOME";
/// The save a fresh home starts with.
pub const DEFAULT_SAVE: &str = "default";
/// Overrides which corpus is opened.
pub const FOLIO_ENV: &str = "DRAMATIS_FOLIO";

/// Which save a process starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    Open(String),
    /// Ask the user. `missing` is the recorded save that no longer exists, to say so.
    Pick {
        missing: Option<String>,
    },
}

/// The user's home directory: `%USERPROFILE%` on Windows, `$HOME` elsewhere.
pub fn user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// What follows a leading `~/` (also `~\` on Windows), if the path starts with one.
pub fn tilde_rest(path: &str) -> Option<&str> {
    #[cfg(windows)]
    if let Some(rest) = path.strip_prefix("~\\") {
        return Some(rest);
    }
    path.strip_prefix("~/")
}

/// A path as typed: a leading `~/` (also `~\` on Windows) is the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    match (tilde_rest(path), user_home()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => path.into(),
    }
}

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
        let user = user_home().unwrap_or_else(|| ".".into());
        Self::new(user.join(".dramatis"))
    }

    /// Create the directory layout (idempotent).
    pub fn create(&self) -> io::Result<()> {
        for dir in [
            self.logs(),
            self.avatars(),
            self.uploads(),
            self.tts(),
            self.prompts(),
            self.saves(),
        ] {
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

    /// The world database of the save to open at start, when there is one.
    pub fn world(&self) -> Option<PathBuf> {
        self.current_save().map(|name| self.save_path(&name))
    }

    pub fn saves(&self) -> PathBuf {
        self.root.join("saves")
    }

    pub fn save_path(&self, name: &str) -> PathBuf {
        self.saves().join(format!("{name}.sqlite"))
    }

    /// What a process does about saves at start. The app and the terminal both read this
    /// once and keep what they opened until restarted, so they agree on the last save
    /// without sharing a live one.
    ///
    /// The file `current_save` names the last save used. A home from before the file
    /// existed has none and opens the `default` save if there is one. A file that is
    /// empty (the user logged out) or that names a save which is gone (renamed or deleted
    /// elsewhere) leaves the choice to the user.
    pub fn start_save(&self) -> Start {
        let recorded = match std::fs::read_to_string(self.root.join("current_save")) {
            Ok(text) => Some(text.trim().to_owned()),
            Err(_) => None,
        };
        match recorded {
            None if self.save_path(DEFAULT_SAVE).is_file() => Start::Open(DEFAULT_SAVE.into()),
            None => Start::Pick { missing: None },
            Some(name) if name.is_empty() => Start::Pick { missing: None },
            Some(name) if self.save_path(&name).is_file() => Start::Open(name),
            Some(name) => Start::Pick {
                missing: Some(name),
            },
        }
    }

    /// The save a process opens at start, if there is one to open.
    pub fn current_save(&self) -> Option<String> {
        match self.start_save() {
            Start::Open(name) => Some(name),
            Start::Pick { .. } => None,
        }
    }

    /// Record the save the next process opens (written whole, then renamed in).
    pub fn set_current_save(&self, name: &str) -> io::Result<()> {
        let tmp = self
            .root
            .join(format!("current_save.{}.tmp", std::process::id()));
        std::fs::write(&tmp, name)?;
        std::fs::rename(&tmp, self.root.join("current_save"))
    }

    /// Log out: the next process starts at the save picker.
    pub fn clear_current_save(&self) -> io::Result<()> {
        self.set_current_save("")
    }

    /// The interface language the person chose; `None` before any choice.
    pub fn interface_language(&self) -> Option<InterfaceLanguage> {
        match std::fs::read_to_string(self.root.join("interface_language"))
            .ok()?
            .trim()
        {
            "corpus" => Some(InterfaceLanguage::Corpus),
            "en" => Some(InterfaceLanguage::English),
            _ => None,
        }
    }

    pub fn set_interface_language(&self, language: InterfaceLanguage) -> io::Result<()> {
        let word = match language {
            InterfaceLanguage::Corpus => "corpus",
            InterfaceLanguage::English => "en",
        };
        let tmp = self
            .root
            .join(format!("interface_language.{}.tmp", std::process::id()));
        std::fs::write(&tmp, word)?;
        std::fs::rename(&tmp, self.root.join("interface_language"))
    }

    /// Whether the app checks for a newer release by itself; off until the user says yes.
    pub fn auto_update_check(&self) -> bool {
        std::fs::read_to_string(self.root.join("update_check")).is_ok_and(|t| t.trim() == "on")
    }

    /// Whether the user has answered the question about automatic checks.
    pub fn update_check_asked(&self) -> bool {
        self.root.join("update_check").is_file()
    }

    pub fn set_auto_update_check(&self, auto: bool) -> io::Result<()> {
        self.write_whole("update_check", if auto { "on" } else { "off" }.as_bytes())
    }

    /// The last successful update check, if one was kept and is readable.
    pub fn update_cache(&self) -> Option<Checked> {
        serde_json::from_slice(&std::fs::read(self.root.join("update_cache.json")).ok()?).ok()
    }

    pub fn set_update_cache(&self, checked: &Checked) -> io::Result<()> {
        self.write_whole("update_cache.json", &serde_json::to_vec(checked)?)
    }

    /// Write a file whole, then rename it in.
    fn write_whole(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let tmp = self.root.join(format!("{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, self.root.join(name))
    }

    /// The corpus the user chose in the app, if the file is still there.
    pub fn chosen_corpus(&self) -> Option<PathBuf> {
        let text = std::fs::read_to_string(self.root.join("corpus_path")).ok()?;
        let path = PathBuf::from(text.trim());
        path.is_file().then_some(path)
    }

    /// Remember the corpus to open at the next start (written whole, then renamed in).
    pub fn set_chosen_corpus(&self, path: &Path) -> io::Result<()> {
        let tmp = self
            .root
            .join(format!("corpus_path.{}.tmp", std::process::id()));
        std::fs::write(&tmp, path.to_string_lossy().as_bytes())?;
        std::fs::rename(&tmp, self.root.join("corpus_path"))
    }

    /// Diagnostics: what a turn that ended without a reply leaves behind.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn media(&self) -> PathBuf {
        self.root.join("media")
    }

    /// Pictures the user chose (their own, a group's). Not under `media/`: that is a cache
    /// that evicts, and these cannot be fetched again.
    pub fn uploads(&self) -> PathBuf {
        self.root.join("uploads")
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

    /// The corpus to open at startup: `DRAMATIS_FOLIO`, else the one the user chose, else
    /// the `.folio` in the home directory. `None` means the app must ask the user for a file.
    pub fn resolve_folio(&self) -> Option<PathBuf> {
        std::env::var_os(FOLIO_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.chosen_corpus())
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
    fn tilde_is_the_user_home() {
        assert_eq!(tilde_rest("~/x/y"), Some("x/y"));
        assert_eq!(tilde_rest("x/~/y"), None);
        assert_eq!(tilde_rest("~x"), None);
        #[cfg(windows)]
        assert_eq!(tilde_rest("~\\x"), Some("x"));
        #[cfg(not(windows))]
        assert_eq!(tilde_rest("~\\x"), None);
        if let Some(home) = user_home() {
            assert_eq!(expand_tilde("~/a"), home.join("a"));
        }
        assert_eq!(expand_tilde("/a/b"), PathBuf::from("/a/b"));
    }

    #[test]
    fn layout_and_folio_discovery() {
        let root = std::env::temp_dir().join(format!("dramatis-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        home.create().unwrap();
        assert!(home.tts().is_dir() && home.avatars().is_dir());
        assert!(home.saves().is_dir());
        assert_eq!(home.folio_in_home(), None);
        std::fs::write(root.join("pack.folio"), b"").unwrap();
        std::fs::write(root.join("notes.txt"), b"").unwrap();
        assert_eq!(home.folio_in_home(), Some(root.join("pack.folio")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn start_save_follows_the_file_and_asks_when_it_cannot() {
        let root = std::env::temp_dir().join(format!("dramatis-cur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        home.create().unwrap();
        // A fresh home has no save: the user picks or creates one.
        assert_eq!(home.start_save(), Start::Pick { missing: None });
        assert_eq!(home.world(), None);
        // A home from before the record opens its default save.
        std::fs::write(home.save_path(DEFAULT_SAVE), b"").unwrap();
        assert_eq!(home.current_save().as_deref(), Some(DEFAULT_SAVE));
        // A name with no file behind it is reported, not replaced.
        home.set_current_save("second").unwrap();
        assert_eq!(
            home.start_save(),
            Start::Pick {
                missing: Some("second".into())
            }
        );
        std::fs::write(home.save_path("second"), b"").unwrap();
        assert_eq!(home.world(), Some(root.join("saves/second.sqlite")));
        // Logging out leaves the file empty and the picker is asked for, even with saves.
        home.clear_current_save().unwrap();
        assert_eq!(home.start_save(), Start::Pick { missing: None });
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_chosen_corpus_beats_the_home_folder_and_must_exist() {
        let root = std::env::temp_dir().join(format!("dramatis-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        home.create().unwrap();
        std::fs::write(root.join("a.folio"), b"").unwrap();
        let elsewhere = root.join("saves/b.folio");
        std::fs::write(&elsewhere, b"").unwrap();
        home.set_chosen_corpus(&elsewhere).unwrap();
        if std::env::var_os(FOLIO_ENV).is_none() {
            assert_eq!(home.resolve_folio(), Some(elsewhere.clone()));
        }
        std::fs::remove_file(&elsewhere).unwrap();
        assert_eq!(home.chosen_corpus(), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
