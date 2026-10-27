-- A note each agent keeps for whoever next holds its identity, and when it
-- was last set (also when cleared). Empty until set.
ALTER TABLE agents ADD COLUMN handoff_note TEXT NOT NULL DEFAULT '';
ALTER TABLE agents ADD COLUMN handoff_note_set_at TEXT;
