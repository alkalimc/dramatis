//! Saves may be encrypted with a password (SQLCipher: its own key derivation and
//! defaults). Without a key a save is a plain SQLite file, so "off" is simply no key.
//! Turning encryption on or off rewrites the file through `sqlcipher_export` and swaps it
//! in whole; changing the password is `PRAGMA rekey`. All of it needs the save closed by
//! this process, and no other process may have it open: a swap leaves their connection on
//! the old file.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode};

use crate::{Error, Result};

/// How long a connection waits for another process's lock.
const BUSY: Duration = Duration::from_secs(5);

/// What a plain SQLite file starts with; an encrypted one starts with salt, which is random.
const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// A password SQLCipher can take: not empty, no NUL (it is passed as text).
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('\0')
}

/// SQLCipher's page size; an encrypted file is a whole number of pages.
const PAGE: u64 = 4096;

/// Whether the file at `path` looks like an encrypted database: not a plain header, and the
/// size of whole pages (an empty or absent file is a plain, new database; a text file is
/// not a database at all).
pub fn is_encrypted(path: impl AsRef<Path>) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let size = f.metadata().map_or(0, |m| m.len());
    let mut head = [0u8; 16];
    size >= PAGE && size % PAGE == 0 && f.read_exact(&mut head).is_ok() && &head != MAGIC
}

fn prepare(conn: Connection, key: Option<&str>) -> Result<Connection> {
    if let Some(k) = key {
        if !valid_key(k) {
            return Err(Error::Locked);
        }
        conn.pragma_update(None, "key", k)?;
    }
    conn.busy_timeout(BUSY)?;
    // The first read is where a missing or wrong key shows.
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    }) {
        Ok(_) => Ok(conn),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::NotADatabase => {
            Err(Error::Locked)
        }
        Err(e) => Err(e.into()),
    }
}

/// Open (creating if absent) and unlock.
pub(crate) fn connect(path: &Path, key: Option<&str>) -> Result<Connection> {
    prepare(Connection::open(path)?, key)
}

/// Open a file that must exist, and unlock it. (Not a read-write-only open: an ATTACH
/// from such a connection cannot create its file.)
pub(crate) fn connect_existing(path: &Path, key: Option<&str>) -> Result<Connection> {
    if !path.is_file() {
        return Err(Error::Io(std::io::ErrorKind::NotFound.into()));
    }
    connect(path, key)
}

/// Write a consistent single-file copy of `conn`'s database to `dest` (which must not
/// exist), encrypted under `key` (`None`: plain).
pub(crate) fn export_to(conn: &Connection, dest: &Path, key: Option<&str>) -> Result<()> {
    if dest.exists() {
        return Err(Error::Io(std::io::ErrorKind::AlreadyExists.into()));
    }
    let dest = dest
        .to_str()
        .ok_or_else(|| Error::Invalid("path is not text".into()))?;
    if key.is_some_and(|k| !valid_key(k)) {
        return Err(Error::Locked);
    }
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    // An empty key attaches a plain database.
    conn.execute(
        "ATTACH DATABASE ?1 AS next KEY ?2",
        (dest, key.unwrap_or("")),
    )?;
    let done = conn
        .query_row("SELECT sqlcipher_export('next')", [], |_| Ok(()))
        .and_then(|()| conn.execute_batch(&format!("PRAGMA next.user_version = {version}")));
    // Detached either way, so the connection is reusable and the file is complete.
    let detached = conn.execute_batch("DETACH DATABASE next");
    done?;
    detached?;
    Ok(())
}

fn staging(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.rewrite-{}", path.display(), std::process::id()))
}

/// Rewrite the database at `path` from key `from` to key `to` (`None`: plain), swapping the
/// new file in only once it opens under the new key. Any failure leaves the original.
pub fn rewrite(path: &Path, from: Option<&str>, to: Option<&str>) -> Result<()> {
    let part = staging(path);
    let _ = std::fs::remove_file(&part);
    let built = (|| {
        let conn = connect_existing(path, from)?;
        export_to(&conn, &part, to)?;
        drop(conn);
        let check = connect_existing(&part, to)?;
        let quick: String = check.pragma_query_value(None, "quick_check", |r| r.get(0))?;
        drop(check);
        if quick != "ok" {
            return Err(Error::Invalid("the rewritten database is damaged".into()));
        }
        std::fs::rename(&part, path)?;
        Ok(())
    })();
    if built.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    built
}

/// Encrypt a plain database under `key`.
pub fn encrypt(path: &Path, key: &str) -> Result<()> {
    if is_encrypted(path) {
        return Err(Error::Invalid("already encrypted".into()));
    }
    rewrite(path, None, Some(key))
}

/// Decrypt the database with its password `key`.
pub fn decrypt(path: &Path, key: &str) -> Result<()> {
    rewrite(path, Some(key), None)
}

