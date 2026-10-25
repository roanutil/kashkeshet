-- One row per server start: what produced the run. See src/provenance.
CREATE TABLE server_runs (
    id                   INTEGER PRIMARY KEY,
    started_at           TEXT NOT NULL,
    version              TEXT NOT NULL,
    git_commit           TEXT NOT NULL,  -- hash, hash-dirty, or 'unknown'
    settings             TEXT NOT NULL,  -- name=value lines
    instructions_sha256  TEXT NOT NULL   -- lowercase hex
);

-- The run that created each agent and message. Rows from before this
-- migration have none.
ALTER TABLE agents ADD COLUMN run_id INTEGER REFERENCES server_runs(id);
ALTER TABLE messages ADD COLUMN run_id INTEGER REFERENCES server_runs(id);
