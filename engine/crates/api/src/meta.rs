//! What `message.attachment` holds for a message the engine wrote: the reply's
//! citations, how sure retrieval was, whether it retells something. One type, so the side
//! that writes it (the agent) and the side that shows it (the history) cannot drift apart.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageMeta {
    /// The passages the reply cites, in order.
    pub cites: Vec<world::Citation>,
    /// How sure retrieval was about the material the reply had.
    pub confidence: Option<index::Level>,
    /// A retelling of something said elsewhere.
    pub relayed: bool,
    /// A context line written by the harness, shown as a service line.
    pub service: bool,
    /// Hash of the picture the user attached, which lives in the save (`world::picture`).
    pub picture: Option<String>,
    /// The picture is gone and cannot be shown.
    pub picture_lost: bool,
    /// A picture an older build kept as a file at this path; moved into the save on first
    /// sight (or marked lost when the file is gone).
    pub file: Option<String>,
}

impl MessageMeta {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The meta a stored message carries; none (or unreadable) is the default.
    pub fn of(attachment: Option<&serde_json::Value>) -> Self {
        attachment
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
}

/// A message's picture for the UI: a `data:` URL, and whether it was lost. A picture still
/// named by file is moved into the save here, once.
pub fn picture_of(
    conn: &rusqlite::Connection,
    m: &world::message::Message,
) -> (Option<crate::types::MediaFile>, bool) {
    let mut meta = MessageMeta::of(m.attachment.as_ref());
    if let Some(path) = meta.file.take() {
        let found = std::fs::metadata(&path)
            .is_ok_and(|f| f.len() <= crate::media::UPLOAD_MAX as u64)
            .then(|| std::fs::read(&path).ok())
            .flatten()
            .and_then(|b| crate::media::image_ext(&b).map(|e| (crate::media::image_mime(e), b)));
        match found.and_then(|(mime, b)| world::picture::put(conn, mime, &b).ok()) {
            Some(hash) => meta.picture = Some(hash),
            None => meta.picture_lost = true,
        }
        if let Ok(v) = serde_json::to_value(&meta) {
            let _ = world::message::set_attachment(conn, m.id, &v);
        }
    }
    if meta.picture_lost {
        return (None, true);
    }
    let Some(hash) = &meta.picture else {
        return (None, false);
    };
    match world::picture::get(conn, hash) {
        Ok(Some((mime, bytes))) => (
            Some(crate::types::MediaFile {
                path: format!("data:{mime};base64,{}", crate::media::base64_encode(&bytes)),
            }),
            false,
        ),
        _ => (None, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_writer_stores() {
        let v = serde_json::json!({
            "cites": [{"page": "P", "revid": 3, "span_from": 1, "span_to": 2, "chunk_id": "dialogue:x"}],
            "confidence": "low",
        });
        let m = MessageMeta::of(Some(&v));
        assert_eq!(m.cites[0].chunk_id, "dialogue:x");
        assert_eq!(m.confidence, Some(index::Level::Low));
        assert!(!m.relayed);
        assert!(MessageMeta::of(Some(&serde_json::json!("junk"))).is_empty());
        assert!(MessageMeta::of(None).is_empty());
    }
}
