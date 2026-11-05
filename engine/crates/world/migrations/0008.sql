-- World database, schema 8. Additive only.

-- A daily event the user deleted from the world drawer. It stays in the table: it still
-- counts toward the day's quota and its material still keeps the next event from repeating
-- it. Only the drawer and the terminal's list leave it out.
ALTER TABLE daily_event ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
