//! Saves: which world this process has open, and the operations on the set of them.
//!
//! The file work is in [`crate::saves`]. What lives here is the rule about the open save:
//! a switch (or renaming the open save) closes the world, so it waits for the agent to be
//! idle, and the open save cannot be deleted. The name of the save to open at the next
//! start is one small file in the home directory, which each process reads once; a switch
//! in one process does not move another that is already running.
//!
//! Passwords of encrypted saves are kept here, in memory, by save name, from the moment
//! they are entered. Turning encryption on or off, or changing the password, rewrites the
//! file, so it needs the engine idle and the save closed by this process; another process
//! with the same save open would be left on the old file and must be restarted.

use std::path::Path;
use std::sync::atomic::Ordering;

use world::World;

use super::{Engine, io};
use crate::convert::world_err;
use crate::error::{ApiError, ApiResult};
use crate::saves;
use crate::views::SaveInfo;

impl Engine {
    pub(crate) fn open_save(&self) -> String {
        self.save.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The password entered for save `name` in this process, if any.
    pub(crate) fn password_of(&self, name: &str) -> Option<String> {
        self.passwords
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// The password entered for the open save in this process, so a front end that
    /// reopens the engine on the same home can carry it over (in memory only).
    pub fn open_password(&self) -> Option<String> {
        self.password_of(&self.open_save())
    }

    fn remember(&self, name: &str, password: Option<&str>) {
        let mut all = self.passwords.lock().unwrap_or_else(|e| e.into_inner());
        match password {
            Some(p) => all.insert(name.to_owned(), p.to_owned()),
            None => all.remove(name),
        };
    }

    pub(crate) fn saves_impl(&self) -> Vec<SaveInfo> {
        saves::list(&self.home, &self.open_save(), self.now())
    }

    pub(crate) fn switch_save_impl(&self, name: &str) -> ApiResult<()> {
        let path = saves::path_of(&self.home, name)?;
        let mut open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        if open.as_str() == name {
            return Ok(());
        }
        if !self.ports.agent.idle() {
            return Err(ApiError::Busy);
        }
        // Opened (and migrated) before anything changes: a file that will not open, or
        // that wants a password not yet entered (`Locked`), leaves the current world as it was.
        let key = self.password_of(name);
        let next = World::open_with(&path, key.as_deref()).map_err(world_err)?;
        self.home.set_current_save(name).map_err(io)?;
        *self.missing_save.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.w() = next;
        self.locked.store(false, Ordering::Relaxed);
        self.ports.agent.switch_world(&path, key.as_deref());
        *open = name.to_owned();
        Ok(())
    }

    /// Close the open save and leave none: the world becomes the empty stand-in, the name
    /// for the next start is cleared (so every client starts at the picker), and an
    /// encrypted save's password is forgotten with it.
    pub(crate) fn log_out_impl(&self) -> ApiResult<()> {
        let mut open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        if open.is_empty() {
            return Ok(());
        }
        if !self.ports.agent.idle() {
            return Err(ApiError::Busy);
        }
        self.home.clear_current_save().map_err(io)?;
        // The connection is dropped so the file is settled and free for other processes.
        *self.w() = World::in_memory().map_err(world_err)?;
        self.locked.store(false, Ordering::Relaxed);
        self.ports.agent.switch_world(Path::new(""), None);
        self.remember(&open, None);
        open.clear();
        Ok(())
    }

    pub(crate) fn unlock_impl(&self, name: &str, password: String) -> ApiResult<()> {
        let path = saves::path_of(&self.home, name)?;
        let open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        if !world::cipher::is_encrypted(&path) {
            return Ok(());
        }
        let next = World::open_with(&path, Some(&password)).map_err(|e| match e {
            world::Error::Locked => ApiError::WrongPassword,
            other => world_err(other),
        })?;
        self.remember(name, Some(&password));
        if open.as_str() == name && self.locked.load(Ordering::Relaxed) {
            *self.w() = next;
            self.locked.store(false, Ordering::Relaxed);
            self.ports.agent.switch_world(&path, Some(&password));
        }
        Ok(())
    }

    /// Encrypt, re-key or decrypt the open save. The engine's own connection is closed for
    /// the rewrite and reopened under the resulting password.
    pub(crate) fn set_password_impl(
        &self,
        old: Option<String>,
        new: Option<String>,
    ) -> ApiResult<()> {
        let invalid = |field: &str| ApiError::Invalid {
            field: field.into(),
        };
        if old.is_none() && new.is_none() {
            return Err(invalid("password"));
        }
        if new.as_deref().is_some_and(|n| !world::cipher::valid_key(n)) {
            return Err(invalid("new"));
        }
        let open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        if open.is_empty() {
            return Err(ApiError::NotFound);
        }
        if self.locked.load(Ordering::Relaxed) {
            return Err(ApiError::Locked);
        }
        if !self.ports.agent.idle() {
            return Err(ApiError::Busy);
        }
        let path = self.home.save_path(&open);
        // Whether `old` is expected is a fact of the file, not the caller's say.
        if world::cipher::is_encrypted(&path) != old.is_some() {
            return Err(invalid("old"));
        }
        let mut world = self.w();
        drop(std::mem::replace(
            &mut *world,
            World::in_memory().map_err(world_err)?,
        ));
        let done = match (old.as_deref(), new.as_deref()) {
            (None, Some(n)) => world::cipher::encrypt(&path, n),
            (Some(o), Some(n)) => world::cipher::rekey(&path, o, n),
            (Some(o), None) => world::cipher::decrypt(&path, o),
            (None, None) => unreachable!("refused above"),
        };
        // After a failure the file is as it was, so it reopens under the password this
        // process knows (which `old` may not be: a wrong `old` is how it failed).
        let key = if done.is_ok() {
            new.clone()
        } else {
            self.password_of(&open)
        };
        let reopened = World::open_with(&path, key.as_deref()).map_err(world_err);
        match reopened {
            Ok(w) => *world = w,
            Err(e) => {
                // The engine would otherwise keep writing to a blank world nobody sees:
                // it stays locked until the save opens again.
                self.locked.store(true, Ordering::Relaxed);
                if done.is_ok() {
                    self.remember(&open, new.as_deref());
                    return Err(e);
                }
            }
        }
        drop(world);
        done.map_err(|e| match e {
            world::Error::Locked => ApiError::WrongPassword,
            other => world_err(other),
        })?;
        self.remember(&open, new.as_deref());
        self.ports.agent.switch_world(&path, new.as_deref());
        Ok(())
    }

    pub(crate) fn rename_save_impl(&self, from: &str, to: &str) -> ApiResult<()> {
        let mut open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        if open.as_str() != from {
            saves::rename(&self.home, from, to)?;
            self.move_password(from, to);
            return Ok(());
        }
        saves::path_of(&self.home, from)?;
        if !self.ports.agent.idle() {
            return Err(ApiError::Busy);
        }
        // The open connection is dropped first so its journal is settled and the file can
        // move whole.
        let key = self.password_of(from);
        let mut world = self.w();
        drop(std::mem::replace(
            &mut *world,
            World::in_memory().map_err(world_err)?,
        ));
        let locked = self.locked.load(Ordering::Relaxed);
        let reopen = |name: &str| {
            if locked {
                World::in_memory()
            } else {
                World::open_with(self.home.save_path(name), key.as_deref())
            }
            .map_err(world_err)
        };
        let moved = saves::rename(&self.home, from, to);
        // The new name takes hold only once the file reopens under it and the name for
        // the next start is written; any failure before that puts everything back.
        let next = moved.clone().and_then(|()| {
            let w = reopen(to)?;
            self.home.set_current_save(to).map_err(io)?;
            Ok(w)
        });
        let w = match next {
            Ok(w) => w,
            Err(e) => {
                if moved.is_ok() {
                    let _ = saves::rename(&self.home, to, from);
                }
                match reopen(from) {
                    Ok(w) => *world = w,
                    // Nothing writable is open: say so rather than let writes vanish.
                    Err(_) => self.locked.store(true, Ordering::Relaxed),
                }
                return Err(e);
            }
        };
        *world = w;
        drop(world);
        self.move_password(from, to);
        self.ports
            .agent
            .switch_world(&self.home.save_path(to), key.as_deref());
        *open = to.to_owned();
        Ok(())
    }

    fn move_password(&self, from: &str, to: &str) {
        let key = self.password_of(from);
        self.remember(from, None);
        if let Some(k) = key {
            self.remember(to, Some(&k));
        }
    }

    pub(crate) fn delete_save_impl(&self, name: &str) -> ApiResult<()> {
        if self.open_save() == name {
            return Err(ApiError::Invalid {
                field: "name".into(),
            });
        }
        saves::delete(&self.home, name)?;
        self.remember(name, None);
        Ok(())
    }

    pub(crate) fn export_save_impl(&self, name: &str, path: &str) -> ApiResult<()> {
        saves::export(
            &self.home,
            name,
            Path::new(path),
            self.password_of(name).as_deref(),
        )
    }

    pub(crate) fn import_save_impl(
        &self,
        path: &str,
        name: Option<&str>,
        password: Option<String>,
    ) -> ApiResult<String> {
        let name =
            saves::import(&self.home, Path::new(path), name, password.as_deref()).map_err(|e| {
                match e {
                    ApiError::Locked if password.is_some() => ApiError::WrongPassword,
                    other => other,
                }
            })?;
        self.remember(&name, password.as_deref());
        Ok(name)
    }
}
