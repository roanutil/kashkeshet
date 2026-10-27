-- Every save is a new immutable version, including saves of empty text.
CREATE TABLE operator_instructions (
    version INTEGER PRIMARY KEY AUTOINCREMENT CHECK (version > 0),
    text TEXT NOT NULL,
    saved_at TEXT NOT NULL,
    run_id INTEGER NOT NULL REFERENCES server_runs(id)
);
CREATE TRIGGER operator_instructions_no_update BEFORE UPDATE ON operator_instructions
BEGIN SELECT RAISE(ABORT, 'operator instruction versions are immutable'); END;
CREATE TRIGGER operator_instructions_no_delete BEFORE DELETE ON operator_instructions
BEGIN SELECT RAISE(ABORT, 'operator instruction versions are immutable'); END;
-- REPLACE performs an implicit delete, which need not fire delete triggers.
CREATE TRIGGER operator_instructions_no_replacement BEFORE INSERT ON operator_instructions
WHEN EXISTS (SELECT 1 FROM operator_instructions WHERE version = NEW.version)
BEGIN SELECT RAISE(ABORT, 'operator instruction versions are immutable'); END;
