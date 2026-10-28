//! Profile keys in the system keychain: service `dramatis`, account `profile.<name>`.
//! A key is never read from the environment or a file, and never printed.
//!
//! Each read of a legacy keychain item can raise a macOS password prompt, so a key is
//! read at most once per process: the first `get` keeps it in memory, `set` and `delete`
//! keep the copy in step. Asking whether a key exists (`has`) reads attributes only and
//! never prompts.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::error::{Error, Result};
use crate::types::Secret;

pub const SERVICE: &str = "dramatis";

pub fn account(profile: &str) -> String {
    format!("profile.{profile}")
}

/// Where keys are kept. The system keychain in production, a fake in tests.
trait Store: Send + Sync {
    /// `None` when the profile has no stored key. May prompt.
    fn read(&self, profile: &str) -> Result<Option<Secret>>;
    /// Whether a key is stored, without reading it.
    fn exists(&self, profile: &str) -> Result<bool>;
    fn write(&self, profile: &str, key: &str) -> Result<()>;
    fn remove(&self, profile: &str) -> Result<()>;
}

/// A store with the keys it has read held in memory. Absent keys are not remembered:
/// another process (the app, the terminal client) may add one at any time, and looking
/// for a missing item never prompts.
struct Cached<S> {
    store: S,
    keys: Mutex<HashMap<String, Secret>>,
}

impl<S: Store> Cached<S> {
    fn new(store: S) -> Self {
        Self {
            store,
            keys: Mutex::default(),
        }
    }

    fn keys(&self) -> std::sync::MutexGuard<'_, HashMap<String, Secret>> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get(&self, profile: &str) -> Result<Option<Secret>> {
        if let Some(k) = self.keys().get(profile) {
            return Ok(Some(k.clone()));
        }
        let key = self.store.read(profile)?;
        if let Some(k) = &key {
            self.keys().insert(profile.to_owned(), k.clone());
        }
        Ok(key)
    }

    fn has(&self, profile: &str) -> Result<bool> {
        if self.keys().contains_key(profile) {
            return Ok(true);
        }
        self.store.exists(profile)
    }

    fn set(&self, profile: &str, key: &str) -> Result<()> {
        self.keys().remove(profile);
        self.store.write(profile, key)?;
        self.keys().insert(profile.to_owned(), Secret::new(key));
        Ok(())
    }

    fn delete(&self, profile: &str) -> Result<()> {
        self.keys().remove(profile);
        self.store.remove(profile)
    }
}

fn failed(e: impl ToString) -> Error {
    Error::Keychain(e.to_string())
}

struct System;

impl System {
    fn entry(profile: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &account(profile)).map_err(failed)
    }
}

impl Store for System {
    fn read(&self, profile: &str) -> Result<Option<Secret>> {
        match Self::entry(profile)?.get_password() {
            Ok(k) => Ok(Some(Secret::new(k))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(failed(e)),
        }
    }

    /// A search that returns attributes and not data does not need the item's access
    /// grant, so it does not prompt.
    #[cfg(target_os = "macos")]
    fn exists(&self, profile: &str) -> Result<bool> {
        use security_framework::item::{ItemClass, ItemSearchOptions};
        /// `errSecItemNotFound`
        const NOT_FOUND: i32 = -25300;
        match ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .account(&account(profile))
            .load_attributes(true)
            .search()
        {
            Ok(found) => Ok(!found.is_empty()),
            Err(e) if e.code() == NOT_FOUND => Ok(false),
            Err(e) => Err(failed(e)),
        }
    }

    /// Elsewhere keyring is its in-memory mock, which never prompts.
    #[cfg(not(target_os = "macos"))]
    fn exists(&self, profile: &str) -> Result<bool> {
        Ok(self.read(profile)?.is_some())
    }

    fn write(&self, profile: &str, key: &str) -> Result<()> {
        Self::entry(profile)?.set_password(key).map_err(failed)
    }

    fn remove(&self, profile: &str) -> Result<()> {
        match Self::entry(profile)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }
}

fn system() -> &'static Cached<System> {
    static KEYS: OnceLock<Cached<System>> = OnceLock::new();
    KEYS.get_or_init(|| Cached::new(System))
}

