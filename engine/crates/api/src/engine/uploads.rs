//! Pictures the user chose (their own, a channel's) and the per-channel title and archive
//! flag that go with them.

use sha2::{Digest, Sha256};
use world::{channel, settings};

use super::{Engine, io};
use crate::convert::{self, world_err};
use crate::error::{ApiError, ApiResult};
use crate::events::WorldChange;
use crate::media;
use crate::port::GroupChange;
use crate::types::*;
use crate::views::ImagePart;

impl Engine {
    /// Decode and store a picture under `uploads/`, named by its content hash so a repeat
    /// is free. Returns the file name.
    fn store_upload(&self, image: &ImagePart) -> ApiResult<String> {
        let invalid = || ApiError::Invalid {
            field: "image".into(),
        };
        let bytes = media::base64_decode(&image.data_base64).ok_or_else(invalid)?;
        if bytes.is_empty() || bytes.len() > media::UPLOAD_MAX {
            return Err(invalid());
        }
        let ext = media::image_ext(&bytes).ok_or_else(invalid)?;
        let stem: String = Sha256::digest(&bytes)[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let name = format!("{stem}.{ext}");
        let dir = self.home.uploads();
        let path = dir.join(&name);
        if !path.is_file() {
            std::fs::create_dir_all(&dir).map_err(io)?;
            let tmp = dir.join(format!(".{name}.part"));
            std::fs::write(&tmp, &bytes).map_err(io)?;
            std::fs::rename(&tmp, &path).map_err(io)?;
        }
        Ok(name)
    }

    /// The file for a stored upload name, if it is still there.
    pub(crate) fn upload_file(&self, name: Option<&str>) -> Option<MediaFile> {
        let path = self.home.uploads().join(name?);
        path.is_file().then(|| MediaFile {
            path: path.to_string_lossy().into_owned(),
        })
    }

    /// The user's own picture, else the corpus's player portrait. Their own comes out of
    /// the save as a `data:` URL, so it never sits in a file outside the save.
    pub(crate) async fn user_avatar_impl(&self) -> ApiResult<Option<MediaFile>> {
        if let Some((mime, bytes)) = settings::user_avatar(&self.w()).map_err(world_err)? {
            return Ok(Some(MediaFile {
                path: format!("data:{mime};base64,{}", media::base64_encode(&bytes)),
            }));
        }
        let Some(c) = &self.corpus else {
            return Ok(None);
        };
        match c.player_avatar_url() {
            Some(url) => self.cached_avatar("~player", Some(url)).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_avatar_impl(
        &self,
        image: Option<ImagePart>,
    ) -> ApiResult<Option<MediaFile>> {
        let invalid = || ApiError::Invalid {
            field: "image".into(),
        };
        let stored = match &image {
            Some(image) => {
                let bytes = media::base64_decode(&image.data_base64).ok_or_else(invalid)?;
                if bytes.is_empty() || bytes.len() > media::UPLOAD_MAX {
                    return Err(invalid());
                }
                // The type is what the bytes say, not what the caller claims.
                let ext = media::image_ext(&bytes).ok_or_else(invalid)?;
                Some((media::image_mime(ext), bytes))
            }
            None => None,
        };
        {
            let w = self.w();
            settings::set_user_avatar(&w, stored.as_ref().map(|(m, b)| (*m, b.as_slice())))
                .map_err(world_err)?;
        }
        self.user_avatar_impl().await
    }

    pub(crate) async fn rename_channel_impl(
        &self,
        ch: &ChannelId,
        name: Option<String>,
    ) -> ApiResult<()> {
        let id = convert::world_channel(ch)?;
        let (group, before) = {
            let found = channel::get(&self.w(), id).map_err(world_err)?;
            (found.kind == world::ChannelKind::Group, found.name)
        };
        channel::set_name(&self.w(), id, name.as_deref()).map_err(world_err)?;
        let after = channel::get(&self.w(), id).map_err(world_err)?.name;
        self.changed(vec![WorldChange::Channel { id: ch.clone() }]);
        if group && before != after {
            self.ports
                .agent
                .group_changed(ch.clone(), crate::port::GroupChange::Renamed(after))
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn set_topic_impl(
        &self,
        ch: &ChannelId,
        topic: Option<String>,
    ) -> ApiResult<()> {
        let id = convert::world_channel(ch)?;
        let changed = channel::set_topic(&self.w(), id, topic.as_deref()).map_err(world_err)?;
        if changed {
            self.changed(vec![WorldChange::Channel { id: ch.clone() }]);
            let after = channel::get(&self.w(), id).map_err(world_err)?.topic;
            self.ports
                .agent
                .group_changed(ch.clone(), GroupChange::Described(after))
                .await?;
        }
        Ok(())
    }

    pub(crate) fn set_channel_avatar_impl(
        &self,
        ch: &ChannelId,
        image: Option<ImagePart>,
    ) -> ApiResult<Option<MediaFile>> {
        let id = convert::world_channel(ch)?;
        channel::get(&self.w(), id).map_err(world_err)?;
        let name = image.as_ref().map(|i| self.store_upload(i)).transpose()?;
        channel::set_avatar(&self.w(), id, name.as_deref()).map_err(world_err)?;
        self.changed(vec![WorldChange::Channel { id: ch.clone() }]);
        Ok(self.upload_file(name.as_deref()))
    }

    pub(crate) fn archive_channel_impl(&self, ch: &ChannelId, archived: bool) -> ApiResult<()> {
        let id = convert::world_channel(ch)?;
        {
            let w = self.w();
            let found = channel::get(&w, id).map_err(world_err)?;
            if archived && found.is_host_channel() {
                return Err(ApiError::Invalid {
                    field: "channel".into(),
                });
            }
            channel::set_archived(&w, id, archived).map_err(world_err)?;
        }
        self.changed(vec![WorldChange::Channel { id: ch.clone() }]);
        Ok(())
    }
}
