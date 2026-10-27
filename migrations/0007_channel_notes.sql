ALTER TABLE channels ADD COLUMN note TEXT NOT NULL DEFAULT '';
ALTER TABLE channels ADD COLUMN note_edited_by TEXT REFERENCES agents(name);
ALTER TABLE channels ADD COLUMN note_edited_at TEXT;
