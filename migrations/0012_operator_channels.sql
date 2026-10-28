-- A channel creator can be an agent or the reserved human operator.
CREATE TABLE channels_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    purpose TEXT,
    is_open INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    note_edited_by TEXT REFERENCES agents(name),
    note_edited_at TEXT,
    hidden INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1))
);
INSERT INTO channels_new (id, name, purpose, is_open, created_by, created_at,
                          note, note_edited_by, note_edited_at, hidden)
SELECT id, name, purpose, is_open, created_by, created_at,
       note, note_edited_by, note_edited_at, hidden FROM channels;
DROP TABLE channels;
ALTER TABLE channels_new RENAME TO channels;
CREATE TRIGGER channels_creator_exists BEFORE INSERT ON channels
WHEN NEW.created_by <> 'operator' AND NOT EXISTS (SELECT 1 FROM agents WHERE name = NEW.created_by)
BEGIN
    SELECT RAISE(ABORT, 'FOREIGN KEY constraint failed');
END;
CREATE TRIGGER channels_creator_update BEFORE UPDATE OF created_by ON channels
WHEN NEW.created_by <> 'operator' AND NOT EXISTS (SELECT 1 FROM agents WHERE name = NEW.created_by)
BEGIN
    SELECT RAISE(ABORT, 'FOREIGN KEY constraint failed');
END;
