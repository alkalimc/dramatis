-- World database, schema 7. Additive only.

-- When a process last showed it was working on a request. A request whose lease has run
-- out has no process behind it (a crash or a quit) and is the only kind a start sweeps.
ALTER TABLE task ADD COLUMN lease_at INTEGER;
