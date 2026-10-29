//! Pictures attached to messages, stored in the save by content hash.

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::Result;

/// Store a picture (once per distinct content) and return its hash.
pub fn put(conn: &Connection, mime: &str, data: &[u8]) -> Result<String> {
    let hash: String = Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    conn.execute(
        "INSERT OR IGNORE INTO picture(hash, mime, data) VALUES (?1, ?2, ?3)",
        (&hash, mime, data),
    )?;
    Ok(hash)
}

/// The MIME type and bytes stored under `hash`.
pub fn get(conn: &Connection, hash: &str) -> Result<Option<(String, Vec<u8>)>> {
    Ok(conn
        .query_row(
            "SELECT mime, data FROM picture WHERE hash = ?1",
            [hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Delete pictures no message names any more; returns how many went.
pub fn drop_orphans(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM picture WHERE hash NOT IN (
             SELECT json_extract(attachment, '$.picture') FROM message
             WHERE attachment IS NOT NULL AND json_extract(attachment, '$.picture') IS NOT NULL)",
        [],
    )?)
}

/// How many pictures the save holds and the bytes they take.
pub fn usage(conn: &Connection) -> Result<(u64, u64)> {
    Ok(conn.query_row(
        "SELECT count(*), coalesce(sum(length(data)), 0) FROM picture",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message;
    use crate::testing::*;
    use crate::{Actor, channel};

    #[test]
    fn stores_once_and_drops_only_unreferenced() {
        let w = world();
        let c = channel::direct(&w, &p("a")).unwrap();
        let a = put(&w, "image/png", &[1, 2, 3]).unwrap();
        assert_eq!(put(&w, "image/png", &[1, 2, 3]).unwrap(), a);
        let b = put(&w, "image/png", &[9]).unwrap();
        assert_eq!(usage(&w).unwrap(), (2, 4));
        assert_eq!(
            get(&w, &a).unwrap(),
            Some(("image/png".into(), vec![1, 2, 3]))
        );
        let meta = serde_json::json!({ "picture": a });
        message::append(&w, c, &Actor::User, "x", None, Some(&meta), 5).unwrap();
        assert_eq!(drop_orphans(&w).unwrap(), 1);
        assert!(get(&w, &a).unwrap().is_some() && get(&w, &b).unwrap().is_none());
    }
}
