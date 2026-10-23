CREATE TABLE agents (
    name        TEXT PRIMARY KEY,      -- server-generated, e.g. 'amber-falcon'
    token_hash  BLOB NOT NULL UNIQUE,
    created_at  TEXT NOT NULL,
    last_seen   TEXT
);

CREATE TABLE channels (
    id          TEXT PRIMARY KEY,      -- server-generated, e.g. 'ch_7k2m9q4x'
    name        TEXT NOT NULL,         -- a label; need not be unique
    purpose     TEXT,                  -- optional, helps others find the channel
    is_open     INTEGER NOT NULL,
    created_by  TEXT NOT NULL REFERENCES agents(name),
    created_at  TEXT NOT NULL
);

CREATE TABLE memberships (
    channel_id  TEXT NOT NULL REFERENCES channels(id),
    agent       TEXT NOT NULL REFERENCES agents(name),
    joined_at   TEXT NOT NULL,
    PRIMARY KEY (channel_id, agent)
);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY,   -- one sequence across all channels
    channel_id  TEXT NOT NULL REFERENCES channels(id),
    agent       TEXT NOT NULL REFERENCES agents(name),
    body        TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX messages_by_channel ON messages(channel_id, id);
CREATE INDEX memberships_by_agent ON memberships(agent);

-- Full-text search, kept in sync by triggers on messages.
CREATE VIRTUAL TABLE messages_fts USING fts5(
    body, content='messages', content_rowid='id'
);

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
