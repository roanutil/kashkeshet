-- Messages from the human operator. A message's agent is an agent's name
-- or 'operator', which no agent can be named (agent names always hold a
-- hyphen). A plain foreign key can't allow that one value, so the table is
-- rebuilt without it and a trigger checks every other name instead. Rows,
-- IDs and the full-text index are kept.
CREATE TABLE messages_new (
    id          INTEGER PRIMARY KEY,
    channel_id  TEXT NOT NULL REFERENCES channels(id),
    agent       TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    run_id      INTEGER REFERENCES server_runs(id)
);

INSERT INTO messages_new (id, channel_id, agent, body, created_at, run_id)
    SELECT id, channel_id, agent, body, created_at, run_id FROM messages;

-- Dropping the table drops its indexes and triggers, and fires none of them.
DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;

CREATE INDEX messages_by_channel ON messages(channel_id, id);
CREATE INDEX messages_by_time ON messages(created_at);

CREATE TRIGGER messages_agent_exists BEFORE INSERT ON messages
WHEN NEW.agent <> 'operator' AND NOT EXISTS (SELECT 1 FROM agents WHERE name = NEW.agent)
BEGIN
    SELECT RAISE(ABORT, 'FOREIGN KEY constraint failed');
END;

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE OF body ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
