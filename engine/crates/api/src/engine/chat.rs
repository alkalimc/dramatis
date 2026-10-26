//! Channels, history, groups, the commands that start a model turn, and read-aloud.

use world::{bond, channel, message, settings};

use super::{Engine, io};
use crate::convert::{self, world_err, world_person};
use crate::error::{ApiError, ApiResult};
use crate::events::WorldChange;
use crate::media;
use crate::meta::MessageMeta;
use crate::types::*;
use crate::views::*;

/// The most messages one `history` call returns.
const HISTORY_MAX: u32 = 500;

/// `text` without the scene-description spans: speech is read, stage directions are not.
fn spoken(text: &str, open: &str, close: &str) -> String {
    if open.is_empty() || close.is_empty() {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        match after.find(close) {
            Some(end) => rest = &after[end + close.len()..],
            // An unclosed marker swallows the rest of the line, as the UI renders it.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

impl Engine {
    pub(crate) fn message_view(&self, m: &message::Message, offset: i32) -> Message {
        let meta = MessageMeta::of(m.attachment.as_ref());
        Message {
            id: convert::message_id(m.id),
            channel: convert::channel_id(m.channel),
            author: convert::author(&m.author),
            text: m.text.clone(),
            actions: m
                .tool_calls
                .as_ref()
                .and_then(|a| serde_json::from_value(a.clone()).ok())
                .unwrap_or_default(),
            attachment: None,
            cites: meta.cites.iter().map(convert::citation).collect(),
            confidence: meta.confidence.map(convert::confidence),
            relayed: meta.relayed,
            task: meta.task.map(convert::task_id),
            note: meta.note.map(|n| MediaFile {
                path: self.home.office().join(n).to_string_lossy().into_owned(),
            }),
            at: convert::ts(m.at, offset),
        }
    }

    /// Channels with something in them, and the host's own; a direct channel opened and
    /// never written to is not a conversation yet.
    pub(crate) fn channels_impl(&self) -> ApiResult<Vec<ChannelSummary>> {
        let offset = self.now().offset_min;
        let w = self.w();
        let mut out = Vec::new();
        for ch in channel::list(&w).map_err(world_err)? {
            let last = message::last(&w, ch.id).map_err(world_err)?;
            let host = ch.is_host_channel();
            if last.is_none() && !host {
                continue;
            }
            let mode = match (&ch.kind, ch.person()) {
                (world::ChannelKind::Group, _) => ch.mode,
                (_, Some(p)) => bond::mode(&w, p).map_err(world_err)?,
                _ => world::Mode::Enabled,
            };
            out.push(ChannelSummary {
                id: convert::channel_id(ch.id),
                kind: match (host, ch.kind) {
                    (true, _) => ChannelKind::Host,
                    (_, world::ChannelKind::Direct) => ChannelKind::Direct,
                    (_, world::ChannelKind::Group) => ChannelKind::Group,
                },
                origin: convert::origin(ch.origin),
                participants: ch
                    .persons()
                    .iter()
                    .map(|p| self.person_ref(p.as_str()))
                    .collect(),
                topic: ch.topic.clone(),
                mode: convert::mode(mode),
                last_at: last.map(|m| convert::ts(m.at, offset)),
            });
        }
        Ok(out)
    }

    pub(crate) fn history_impl(
        &self,
        ch: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> ApiResult<Vec<Message>> {
        let ch = convert::world_channel(ch)?;
        let before = before.map(convert::world_message).transpose()?;
        let offset = self.now().offset_min;
        let w = self.w();
        channel::get(&w, ch).map_err(world_err)?;
        let rows =
            message::history(&w, ch, before, limit.clamp(1, HISTORY_MAX)).map_err(world_err)?;
        Ok(rows.iter().map(|m| self.message_view(m, offset)).collect())
    }

    pub(crate) fn open_direct_impl(&self, person: &PersonId) -> ApiResult<ChannelId> {
        self.require_available(person)?;
        let id = channel::direct(&self.w(), &world_person(person)).map_err(world_err)?;
        Ok(convert::channel_id(id))
    }

    pub(crate) fn host_channel_impl(&self) -> ApiResult<ChannelId> {
        let id = channel::host(&self.w()).map_err(world_err)?;
        Ok(convert::channel_id(id))
    }

    pub(crate) async fn send_message_impl(
        &self,
        ch: ChannelId,
        text: String,
        image: Option<ImagePart>,
    ) -> ApiResult<MessageId> {
        if text.trim().is_empty() && image.is_none() {
            return Err(ApiError::Invalid {
                field: "text".into(),
            });
        }
        let found = {
            let id = convert::world_channel(&ch)?;
            channel::get(&self.w(), id).map_err(world_err)?
        };
        if !found.is_open() {
            return Err(ApiError::Invalid {
                field: "channel".into(),
            });
        }
        if let Some(p) = found.person() {
            self.require_available(&PersonId(p.0.clone()))?;
        }
        if !self.has_chat() {
            return Err(ApiError::NoEndpoint);
        }
        self.ports.agent.send_message(ch, text, image).await
    }

    pub(crate) async fn relay_impl(
        &self,
        excerpt: String,
        from: MessageId,
        to: PersonId,
    ) -> ApiResult<ChannelId> {
        if excerpt.trim().is_empty() {
            return Err(ApiError::Invalid {
                field: "excerpt".into(),
            });
        }
        message::get(&self.w(), convert::world_message(&from)?).map_err(world_err)?;
        self.require_available(&to)?;
        if !self.has_chat() {
            return Err(ApiError::NoEndpoint);
        }
        self.ports.agent.relay(excerpt, from, to).await
    }

    pub(crate) fn mark_read_impl(&self, ch: &ChannelId, up_to: &MessageId) -> ApiResult<()> {
        channel::mark_read(
            &self.w(),
            convert::world_channel(ch)?,
            convert::world_message(up_to)?,
        )
        .map_err(world_err)
    }

    pub(crate) fn create_group_impl(
        &self,
        members: Vec<PersonId>,
        topic: Option<String>,
    ) -> ApiResult<ChannelId> {
        let mut unique: Vec<PersonId> = Vec::new();
        for m in members {
            if !unique.contains(&m) {
                unique.push(m);
            }
        }
        if unique.is_empty() {
            return Err(ApiError::Invalid {
                field: "members".into(),
            });
        }
        for m in &unique {
            self.require_available(m)?;
        }
        let members: Vec<world::PersonId> = unique.iter().map(world_person).collect();
        let topic = topic.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        let id = channel::create_group(&self.w(), &members, topic.as_deref(), world::Origin::User)
            .map_err(world_err)?;
        let id = convert::channel_id(id);
        self.changed(vec![WorldChange::Channel { id: id.clone() }]);
        Ok(id)
    }

    pub(crate) fn delete_group_impl(&self, ch: &ChannelId) -> ApiResult<()> {
        let id = convert::world_channel(ch)?;
        let now = self.now().ms;
        {
            let w = self.w();
            if channel::get(&w, id).map_err(world_err)?.kind != world::ChannelKind::Group {
                return Err(ApiError::Invalid {
                    field: "channel".into(),
                });
            }
            channel::delete_group(&w, id, now).map_err(world_err)?;
        }
        self.changed(vec![WorldChange::Channel { id: ch.clone() }]);
        Ok(())
    }

    // ---- read-aloud ----

    fn tts_role(&self) -> ApiResult<Option<(crate::endpoints::Profile, String)>> {
        let endpoints = self.current_endpoints();
        let Some(role) = endpoints.roles.clone().and_then(|r| r.tts) else {
            return Ok(None);
        };
        let profile = endpoints
            .profile(&role.profile)
            .cloned()
            .ok_or(ApiError::NotFound)?;
        Ok(Some((profile, role.model)))
    }

    /// One message's speech, from the cache when it was synthesised before.
    pub(crate) async fn tts_play_impl(&self, id: &MessageId) -> ApiResult<MediaFile> {
        let (profile, model) = self.tts_role()?.ok_or(ApiError::Invalid {
            field: "tts".into(),
        })?;
        let (text, voice) = {
            let w = self.w();
            let m = message::get(&w, convert::world_message(id)?).map_err(world_err)?;
            let voice = match m.author.person() {
                Some(p) => settings::voice(&w, p).map_err(world_err)?,
                None => None,
            };
            (m.text, voice)
        };
        let (open, close) = self.corpus.as_ref().map_or_else(
            || (String::new(), String::new()),
            |c| c.manifest.scene_marker.clone(),
        );
        let text = spoken(&text, &open, &close);
        if text.trim().is_empty() {
            return Err(ApiError::Invalid {
                field: "message".into(),
            });
        }
        let stem = media::stem(&[&profile.name, &model, voice.as_deref().unwrap_or(""), &text]);
        let file = |p: std::path::PathBuf| MediaFile {
            path: p.to_string_lossy().into_owned(),
        };
        if let Some(path) = self.media.get("tts", &stem) {
            return Ok(file(path));
        }
        let speech = self
            .ports
            .llm
            .speech(&profile, &model, voice.as_deref(), &text)
            .await?;
        let path = self
            .media
            .put("tts", &stem, &speech.ext, &speech.bytes)
            .map_err(io)?;
        Ok(file(path))
    }

    pub(crate) async fn tts_voices_impl(&self) -> ApiResult<Vec<String>> {
        match self.tts_role()? {
            Some((profile, model)) => self.ports.llm.voices(&profile, &model).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) fn set_voice_impl(&self, person: &PersonId, voice: Option<String>) -> ApiResult<()> {
        if self.corpus()?.person(&person.0).is_none() {
            return Err(ApiError::NotFound);
        }
        let key = settings::voice_key(&world_person(person));
        let w = self.w();
        match voice.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) {
            Some(v) => settings::set(&w, &key, &v),
            None => settings::unset(&w, &key),
        }
        .map_err(world_err)
    }
}

#[cfg(test)]
mod tests {
    use super::spoken;

    #[test]
    fn scene_spans_are_not_read() {
        assert_eq!(
            spoken("*waves* Hello. *sits* Well.", "*", "*"),
            " Hello.  Well."
        );
        assert_eq!(spoken("Hi *unclosed", "*", "*"), "Hi ");
        assert_eq!(spoken("(a) b", "(", ")"), " b");
        assert_eq!(spoken("plain", "", ""), "plain");
    }
}
