-- World database, schema 5. Additive only.

-- name: a title the user gave the channel (NULL: shown from its members). avatar: file name
-- of the user's picture for it, under the home's uploads directory (NULL: composed from the
-- members). archived: kept out of the main list; a person's new message clears it.
ALTER TABLE channel ADD COLUMN name TEXT;
ALTER TABLE channel ADD COLUMN avatar TEXT;
ALTER TABLE channel ADD COLUMN archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1));
