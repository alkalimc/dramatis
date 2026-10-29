//! Telling other processes that a save is open. Each process with a save open holds a
//! shared advisory lock on `<save>.lock`; a step that rewrites, moves or deletes the file
//! takes the lock exclusively first, which fails while any other process holds it. The lock
//! is on a file beside the save, not on the save, because those steps replace the save's
//! file and a lock on the old one would mean nothing to anyone.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{ApiError, ApiResult};

/// Which file a path pointed at when it was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ident {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
fn ident(path: &Path) -> Option<Ident> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(Ident {
        dev: m.dev(),
        ino: m.ino(),
    })
}

#[cfg(not(unix))]
fn ident(path: &Path) -> Option<Ident> {
    // No inode to compare: the file's length and age stand in, which still sees a swap.
    let m = std::fs::metadata(path).ok()?;
    let age = m
        .created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(Ident {
        dev: m.len(),
        ino: age.as_nanos() as u64,
    })
}

/// The lock file of the save at `save`.
pub(crate) fn lock_path(save: &Path) -> PathBuf {
    PathBuf::from(format!("{}.lock", save.display()))
}

/// A lock held on one save; dropping it lets go.
#[derive(Debug)]
pub(crate) struct SaveLock {
    file: File,
    save: PathBuf,
    /// The file the save's path pointed at when the save was opened.
    opened: Option<Ident>,
}

fn open(save: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(save))
}

/// What a lock request is: shared, or exclusive, and whether to wait.
#[derive(Clone, Copy)]
enum Mode {
    Shared,
    Exclusive,
}

/// `flock` without waiting: another process in the way is [`ApiError::OpenElsewhere`].
#[cfg(unix)]
fn flock(file: &File, mode: Mode, wait: bool) -> ApiResult<()> {
    use std::os::fd::AsRawFd;
    let op = match mode {
        Mode::Shared => libc::LOCK_SH,
        Mode::Exclusive => libc::LOCK_EX,
    } | if wait { 0 } else { libc::LOCK_NB };
    // SAFETY: the descriptor is open for as long as `file` is borrowed.
    if unsafe { libc::flock(file.as_raw_fd(), op) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    Err(if e.kind() == io::ErrorKind::WouldBlock {
        ApiError::OpenElsewhere
    } else {
        ApiError::Io {
            detail: e.to_string(),
        }
    })
}

/// No advisory locks here: the poller's check of the file is all there is.
#[cfg(not(unix))]
fn flock(_: &File, _: Mode, _: bool) -> ApiResult<()> {
    Ok(())
}

impl SaveLock {
    /// Note that this process has the save open. Fails only while another process is
    /// rewriting it.
    pub fn shared(save: &Path) -> ApiResult<Self> {
        let file = open(save).map_err(|e| ApiError::Io {
            detail: e.to_string(),
        })?;
        flock(&file, Mode::Shared, false)?;
        Ok(Self::new(file, save))
    }

    /// Be the only process with the save open, or fail with [`ApiError::OpenElsewhere`].
    /// The caller lets go of its own shared lock first, as a process conflicts with itself.
    pub fn exclusive(save: &Path) -> ApiResult<Self> {
        let file = open(save).map_err(|e| ApiError::Io {
            detail: e.to_string(),
        })?;
        flock(&file, Mode::Exclusive, false)?;
        Ok(Self::new(file, save))
    }

    fn new(file: File, save: &Path) -> Self {
        Self {
            file,
            save: save.to_owned(),
            opened: ident(save),
        }
    }

    /// Back to a shared lock once the rewrite is done, remembering the file now there.
    pub fn downgrade(mut self) -> ApiResult<Self> {
        flock(&self.file, Mode::Shared, true)?;
        self.opened = ident(&self.save);
        Ok(self)
    }

    /// The file the save's path pointed at when this lock was taken.
    pub fn opened(&self) -> Option<Ident> {
        self.opened
    }

    /// Remember another file as the one opened: for a lock given up and taken again around
    /// a step that changed nothing.
    pub fn with_opened(mut self, opened: Option<Ident>) -> Self {
        self.opened = opened;
        self
    }

    /// The file at the save's path is not the one that was opened.
    pub fn moved(&self) -> bool {
        ident(&self.save) != self.opened
    }
}

/// Remove the lock file of a save that is gone or has another name.
pub(crate) fn forget(save: &Path) {
    let _ = std::fs::remove_file(lock_path(save));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_waits_for_every_shared_holder() {
        let dir = std::env::temp_dir().join(format!("dramatis-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.join("a.sqlite");
        std::fs::write(&save, b"x").unwrap();
        let one = SaveLock::shared(&save).unwrap();
        let two = SaveLock::shared(&save).unwrap();
        assert_eq!(
            SaveLock::exclusive(&save).unwrap_err(),
            ApiError::OpenElsewhere
        );
        drop(one);
        assert!(SaveLock::exclusive(&save).is_err());
        drop(two);
        let all = SaveLock::exclusive(&save).unwrap();
        assert_eq!(
            SaveLock::shared(&save).unwrap_err(),
            ApiError::OpenElsewhere
        );
        let held = all.downgrade().unwrap();
        assert!(!held.moved());
        std::fs::write(dir.join("b"), b"y").unwrap();
        std::fs::rename(dir.join("b"), &save).unwrap();
        assert!(held.moved());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
