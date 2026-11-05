-- World database, schema 9: the whole current layout, created in one go for a new save.
--
-- Everything that changes with play lives here; everything that changes with the corpus
-- lives in the .folio. This database may cite the corpus (person_id, Citation); the
-- corpus never cites this database.
--
-- The app opens only a save at exactly this schema. A save from an earlier release is
-- brought here by `dramatis-migrate`, which keeps the steps that led to this layout; its
-- tests check that those steps and this file give the same tables. A change to the layout
-- edits this file, raises SCHEMA_VERSION in lib.rs and adds a step to the tool.
--
-- Conventions: STRICT tables; timestamps are INTEGER unix milliseconds; booleans are
-- INTEGER 0/1; enums are lowercase TEXT under a CHECK; JSON columns are TEXT under
-- json_valid. An actor is 'user', 'host' (the in-world assistant) or a person_id.
-- Foreign keys are enforced (the connection turns them on).

-- A relationship. a = 'user' or a person_id, b = a person_id. A person pair is stored
-- once, with a < b. The initial trust (user and person pairs differ) is written by
-- `world`, so there is no default here. Trust grows in fractions of a point while trust
-- is an integer: trust_exact accumulates the exact value and trust is its rounding.
CREATE TABLE bond (
    a           TEXT    NOT NULL,
    b           TEXT    NOT NULL,
    trust       INTEGER NOT NULL CHECK (trust BETWEEN 0 AND 200),
    mode        TEXT    NOT NULL DEFAULT 'frozen' CHECK (mode IN ('enabled', 'frozen', 'disabled')),
    last_seen   INTEGER,                -- latest message between the two; NULL = never
    trust_exact REAL    CHECK (trust_exact IS NULL OR trust_exact BETWEEN 0 AND 200),
    PRIMARY KEY (a, b),
    CHECK (a <> b AND a <> 'host'),
    CHECK (a = 'user' OR a < b),
    CHECK (b NOT IN ('user', 'host'))
) STRICT, WITHOUT ROWID;

-- A conversation. The user is implicitly in every channel; everyone else is listed in
-- channel_participant. log_segment is the segment currently appended to (NULL until the
-- first session opens).
--   read_up_to   the newest message id the user has seen here; NULL = none.
--   direct_with  whose direct channel this is (a person_id, or 'host' for the host's own);
--                a direct channel keeps that owner when a colleague is pulled in.
--   deleted_at   a deleted group keeps its row and participants, so memories scoped to it
--                stay visible to the people who were there; its messages and logs go.
--   name         a title the user gave the channel (NULL: shown from its members).
--   avatar       file name of the user's picture for it, under the home's uploads
--                directory (NULL: composed from the members).
--   archived     kept out of the main list; a person's new message clears it.
CREATE TABLE channel (
    id          INTEGER PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('direct', 'group', 'favorites')),
    origin      TEXT    NOT NULL CHECK (origin IN ('user', 'system')),
    topic       TEXT,
    mode        TEXT    NOT NULL DEFAULT 'frozen' CHECK (mode IN ('enabled', 'frozen', 'disabled')),
    log_segment INTEGER REFERENCES log_segment(id),
    read_up_to  INTEGER,
    direct_with TEXT    CHECK (direct_with IS NULL OR direct_with <> 'user'),
    deleted_at  INTEGER,
    name        TEXT,
    avatar      TEXT,
    archived    INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
) STRICT;
CREATE UNIQUE INDEX idx_channel_favorites ON channel(kind) WHERE kind = 'favorites';
CREATE UNIQUE INDEX idx_channel_direct_with ON channel(direct_with) WHERE direct_with IS NOT NULL;

