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

#[cfg(windows)]
fn ident(path: &Path) -> Option<Ident> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // No access asked: only attributes are read, so SQLite's sharing mode is no obstacle.
    let file = OpenOptions::new().access_mode(0).open(path).ok()?;
    // SAFETY: an all-zero struct of integers is valid, and the handle is open for the call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return None;
    }
    Some(Ident {
        dev: u64::from(info.dwVolumeSerialNumber),
        ino: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

#[cfg(not(any(unix, windows)))]
fn ident(path: &Path) -> Option<Ident> {
    // No file identity to compare: the file's age stands in, which still sees a swap.
    let m = std::fs::metadata(path).ok()?;
    let age = m
        .created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(Ident {
        dev: 0,
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

/// `LockFileEx` on the first byte; the lock goes when the handle closes.
#[cfg(windows)]
fn flock(file: &File, mode: Mode, wait: bool) -> ApiResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;
    let flags = match mode {
        Mode::Shared => 0,
        Mode::Exclusive => LOCKFILE_EXCLUSIVE_LOCK,
    } | if wait { 0 } else { LOCKFILE_FAIL_IMMEDIATELY };
    // SAFETY: the handle is open for as long as `file` is borrowed; the zeroed OVERLAPPED
    // (offset 0) lives across the call, which is synchronous on a handle opened without
    // FILE_FLAG_OVERLAPPED.
    let mut at: OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut at) } != 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    Err(if e.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        ApiError::OpenElsewhere
    } else {
        ApiError::Io {
            detail: e.to_string(),
        }
    })
}

/// Let go of the lock on the first byte, for a downgrade (`LockFileEx` cannot convert).
#[cfg(windows)]
fn unlock(file: &File) -> ApiResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;
    // SAFETY: as in `flock`.
    let mut at: OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut at) } != 0 {
        return Ok(());
    }
    Err(ApiError::Io {
        detail: io::Error::last_os_error().to_string(),
    })
}

/// No advisory locks here: the poller's check of the file is all there is.
#[cfg(not(any(unix, windows)))]
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
        // Windows cannot convert a lock in place: let go, then wait for a shared one. Another
        // process may take the lock exclusively in between; it is then waited out.
        #[cfg(windows)]
        unlock(&self.file)?;
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
        std::fs::write(&save, b"longer").unwrap();
        assert!(!held.moved());
        std::fs::write(dir.join("b"), b"y").unwrap();
        std::fs::rename(dir.join("b"), &save).unwrap();
        assert!(held.moved());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