/// Give an encrypted database a new password.
pub fn rekey(path: &Path, old: &str, new: &str) -> Result<()> {
    if !valid_key(new) {
        return Err(Error::Locked);
    }
    let conn = connect_existing(path, Some(old))?;
    conn.pragma_update(None, "rekey", new)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("world-cipher-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn mark(w: &World, v: &str) {
        crate::settings::set(w, "mark", v).unwrap();
    }

    fn marked(w: &World) -> Option<String> {
        crate::settings::get(w, "mark").unwrap()
    }

    #[test]
    fn encrypt_lock_rekey_and_decrypt_keep_the_data_in_one_file() {
        let dir = scratch("life");
        let path = dir.join("a.sqlite");
        let w = World::open(&path).unwrap();
        mark(&w, "plain");
        drop(w);
        assert!(!is_encrypted(&path));

        encrypt(&path, "pw one").unwrap();
        assert!(is_encrypted(&path));
        assert_eq!(files(&dir), ["a.sqlite"], "no staging file is left");
        // Without the password, or with another, it does not open.
        assert!(matches!(World::open(&path), Err(Error::Locked)));
        assert!(matches!(
            World::open_with(&path, Some("nope")),
            Err(Error::Locked)
        ));
        assert!(is_encrypted(&path), "a refused open changes nothing");
        let w = World::open_with(&path, Some("pw one")).unwrap();
        assert_eq!(marked(&w).as_deref(), Some("plain"));
        mark(&w, "written while encrypted");
        assert_eq!(
            crate::SCHEMA_VERSION,
            w.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap()
        );
        drop(w);
        assert_eq!(files(&dir), ["a.sqlite"], "one file once closed");
        // The words are not in the file.
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(7).any(|w| w == b"written"));

        assert!(matches!(
            rekey(&path, "wrong", "pw two"),
            Err(Error::Locked)
        ));
        rekey(&path, "pw one", "pw two").unwrap();
        assert!(matches!(
            World::open_with(&path, Some("pw one")),
            Err(Error::Locked)
        ));
        let w = World::open_with(&path, Some("pw two")).unwrap();
        assert_eq!(marked(&w).as_deref(), Some("written while encrypted"));
        drop(w);

        assert!(matches!(decrypt(&path, "wrong"), Err(Error::Locked)));
        decrypt(&path, "pw two").unwrap();
        assert!(!is_encrypted(&path));
        let w = World::open(&path).unwrap();
        assert_eq!(marked(&w).as_deref(), Some("written while encrypted"));
        drop(w);
        assert_eq!(files(&dir), ["a.sqlite"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_password_with_quotes_works_and_an_empty_one_is_refused() {
        let dir = scratch("quote");
        let path = dir.join("a.sqlite");
        drop(World::open(&path).unwrap());
        assert!(matches!(encrypt(&path, ""), Err(Error::Locked)));
        encrypt(&path, "it's \"odd\"; --").unwrap();
        World::open_with(&path, Some("it's \"odd\"; --")).unwrap();
        assert!(matches!(
            World::open_with(&path, Some("it's")),
            Err(Error::Locked)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn copies_and_checks_follow_the_key() {
        let dir = scratch("copy");
        let path = dir.join("a.sqlite");
        let w = World::open(&path).unwrap();
        mark(&w, "m");
        drop(w);
        encrypt(&path, "k").unwrap();
        let w = World::open_with(&path, Some("k")).unwrap();
        let copy = dir.join("b.sqlite");
        // The first open handle is still there: the copy is consistent and encrypted.
        crate::copy_file(&path, &copy, Some("k")).unwrap();
        assert!(
            crate::copy_file(&path, &copy, Some("k")).is_err(),
            "never overwrites"
        );
        drop(w);
        assert!(is_encrypted(&copy));
        assert!(matches!(crate::check_file(&copy, None), Err(Error::Locked)));
        assert!(matches!(
            crate::check_file(&copy, Some("x")),
            Err(Error::Locked)
        ));
        crate::check_file(&copy, Some("k")).unwrap();
        assert_eq!(files(&dir), ["a.sqlite", "b.sqlite"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn two_connections_share_one_encrypted_save() {
        let dir = scratch("two");
        let path = dir.join("a.sqlite");
        drop(World::open(&path).unwrap());
        encrypt(&path, "k").unwrap();
        let app = World::open_with(&path, Some("k")).unwrap();
        let terminal = World::open_with(&path, Some("k")).unwrap();
        mark(&app, "from app");
        assert_eq!(marked(&terminal).as_deref(), Some("from app"));
        mark(&terminal, "from terminal");
        assert_eq!(marked(&app).as_deref(), Some("from terminal"));
        // While both are open, at most a transient journal sits beside the file.
        assert!(
            files(&dir)
                .iter()
                .all(|f| f == "a.sqlite" || f.ends_with("-journal"))
        );
        drop((app, terminal));
        assert_eq!(files(&dir), ["a.sqlite"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_save_left_in_wal_mode_by_an_older_build_becomes_one_file() {
        let dir = scratch("wal");
        let live = dir.join("live");
        let left = dir.join("left");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&left).unwrap();
        let c = Connection::open(live.join("a.sqlite")).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.execute_batch("CREATE TABLE mine(x); INSERT INTO mine VALUES (7);")
            .unwrap();
        // What a crashed older build leaves behind: the file with its log beside it.
        for f in files(&live) {
            std::fs::copy(live.join(&f), left.join(&f)).unwrap();
        }
        drop(c);
        assert_eq!(files(&left).len(), 3, "{:?}", files(&left));

        drop(World::open(left.join("a.sqlite")).unwrap());
        assert_eq!(files(&left), ["a.sqlite"]);
        let w = World::open(left.join("a.sqlite")).unwrap();
        let x: i64 = w.query_row("SELECT x FROM mine", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 7, "what was in the log is in the file");
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
