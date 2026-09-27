-- World database, schema 1.
--
-- Everything that changes with play lives here; everything that changes with the corpus
-- lives in the .folio. This database may cite the corpus (person_id, Citation); the
-- corpus never cites this database.
--
-- Conventions: STRICT tables; timestamps are INTEGER unix milliseconds; booleans are
-- INTEGER 0/1; enums are lowercase TEXT under a CHECK; JSON columns are TEXT under
-- json_valid. An actor is 'user', 'host' (the in-world assistant) or a person_id.
-- Foreign keys are enforced (the connection turns them on).

-- A relationship. a = 'user' or a person_id, b = a person_id. A person pair is stored
-- once, with a < b. The initial trust (user and person pairs differ) is written by
-- `world`, so there is no default here.
CREATE TABLE bond (
    a         TEXT    NOT NULL,
    b         TEXT    NOT NULL,
    trust     INTEGER NOT NULL CHECK (trust BETWEEN 0 AND 200),
    mode      TEXT    NOT NULL DEFAULT 'frozen' CHECK (mode IN ('enabled', 'frozen', 'disabled')),
    last_seen INTEGER,                -- latest message between the two; NULL = never
    PRIMARY KEY (a, b),
    CHECK (a <> b AND a <> 'host'),
    CHECK (a = 'user' OR a < b),
    CHECK (b NOT IN ('user', 'host'))
) STRICT, WITHOUT ROWID;

-- A conversation. The user is implicitly in every channel; everyone else is listed in
-- channel_participant. log_segment is the segment currently appended to (NULL until the
-- first session opens).
CREATE TABLE channel (
    id          INTEGER PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('direct', 'group')),
    origin      TEXT    NOT NULL CHECK (origin IN ('user', 'system')),
    topic       TEXT,
    mode        TEXT    NOT NULL DEFAULT 'frozen' CHECK (mode IN ('enabled', 'frozen', 'disabled')),
    log_segment INTEGER REFERENCES log_segment(id)
) STRICT;

-- Channel.participants. Membership is having been brought in; memory visibility for
-- audience = participants reads this table.
CREATE TABLE channel_participant (
    channel     INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    participant TEXT    NOT NULL CHECK (participant <> 'user'),   -- person_id or 'host'
    PRIMARY KEY (channel, participant)
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_channel_participant_participant ON channel_participant(participant);

-- What was said, as the user sees it. The model-facing bytes live in log_entry.
CREATE TABLE message (
    id         INTEGER PRIMARY KEY,
    channel    INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    author     TEXT    NOT NULL,                                  -- actor
    text       TEXT    NOT NULL,
    tool_calls TEXT    CHECK (tool_calls IS NULL OR json_valid(tool_calls)),
    attachment TEXT    CHECK (attachment IS NULL OR json_valid(attachment)),
    at         INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_message_channel_at ON message(channel, at);
CREATE INDEX idx_message_author_at ON message(author, at);

-- A delegated question. Children (parent = id) come from request_join; the sum of
-- turns_left over one tree never exceeds what the root was given (enforced by `world`).
-- A pinned task may have no assignee and no channel: it is a question waiting for
-- someone. cites is a JSON array of Citation
-- {page, revid, span_from, span_to, chunk_id}.
CREATE TABLE task (
    id         INTEGER PRIMARY KEY,
    question   TEXT    NOT NULL,
    asker      TEXT    NOT NULL DEFAULT 'user' CHECK (asker = 'user'),
    assignee   TEXT,                                              -- person_id
    parent     INTEGER REFERENCES task(id),
    turns_left INTEGER NOT NULL CHECK (turns_left >= 0),
    status     TEXT    NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'done')),
    pinned     INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    cites      TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(cites) AND json_type(cites) = 'array'),
    note_path  TEXT,                                              -- relative to the office dir
    channel    INTEGER REFERENCES channel(id)
) STRICT;
CREATE INDEX idx_task_parent ON task(parent);
CREATE INDEX idx_task_assignee_status ON task(assignee, status);

