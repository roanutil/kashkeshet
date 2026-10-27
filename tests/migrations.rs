//! Opening a database made by an older version brings it up to date and
//! keeps its rows.

use kashkeshet::store::Db;
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;

const V1: &str = include_str!("../migrations/0001_init.sql");

/// A fresh, empty directory for one test's database file.
fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kashkeshet-migrations-{test}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A database at version 1 with one agent, channel, and message.
fn make_v1(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open v1");
    conn.execute_batch(V1).expect("v1 schema");
    conn.pragma_update(None, "user_version", 1)
        .expect("version");
    conn.execute_batch(
        "INSERT INTO agents (name, token_hash, created_at) VALUES ('amber-falcon', x'00', 't');
         INSERT INTO channels (id, name, is_open, created_by, created_at)
             VALUES ('ch_1', 'general', 1, 'amber-falcon', 't');
         INSERT INTO messages (channel_id, agent, body, created_at)
             VALUES ('ch_1', 'amber-falcon', 'hello from v1', 't');",
    )
    .expect("v1 rows");
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        &format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?"),
        [column],
        |_| Ok(()),
    )
    .optional()
    .expect("table info")
    .is_some()
}

#[test]
fn a_v1_database_gains_server_runs_and_keeps_its_rows() {
    let dir = temp_dir("v1");
    let path = dir.join("old.db");
    make_v1(&path);

    drop(Db::open(&path).expect("open and migrate"));

    let conn = Connection::open(&path).expect("reopen");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("version");
    assert_eq!(version, 7);
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM server_runs", [], |r| r.get(0))
        .expect("server_runs exists");
    assert_eq!(runs, 0);
    assert!(column_exists(&conn, "agents", "run_id"));
    assert!(column_exists(&conn, "messages", "run_id"));
    assert!(column_exists(&conn, "agents", "revoked_at"));
    assert!(column_exists(&conn, "agents", "recovery_hash"));
    let recovery_hash: Option<Vec<u8>> = conn
        .query_row(
            "SELECT recovery_hash FROM agents WHERE name = 'amber-falcon'",
            [],
            |r| r.get(0),
        )
        .expect("old agent kept without recovery credentials");
    assert_eq!(recovery_hash, None);

    let agent_run: Option<i64> = conn
        .query_row(
            "SELECT run_id FROM agents WHERE name = 'amber-falcon'",
            [],
            |r| r.get(0),
        )
        .expect("old agent kept");
    assert_eq!(agent_run, None);
    let revoked: Option<String> = conn
        .query_row(
            "SELECT revoked_at FROM agents WHERE name = 'amber-falcon'",
            [],
            |r| r.get(0),
        )
        .expect("old agent kept");
    assert_eq!(revoked, None, "existing agents stay in use");
    let description: Option<String> = conn
        .query_row(
            "SELECT description FROM agents WHERE name = 'amber-falcon'",
            [],
            |r| r.get(0),
        )
        .expect("old agent kept with no description");
    assert_eq!(description, None);
    let (body, message_run): (String, Option<i64>) = conn
        .query_row("SELECT body, run_id FROM messages", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("old message kept");
    assert_eq!(body, "hello from v1");
    assert_eq!(message_run, None);

    // Search still finds the old message: the full-text index survived.
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'hello'",
            [],
            |r| r.get(0),
        )
        .expect("search");
    assert_eq!(hits, 1);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_new_row_can_name_its_run() {
    let dir = temp_dir("new-row");
    let path = dir.join("new.db");
    drop(Db::open(&path).expect("create"));

    let conn = Connection::open(&path).expect("reopen");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("foreign keys");
    conn.execute_batch(
        "INSERT INTO server_runs (started_at, version, git_commit, settings, instructions_sha256)
             VALUES ('t', '0.1.0', 'unknown', 'max_limit=100', 'ab');
         INSERT INTO agents (name, token_hash, created_at, run_id)
             VALUES ('amber-falcon', x'00', 't', 1);",
    )
    .expect("insert with run");
    let bad = conn.execute(
        "INSERT INTO agents (name, token_hash, created_at, run_id) VALUES ('b', x'01', 't', 99)",
        [],
    );
    assert!(bad.is_err(), "a run_id must name an existing run");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn an_older_database_gains_the_message_time_index() {
    let dir = temp_dir("time-index");
    let path = dir.join("old.db");
    make_v1(&path);

    drop(Db::open(&path).expect("open and migrate"));

    let conn = Connection::open(&path).expect("reopen");
    let index: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'messages_by_time'",
            [],
            |r| r.get(0),
        )
        .optional()
        .expect("schema");
    assert_eq!(index.as_deref(), Some("messages_by_time"));
    // The query the dashboard runs uses it.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT channel_id, created_at FROM messages WHERE created_at >= 'a'",
            [],
            |r| r.get(3),
        )
        .expect("plan");
    assert!(plan.contains("messages_by_time"), "{plan}");
    let kept: i64 = conn
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .expect("count");
    assert_eq!(kept, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn existing_channels_gain_an_empty_note_and_edits_survive_reopening() {
    let dir = temp_dir("channel-notes");
    let path = dir.join("old.db");
    make_v1(&path);
    let db = Db::open(&path).unwrap();
    let note = db
        .read(|s| s.channel_summary("ch_1", None, None))
        .await
        .unwrap()
        .unwrap()
        .note;
    assert_eq!(note.text, "");
    assert_eq!(note.edited_by, None);
    assert_eq!(note.edited_at, None);
    db.write(|s| s.set_note("ch_1", "a note\n😀", "amber-falcon", "2026-10-26T12:00:00Z"))
        .await
        .unwrap();
    drop(db);
    let db = Db::open(&path).unwrap();
    let note = db
        .read(|s| s.channel_summary("ch_1", None, None))
        .await
        .unwrap()
        .unwrap()
        .note;
    assert_eq!(note.text, "a note\n😀");
    assert_eq!(note.edited_by.as_deref(), Some("amber-falcon"));
    assert_eq!(note.edited_at.as_deref(), Some("2026-10-26T12:00:00Z"));
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}
