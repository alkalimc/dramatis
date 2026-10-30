-- World database, schema 5. Additive only.

-- The user's own picture, kept in the save so it travels with it and is encrypted with it.
-- One row at most.
CREATE TABLE user_avatar (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    mime TEXT NOT NULL,
    data BLOB NOT NULL
) STRICT;
