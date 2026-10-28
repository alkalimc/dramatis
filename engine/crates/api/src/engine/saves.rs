//! Saves: which world this process has open, and the operations on the set of them.
//!
//! The file work is in [`crate::saves`]. What lives here is the rule about the open save:
//! a switch (or renaming the open save) closes the world, so it waits for the agent to be
//! idle, and the open save cannot be deleted. The name of the save to open at the next
//! start is one small file in the home directory, which each process reads once; a switch
//! in one process does not move another that is already running.

use std::path::Path;

use world::World;

use super::{Engine, io};
use crate::convert::world_err;
use crate::error::{ApiError, ApiResult};
use crate::saves;
use crate::views::SaveInfo;

impl Engine {
    fn open_save(&self) -> String {
        self.save.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
        // Opened (and migrated) before anything changes: a file that will not open
        // leaves the current world as it was.
        let next = World::open(&path).map_err(world_err)?;
        self.home.set_current_save(name).map_err(io)?;
        *self.w() = next;
        self.ports.agent.switch_world(&path);
        *open = name.to_owned();
        Ok(())
    }

    pub(crate) fn rename_save_impl(&self, from: &str, to: &str) -> ApiResult<()> {
        let mut open = self.save.lock().unwrap_or_else(|e| e.into_inner());
        if open.as_str() != from {
            return saves::rename(&self.home, from, to);
        }
        saves::path_of(&self.home, from)?;
        if !self.ports.agent.idle() {
            return Err(ApiError::Busy);
        }
        // The open connection is dropped first so its log is folded into the file and the
        // file can move whole.
        let mut world = self.w();
        drop(std::mem::replace(
            &mut *world,
            World::in_memory().map_err(world_err)?,
        ));
        let moved = saves::rename(&self.home, from, to);
        let now = if moved.is_ok() { to } else { from };
        *world = World::open(self.home.save_path(now)).map_err(world_err)?;
        moved?;
        self.home.set_current_save(to).map_err(io)?;
        self.ports.agent.switch_world(&self.home.save_path(to));
        *open = to.to_owned();
        Ok(())
    }

    pub(crate) fn delete_save_impl(&self, name: &str) -> ApiResult<()> {
        if self.open_save() == name {
            return Err(ApiError::Invalid {
                field: "name".into(),
            });
        }
        saves::delete(&self.home, name)
    }

    pub(crate) fn export_save_impl(&self, name: &str, path: &str) -> ApiResult<()> {
        saves::export(&self.home, name, Path::new(path))
    }

    pub(crate) fn import_save_impl(&self, path: &str, name: Option<&str>) -> ApiResult<String> {
        saves::import(&self.home, Path::new(path), name)
    }
}
