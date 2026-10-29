//! What the app keeps on disk, and emptying the caches among it.

use std::path::Path;

use world::picture;

use super::Engine;
use crate::convert::world_err;
use crate::error::{ApiError, ApiResult};
use crate::views::*;

/// Kilobytes, rounded up so a small non-empty cache never reads as empty.
fn kb(bytes: u64) -> u32 {
    u32::try_from(bytes.div_ceil(1024)).unwrap_or(u32::MAX)
}

/// Bytes and file count of everything under `dir`.
fn tree(dir: &Path) -> (u64, u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let (mut bytes, mut files) = (0, 0);
    for e in entries.filter_map(Result::ok) {
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            let (b, f) = tree(&e.path());
            bytes += b;
            files += f;
        } else if meta.is_file() {
            bytes += meta.len();
            files += 1;
        }
    }
    (bytes, files)
}

impl Engine {
    pub(crate) fn storage_usage_impl(&self) -> ApiResult<StorageUsage> {
        let open = self.save.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // A save is its database file and the side files SQLite keeps beside it.
        let mut sizes: Vec<(String, u64)> = Vec::new();
        for e in std::fs::read_dir(self.home.saves())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let file = e.file_name().to_string_lossy().into_owned();
            let Some((name, _)) = file.split_once(".sqlite") else {
                continue;
            };
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            match sizes.iter_mut().find(|s| s.0 == name) {
                Some(s) => s.1 += meta.len(),
                None => sizes.push((name.to_owned(), meta.len())),
            }
        }
        sizes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let saves_bytes: u64 = sizes.iter().map(|s| s.1).sum();

        let (avatar_bytes, avatar_files) = self.media.usage("avatar");
        let (audio_bytes, audio_files) = self.media.usage("tts");
        let (upload_bytes, upload_files) = tree(&self.home.uploads());
        let corpus = self
            .home
            .resolve_folio()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len());
        let (pictures, picture_bytes) = picture::usage(&self.w()).map_err(world_err)?;

        let categories = vec![
            StorageCategory {
                kind: StorageKind::Saves,
                kb: kb(saves_bytes),
                files: u32::try_from(sizes.len()).unwrap_or(u32::MAX),
                clearable: false,
            },
            StorageCategory {
                kind: StorageKind::Avatars,
                kb: kb(avatar_bytes),
                files: avatar_files,
                clearable: true,
            },
            StorageCategory {
                kind: StorageKind::Audio,
                kb: kb(audio_bytes),
                files: audio_files,
                clearable: true,
            },
            StorageCategory {
                kind: StorageKind::Uploads,
                kb: kb(upload_bytes),
                files: upload_files,
                clearable: false,
            },
            StorageCategory {
                kind: StorageKind::Corpus,
                kb: kb(corpus),
                files: u32::from(corpus > 0),
                clearable: false,
            },
        ];
        Ok(StorageUsage {
            total_kb: categories.iter().map(|c| c.kb).sum(),
            categories,
            saves: sizes
                .into_iter()
                .map(|(name, bytes)| SaveSize {
                    current: name == open,
                    name,
                    kb: kb(bytes),
                })
                .collect(),
            pictures: u32::try_from(pictures).unwrap_or(u32::MAX),
            picture_kb: kb(picture_bytes),
            cache_limit_kb: kb(self.params.media.cache_bytes()),
        })
    }

    pub(crate) fn clear_storage_impl(&self, kinds: &[StorageKind]) -> ApiResult<StorageUsage> {
        // Refuse the whole request before deleting anything.
        if kinds
            .iter()
            .any(|k| !matches!(k, StorageKind::Avatars | StorageKind::Audio))
        {
            return Err(ApiError::Invalid {
                field: "categories".into(),
            });
        }
        for k in kinds {
            self.media.clear(match k {
                StorageKind::Avatars => "avatar",
                _ => "tts",
            });
        }
        self.storage_usage_impl()
    }
}
