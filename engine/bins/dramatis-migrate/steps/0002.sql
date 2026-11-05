-- World database, schema 2. Additive only: new columns and one new table.

-- Trust grows in fractions of a point (a shared turn adds less than one) while trust is
-- an integer: the exact value accumulates here and trust is its rounding. NULL (rows from
-- schema 1) means exactly trust.
ALTER TABLE bond ADD COLUMN trust_exact REAL
    CHECK (trust_exact IS NULL OR trust_exact BETWEEN 0 AND 200);

-- read_up_to: the newest message id the user has seen here; NULL = none.
-- direct_with: whose direct channel this is (a person_id, or 'host' for the host's own).
-- A direct channel keeps that owner when a colleague is pulled in; NULL on schema-1 rows,
-- where the sole participant is the owner.
-- deleted_at: a deleted group keeps its row and participants, so memories scoped to it
-- stay visible to the people who were there; its messages and logs are removed.
ALTER TABLE channel ADD COLUMN read_up_to INTEGER;
ALTER TABLE channel ADD COLUMN direct_with TEXT CHECK (direct_with IS NULL OR direct_with <> 'user');
ALTER TABLE channel ADD COLUMN deleted_at INTEGER;
CREATE UNIQUE INDEX idx_channel_direct_with ON channel(direct_with) WHERE direct_with IS NOT NULL;

-- The trust change a hurt applied (negative, after clamping at 0), so retracting it gives
-- back exactly that and restoring takes it again.
ALTER TABLE fact ADD COLUMN trust_delta REAL;

-- One reason for one person to speak unprompted. Every delivered opening is recorded,
-- which is what the daily limits count and what "came by" lists. material is what the
-- opening's attachment carries (a JSON array of the chunk ids that matched), opaque here.
--   kind        ref
--   commitment  fact id      birthday    local year
--   memory      fact id      words       message id
CREATE TABLE seed (
    id           INTEGER PRIMARY KEY,
    person       TEXT    NOT NULL CHECK (person NOT IN ('user', 'host')),
    kind         TEXT    NOT NULL CHECK (kind IN (
                     'commitment', 'birthday', 'memory', 'words')),
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