-- Memory (L1, L2). audience is a hard filter applied when candidates are built:
--   world         audience_ref NULL
--   participants  audience_ref = channel id, in decimal
--   self          audience_ref = the person_id who may see it
-- about is optional: task / fact carry the id in decimal, person a person_id, user no
-- ref, citation a JSON Citation. Retraction is a flag so a deleted memory still
-- deduplicates future writes and can be restored.
CREATE TABLE fact (
    id            INTEGER PRIMARY KEY,
    author        TEXT    NOT NULL,                               -- actor
    audience_kind TEXT    NOT NULL CHECK (audience_kind IN ('world', 'participants', 'self')),
    audience_ref  TEXT,
    kind          TEXT    NOT NULL CHECK (kind IN ('fact', 'commitment', 'conclusion', 'hurt')),
    about_kind    TEXT    CHECK (about_kind IN ('task', 'fact', 'person', 'user', 'citation')),
    about_ref     TEXT,
    due           INTEGER,
    delivered     INTEGER NOT NULL DEFAULT 0 CHECK (delivered IN (0, 1)),
    text          TEXT    NOT NULL,
    retracted     INTEGER NOT NULL DEFAULT 0 CHECK (retracted IN (0, 1)),
    created_at    INTEGER NOT NULL,
    CHECK ((audience_kind = 'world') = (audience_ref IS NULL)),
    CHECK ((about_kind IS NULL OR about_kind = 'user') = (about_ref IS NULL)),
    CHECK (about_kind IS NOT 'citation' OR json_valid(about_ref))
) STRICT;
CREATE INDEX idx_fact_audience ON fact(audience_kind, audience_ref);
CREATE INDEX idx_fact_due ON fact(kind, delivered, due);

-- The append-only session log, one segment per rollover. prefix_a / prefix_b / prefix_c
-- are the exact bytes of the A (global static), B (personas) and C (opening facts)
-- blocks, written once when the segment opens and never rewritten. shape is the request
-- shape that must stay byte-identical for the segment's lifetime (JSON: model, tool
-- list digest, reasoning). The segment id is what prompt_cache_key is derived from.
CREATE TABLE log_segment (
    id         INTEGER PRIMARY KEY,
    channel    INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL CHECK (seq >= 0),                 -- 0, 1, … within channel
    prefix_a   BLOB    NOT NULL,
    prefix_b   BLOB    NOT NULL,
    prefix_c   BLOB    NOT NULL,
    shape      TEXT    NOT NULL CHECK (json_valid(shape)),
    opened_at  INTEGER NOT NULL,
    UNIQUE (channel, seq)
) STRICT;

-- Appended after the prefix, in order. bytes are exactly what was sent or received.
CREATE TABLE log_entry (
    segment INTEGER NOT NULL REFERENCES log_segment(id) ON DELETE CASCADE,
    seq     INTEGER NOT NULL CHECK (seq >= 0),
    role    TEXT    NOT NULL CHECK (role IN ('user', 'assistant', 'tool')),
    bytes   BLOB    NOT NULL,
    at      INTEGER NOT NULL,
    PRIMARY KEY (segment, seq)
) STRICT, WITHOUT ROWID;

-- One row per model call, from the endpoint's usage. Quota windows sum points over a
-- rolling range of at; points is fixed at record time with the prices then in force.
CREATE TABLE meter (
    id        INTEGER PRIMARY KEY,
    at        INTEGER NOT NULL,
    shape     TEXT    NOT NULL CHECK (shape IN (
                  'direct', 'group', 'interject', 'opening', 'ask', 'host', 'wrapup')),
    channel   INTEGER REFERENCES channel(id) ON DELETE SET NULL,
    model     TEXT    NOT NULL,
    uncached  INTEGER NOT NULL CHECK (uncached >= 0),
    cached    INTEGER NOT NULL CHECK (cached >= 0),
    output    INTEGER NOT NULL CHECK (output >= 0),
    points    REAL    NOT NULL CHECK (points >= 0)
) STRICT;
CREATE INDEX idx_meter_at ON meter(at);

-- User settings, key -> JSON value. Known keys:
--   user.name            string
--   user.birthday        "MM-DD" or null
--   intensity            "low" | "middle" | "high" | "extra_high" | "max" | "ultra"
--   quiet_hours          {"from": "HH:MM", "to": "HH:MM"}, user local time
--   notifications        bool
--   voice.<person_id>    string, the read-aloud voice id
-- A missing key means the default.
CREATE TABLE settings (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL CHECK (json_valid(value))
) STRICT, WITHOUT ROWID;