/// `None` when the profile has no stored key. Prompts at most once per profile and
/// process.
pub fn get(profile: &str) -> Result<Option<Secret>> {
    system().get(profile)
}

/// Whether the profile has a stored key. Never reads the key, so never prompts.
pub fn has(profile: &str) -> Result<bool> {
    system().has(profile)
}

pub fn set(profile: &str, key: &str) -> Result<()> {
    system().set(profile, key)
}

pub fn delete(profile: &str) -> Result<()> {
    system().delete(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    #[derive(Default)]
    struct Fake {
        items: Mutex<HashMap<String, String>>,
        reads: Arc<AtomicUsize>,
        probes: Arc<AtomicUsize>,
    }

    impl Store for Fake {
        fn read(&self, p: &str) -> Result<Option<Secret>> {
            self.reads.fetch_add(1, SeqCst);
            Ok(self.items.lock().unwrap().get(p).cloned().map(Secret::new))
        }
        fn exists(&self, p: &str) -> Result<bool> {
            self.probes.fetch_add(1, SeqCst);
            Ok(self.items.lock().unwrap().contains_key(p))
        }
        fn write(&self, p: &str, k: &str) -> Result<()> {
            self.items.lock().unwrap().insert(p.into(), k.into());
            Ok(())
        }
        fn remove(&self, p: &str) -> Result<()> {
            self.items.lock().unwrap().remove(p);
            Ok(())
        }
    }

    fn cache(items: &[(&str, &str)]) -> (Cached<Fake>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let fake = Fake::default();
        for (p, k) in items {
            fake.items.lock().unwrap().insert((*p).into(), (*k).into());
        }
        let (reads, probes) = (fake.reads.clone(), fake.probes.clone());
        (Cached::new(fake), reads, probes)
    }

    #[test]
    fn many_turns_read_each_profile_once() {
        let (c, reads, _) = cache(&[("a", "ka"), ("b", "kb"), ("unused", "ku")]);
        for _ in 0..50 {
            assert_eq!(c.get("a").unwrap().unwrap().expose(), "ka");
            assert_eq!(c.get("b").unwrap().unwrap().expose(), "kb");
        }
        assert_eq!(reads.load(SeqCst), 2);
    }

    #[test]
    fn has_never_reads_the_key() {
        let (c, reads, probes) = cache(&[("a", "ka")]);
        assert!(c.has("a").unwrap());
        assert!(!c.has("b").unwrap());
        assert_eq!(reads.load(SeqCst), 0);
        assert_eq!(probes.load(SeqCst), 2);
        c.get("a").unwrap();
        assert!(c.has("a").unwrap());
        assert_eq!(probes.load(SeqCst), 2, "a cached key needs no probe");
    }

    #[test]
    fn set_and_delete_keep_the_copy_in_step() {
        let (c, reads, _) = cache(&[("a", "old")]);
        assert_eq!(c.get("a").unwrap().unwrap().expose(), "old");
        c.set("a", "new").unwrap();
        assert_eq!(c.get("a").unwrap().unwrap().expose(), "new");
        assert_eq!(reads.load(SeqCst), 1, "set keeps the value it wrote");
        c.delete("a").unwrap();
        assert!(c.get("a").unwrap().is_none());
        assert!(!c.has("a").unwrap());
    }

    #[test]
    fn a_missing_key_is_looked_for_again() {
        let (c, reads, _) = cache(&[]);
        assert!(c.get("a").unwrap().is_none());
        c.store.write("a", "late").unwrap();
        assert_eq!(c.get("a").unwrap().unwrap().expose(), "late");
        assert_eq!(reads.load(SeqCst), 2);
    }
}