-- Channel.participants. Membership is having been brought in; memory visibility for
-- audience = participants reads this table.
CREATE TABLE channel_participant (
    channel     INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    participant TEXT    NOT NULL CHECK (participant <> 'user'),   -- person_id or 'host'
    PRIMARY KEY (channel, participant)
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_channel_participant_participant ON channel_participant(participant);

-- What was said, as the user sees it.
CREATE TABLE message (
    id         INTEGER PRIMARY KEY,
    channel    INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    author     TEXT    NOT NULL,                                  -- actor
    text       TEXT    NOT NULL,
    tool_calls TEXT    CHECK (tool_calls IS NULL OR json_valid(tool_calls)),
    attachment TEXT    CHECK (attachment IS NULL OR json_valid(attachment)),
    at         INTEGER NOT NULL,
    reply_to   INTEGER REFERENCES message(id) ON DELETE SET NULL      -- the message this one answers
) STRICT;
CREATE INDEX idx_message_channel_at ON message(channel, at);
CREATE INDEX idx_message_author_at ON message(author, at);

-- Memory (L1, L2). audience is a hard filter applied when candidates are built:
--   world         audience_ref NULL
--   participants  audience_ref = channel id, in decimal
--   self          audience_ref = the person_id who may see it
-- about is optional: fact carries the id in decimal, person a person_id, user no
-- ref, citation a JSON Citation. Retraction is a flag so a deleted memory still
-- deduplicates future writes and can be restored. trust_delta is the trust change a hurt
-- applied (negative, after clamping at 0), so retracting it gives back exactly that.
-- Past `expires` (unix ms) a fact is swept away; NULL: it stays.
CREATE TABLE fact (
    id            INTEGER PRIMARY KEY,
    author        TEXT    NOT NULL,                               -- actor
    audience_kind TEXT    NOT NULL CHECK (audience_kind IN ('world', 'participants', 'self')),
    audience_ref  TEXT,
    kind          TEXT    NOT NULL CHECK (kind IN ('fact', 'commitment', 'hurt')),
    about_kind    TEXT    CHECK (about_kind IN ('fact', 'person', 'user', 'citation')),
    about_ref     TEXT,
    due           INTEGER,
    delivered     INTEGER NOT NULL DEFAULT 0 CHECK (delivered IN (0, 1)),
    -- How a commitment ended: brought up (kept) or left until it expired (lapsed).
    outcome       TEXT    CHECK (outcome IN ('kept', 'lapsed')),
    text          TEXT    NOT NULL,
    retracted     INTEGER NOT NULL DEFAULT 0 CHECK (retracted IN (0, 1)),
    created_at    INTEGER NOT NULL,
    trust_delta   REAL,
    expires       INTEGER,
    CHECK ((audience_kind = 'world') = (audience_ref IS NULL)),
    CHECK ((about_kind IS NULL OR about_kind = 'user') = (about_ref IS NULL)),
    CHECK (about_kind IS NOT 'citation' OR json_valid(about_ref))
) STRICT;
CREATE INDEX idx_fact_audience ON fact(audience_kind, audience_ref);
CREATE INDEX idx_fact_due ON fact(kind, delivered, due);

-- The model log: a cache of what was sent, not content. A build that would have sent other
-- bytes drops it and rebuilds from messages, summaries and memories. One segment per
-- rollover. prefix_a / prefix_b / prefix_c are the exact bytes of the A (global static),
-- B (personas) and C (opening facts) blocks, written once when the segment opens and
-- never rewritten. shape is the request shape that must stay byte-identical for the
-- segment's lifetime (JSON: model, tool list digest, reasoning). The segment id is what
-- prompt_cache_key is derived from. digest is the build/prompt digest the segment was
-- written under; a segment under another digest is dropped.
CREATE TABLE log_segment (
    id         INTEGER PRIMARY KEY,
    channel    INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL CHECK (seq >= 0),                 -- 0, 1, … within channel
    prefix_a   BLOB    NOT NULL,
    prefix_b   BLOB    NOT NULL,
    prefix_c   BLOB    NOT NULL,
    shape      TEXT    NOT NULL CHECK (json_valid(shape)),
    opened_at  INTEGER NOT NULL,
    digest     TEXT    NOT NULL DEFAULT '',
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

-- The summaries a rollover wrote, kept apart from the log so that dropping a segment
-- loses no memory of the conversation. A summary belongs to its channel and goes with it.
CREATE TABLE channel_summary (
    channel INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    at      INTEGER NOT NULL,
    text    TEXT    NOT NULL,
    PRIMARY KEY (channel, at)
) STRICT, WITHOUT ROWID;

-- One row per model call, from the endpoint's usage. Quota windows sum points over a
-- rolling range of at; points is fixed at record time with the prices then in force.
-- requests: 1 for a call recorded on a request-billed model (its points stay 0), else 0.
CREATE TABLE meter (
    id        INTEGER PRIMARY KEY,
    at        INTEGER NOT NULL,
    shape     TEXT    NOT NULL CHECK (shape IN (
                  'direct', 'group', 'interject', 'opening', 'host', 'wrapup')),
    channel   INTEGER REFERENCES channel(id) ON DELETE SET NULL,
    model     TEXT    NOT NULL,
    uncached  INTEGER NOT NULL CHECK (uncached >= 0),
    cached    INTEGER NOT NULL CHECK (cached >= 0),
    output    INTEGER NOT NULL CHECK (output >= 0),
    points    REAL    NOT NULL CHECK (points >= 0),
    requests  INTEGER NOT NULL DEFAULT 0 CHECK (requests IN (0, 1))
) STRICT;
CREATE INDEX idx_meter_at ON meter(at);

-- User settings, key -> JSON value; the keys the app reads are listed in `settings.rs`.
-- A missing key means the default.
CREATE TABLE settings (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL CHECK (json_valid(value))
) STRICT, WITHOUT ROWID;

-- One reason for one person to speak unprompted. Every delivered opening is recorded,
-- which is what the daily limits count and what "came by" lists. material is what the
-- opening's attachment carries (a JSON array of the chunk ids that matched), opaque here.
--   kind        ref
--   commitment  fact id      birthday    local year
--   memory      fact id      words       message id
--   event       (see event.rs)  visit    (see presence.rs)
CREATE TABLE seed (
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
CREATE INDEX idx_seed_delivered ON seed(delivered_at);

-- A triggered session: one person's stretch of conversation in one channel. Opened when
-- he is triggered, closed on idle, on close or when its budget runs out. A frozen person
-- with an open session is thawed; closing refreezes him. Budget: replies_left = replies he
-- may still give (a colleague pulled into an ordinary conversation answers once); NULL =
-- each reply is paid for by the user's own turn.
CREATE TABLE session (
    id           INTEGER PRIMARY KEY,
    channel      INTEGER NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    person       TEXT    NOT NULL CHECK (person NOT IN ('user', 'host')),
    cause        TEXT    NOT NULL CHECK (cause IN (
                     'addressed', 'join', 'commitment', 'opening')),
    replies_left INTEGER CHECK (replies_left IS NULL OR replies_left >= 0),
    opened_at    INTEGER NOT NULL,
    last_at      INTEGER NOT NULL,
    closed_at    INTEGER
) STRICT;
CREATE INDEX idx_session_open ON session(closed_at, channel);

-- The user's own picture, kept in the save so it travels with it and is encrypted with it.
-- One row at most.
CREATE TABLE user_avatar (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    mime TEXT NOT NULL,
    data BLOB NOT NULL
) STRICT;

-- Pictures the user attached to messages, kept in the save so they travel with it and are
-- encrypted with it. Keyed by content hash: the same picture sent twice is stored once.
-- A message names its picture in `message.attachment` (`picture`).
CREATE TABLE picture (
    hash TEXT PRIMARY KEY,
    mime TEXT NOT NULL,
    data BLOB NOT NULL
) STRICT;

-- A happening of the day, made up for one local day. `persons` are the people it
-- involves (JSON array of ids), `material` the corpus units it was drawn from, `fact` the
-- world-audience fact that tells everyone about it until the day ends, `message` the
-- host's post. hidden: a daily event the user deleted from the world drawer stays in the
-- table (it still counts toward the day's quota and its material still keeps the next
-- event from repeating it); only the drawer and the terminal's list leave it out.
CREATE TABLE daily_event (
    id       INTEGER PRIMARY KEY,
    day      TEXT    NOT NULL,                                    -- local date, YYYY-MM-DD
    at       INTEGER NOT NULL,
    title    TEXT    NOT NULL,
    text     TEXT    NOT NULL,
    persons  TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(persons) AND json_type(persons) = 'array'),
    material TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(material) AND json_type(material) = 'array'),
    fact     INTEGER REFERENCES fact(id) ON DELETE SET NULL,
    message  INTEGER REFERENCES message(id) ON DELETE SET NULL,
    hidden   INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX idx_daily_event_day ON daily_event(day);

-- Which versions last opened this save: app_version, pack (the corpus pack's name),
-- pack_version. Written by the app after a successful open.
CREATE TABLE save_meta (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
) STRICT, WITHOUT ROWID;
