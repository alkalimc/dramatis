-- World database, schema 8. Additive only.
--
-- A save holds generated content. Prompts and context assembly change with the build, so
-- what the model log stores of them is a cache that the reader may drop.

-- Which versions last opened this save: app_version, pack (the corpus pack's name),
-- pack_version. Written by the app after a successful open; absent in older saves.
CREATE TABLE save_meta (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
) STRICT, WITHOUT ROWID;

-- The build/prompt digest a segment was written under. A segment whose digest is not the
-- running build's is dropped and rebuilt from messages; '' (a segment of an older save)
-- never matches.
ALTER TABLE log_segment ADD COLUMN digest TEXT NOT NULL DEFAULT '';

-- The summaries a rollover wrote, which until now lived only inside the segment's prefix.
-- Kept apart from the log so that dropping a segment loses no memory of the conversation.
-- A summary belongs to its channel and goes with it.
CREATE TABLE channel_summary (
    channel INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    at      INTEGER NOT NULL,
    text    TEXT    NOT NULL,
    PRIMARY KEY (channel, at)
) STRICT, WITHOUT ROWID;
