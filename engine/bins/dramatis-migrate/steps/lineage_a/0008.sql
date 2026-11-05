-- World database, schema 8. Additive only.

-- Pictures the user attached to messages, kept in the save so they travel with it and are
-- encrypted with it. Keyed by content hash: the same picture sent twice is stored once.
-- A message names its picture in `message.attachment` (`picture`).
CREATE TABLE picture (
    hash TEXT PRIMARY KEY,
    mime TEXT NOT NULL,
    data BLOB NOT NULL
) STRICT;
