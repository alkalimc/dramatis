-- World database, schema 4. Additive only.

-- failure: why a request ended without an answer ('failed': its run errored or never
-- started; 'interrupted': the app stopped while it was open). Such a request is 'done'
-- with no reply. failure_told: the host has been shown the failure once.
ALTER TABLE task ADD COLUMN failure TEXT CHECK (failure IS NULL OR failure IN ('failed', 'interrupted'));
ALTER TABLE task ADD COLUMN failure_told INTEGER NOT NULL DEFAULT 0 CHECK (failure_told IN (0, 1));
