-- Every save is a new immutable version, including saves of empty text.
CREATE TABLE operator_instructions (
    version INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    saved_at TEXT NOT NULL,
    run_id INTEGER NOT NULL REFERENCES server_runs(id)
);
CREATE TRIGGER operator_instructions_no_update BEFORE UPDATE ON operator_instructions
BEGIN SELECT RAISE(ABORT, 'operator instruction versions are immutable'); END;
CREATE TRIGGER operator_instructions_no_delete BEFORE DELETE ON operator_instructions
BEGIN SELECT RAISE(ABORT, 'operator instruction versions are immutable'); END;
