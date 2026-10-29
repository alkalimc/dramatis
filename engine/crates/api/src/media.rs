//! The bounded local media cache: avatars and read-aloud audio under `$DRAMATIS_HOME/media/`.
//!
//! Nothing here is ever archived or distributed; every file can be fetched or synthesised
//! again, so eviction is plain least-recently-used by modification time, which a hit
//! refreshes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct MediaCache {
    root: PathBuf,
    limit: u64,
}

/// A short stable file stem for a cache key.
pub fn stem(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    h.finalize()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The extension for image bytes by their magic number; `None` for anything else (an
/// error page served with a 200 is not an avatar).
pub fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("jpg"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("webp"),
        _ => None,
    }
}

/// The MIME type for an extension [`image_ext`] returned.
pub fn image_mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "gif" => "image/gif",
        _ => "image/webp",
    }
}

/// Largest picture a user may set; the UI downsizes first, so this only stops abuse.
pub const UPLOAD_MAX: usize = 8 << 20;

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64, padding optional, whitespace ignored. `None` on any other byte.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl MediaCache {
    pub fn new(root: impl Into<PathBuf>, limit_bytes: u64) -> Self {
        Self {
            root: root.into(),
            limit: limit_bytes,
        }
    }

    /// A cached file `dir/stem.*`, touched so it counts as recently used.
    pub fn get(&self, dir: &str, stem: &str) -> Option<PathBuf> {
        let entries = std::fs::read_dir(self.root.join(dir)).ok()?;
        let found = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.file_stem().is_some_and(|s| s == stem) && p.is_file())?;
        touch(&found);
        Some(found)
    }

    /// Store `dir/stem.ext`, then evict down to the limit (never the file just written).
    pub fn put(&self, dir: &str, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let dir = self.root.join(dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{stem}.{ext}"));
        let tmp = dir.join(format!(".{stem}.{ext}.part"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.evict(&path);
        Ok(path)
    }

    /// Bytes and file count under `dir` (a cache directory such as `avatar`).
    pub fn usage(&self, dir: &str) -> (u64, u32) {
        let mut files = Vec::new();
        collect(&self.root.join(dir), &mut files);
        (
            files.iter().map(|(_, len, _)| len).sum(),
            u32::try_from(files.len()).unwrap_or(u32::MAX),
        )
    }

    /// Delete every file under `dir`; returns the bytes freed. Only cache files live here,
    /// so nothing is lost that cannot be fetched or synthesised again.
    pub fn clear(&self, dir: &str) -> u64 {
        let mut files = Vec::new();
        collect(&self.root.join(dir), &mut files);
        files
            .into_iter()
            .filter(|(path, _, _)| std::fs::remove_file(path).is_ok())
            .map(|(_, len, _)| len)
            .sum()
    }

    fn evict(&self, keep: &Path) {
        let mut files = Vec::new();
        collect(&self.root, &mut files);
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        if total <= self.limit {
            return;
        }
        files.sort_by_key(|(_, _, at)| *at);
        for (path, len, _) in files {
            if total <= self.limit {
                break;
            }
            if path == keep {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

fn collect(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(Result::ok) {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            collect(&path, out);
        } else if meta.is_file() {
            let at = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((path, meta.len(), at));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn least_recently_used_goes_first() {
        let root = std::env::temp_dir().join(format!("dramatis-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = MediaCache::new(&root, 25);
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        let a = cache.put("tts", "a", "mp3", &[1; 10]).unwrap();
        let b = cache.put("avatar", "b", "png", &[2; 10]).unwrap();
        for p in [&a, &b] {
            std::fs::File::options()
                .append(true)
                .open(p)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        assert_eq!(cache.get("tts", "a"), Some(a.clone()), "a hit refreshes a");
        let c = cache.put("tts", "c", "mp3", &[3; 10]).unwrap();
        assert!(a.exists() && c.exists());
        assert!(!b.exists(), "b was the least recently used");
        assert_eq!(cache.get("avatar", "b"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn usage_counts_and_clear_empties_one_directory() {
        let root = std::env::temp_dir().join(format!("dramatis-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = MediaCache::new(&root, u64::MAX);
        cache.put("tts", "a", "mp3", &[1; 10]).unwrap();
        cache.put("tts", "b", "mp3", &[1; 5]).unwrap();
        let keep = cache.put("avatar", "c", "png", &[2; 7]).unwrap();
        assert_eq!(cache.usage("tts"), (15, 2));
        assert_eq!(cache.usage("none"), (0, 0));
        assert_eq!(cache.clear("tts"), 15);
        assert_eq!(cache.usage("tts"), (0, 0));
        assert!(keep.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn base64_round_trips() {
        for sample in [&b""[..], b"h", b"hi", b"hel", b"hello", &[0, 255, 254, 1]] {
            assert_eq!(base64_decode(&base64_encode(sample)).unwrap(), sample);
        }
        assert_eq!(base64_encode(b"hi"), "aGk=");
        assert_eq!(base64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(base64_decode("aGk").unwrap(), b"hi");
        assert_eq!(base64_decode("aG\nVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("").unwrap(), b"");
        assert!(base64_decode("a*b").is_none());
    }

    #[test]
    fn images_by_magic_and_stable_stems() {
        assert_eq!(image_ext(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(image_ext(b"<!DOCTYPE html>"), None);
        assert_eq!(stem(&["a", "b"]), stem(&["a", "b"]));
        assert_ne!(stem(&["ab", ""]), stem(&["a", "b"]));
        assert_eq!(stem(&["x"]).len(), 24);
    }
}
