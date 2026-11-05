-- World database, schema 3. Additive only.

-- requests: 1 for a call recorded on a request-billed model (its points stay 0), else 0.
-- The hourly request window counts these; rows from schema 2 are token-billed.
ALTER TABLE meter ADD COLUMN requests INTEGER NOT NULL DEFAULT 0 CHECK (requests IN (0, 1));
