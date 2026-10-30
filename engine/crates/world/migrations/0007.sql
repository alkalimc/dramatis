-- World database, schema 7. No data is lost.

-- A fact may end: past `expires` (unix ms) it is swept away. NULL: it stays.
ALTER TABLE fact ADD COLUMN expires INTEGER;

-- A happening of the day, made up for one local day. `persons` are the people it
-- involves (JSON array of ids), `material` the corpus units it was drawn from, `fact` the
-- world-audience fact that tells everyone about it until the day ends, `message` the
-- host's post.
CREATE TABLE daily_event (
    id       INTEGER PRIMARY KEY,
    day      TEXT    NOT NULL,                                    -- local date, YYYY-MM-DD
    at       INTEGER NOT NULL,
    title    TEXT    NOT NULL,
    text     TEXT    NOT NULL,
    persons  TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(persons) AND json_type(persons) = 'array'),
    material TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(material) AND json_type(material) = 'array'),
    fact     INTEGER REFERENCES fact(id) ON DELETE SET NULL,
    message  INTEGER REFERENCES message(id) ON DELETE SET NULL
) STRICT;
CREATE INDEX idx_daily_event_day ON daily_event(day);

-- Seeds gain the kinds `event` and `visit`. A CHECK cannot be altered, so the table is rebuilt.
CREATE TABLE seed_new (
    id           INTEGER PRIMARY KEY,
    person       TEXT    NOT NULL CHECK (person NOT IN ('user', 'host')),
    kind         TEXT    NOT NULL CHECK (kind IN (
                     'commitment', 'birthday', 'memory', 'words', 'event', 'visit')),
    ref          TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    delivered_at INTEGER,
    channel      INTEGER REFERENCES channel(id) ON DELETE SET NULL,
    message      INTEGER REFERENCES message(id) ON DELETE SET NULL,
    material     TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(material) AND json_type(material) = 'array'),
    UNIQUE (person, kind, ref)
) STRICT;
INSERT INTO seed_new SELECT id, person, kind, ref, created_at, delivered_at, channel, message, material FROM seed;
DROP TABLE seed;
ALTER TABLE seed_new RENAME TO seed;
CREATE INDEX idx_seed_delivered ON seed(delivered_at);
