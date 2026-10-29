//! `kkt backup`, and the backup `kkt serve` makes before upgrading a
//! database: run with the real binary on databases in their own home
//! directories.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

mod common;

use rmcp::model::CallToolRequestParams;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);
const V1: &str = include_str!("../migrations/0001_init.sql");

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-backup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp home");
    dir
}

fn free_bind() -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("addr")
        .port();
    format!("127.0.0.1:{port}")
}

/// `kkt` with `args`, with `home` as its only home directory.
fn kkt(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(KKT);
    command.args(args).own_home(home).kill_on_drop(true);
    command
}

/// Starts `command` and returns it with every line it logged up to and
/// including the "listening" line.
async fn start(mut command: Command) -> (Child, String) {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start kkt");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let log = tokio::time::timeout(TIMEOUT, async {
        let mut log = String::new();
        while let Some(line) = lines.next_line().await.expect("read stdout") {
            log.push_str(&line);
            log.push('\n');
            if line.contains("listening") {
                return log;
            }
        }
        panic!("kkt exited before listening: {log}");
    })
    .await
    .expect("kkt did not start in time");
    (child, log)
}

/// Runs `command` to the end: whether it succeeded, and what it printed.
async fn finish(mut command: Command) -> (bool, String) {
    let out = tokio::time::timeout(TIMEOUT, command.output())
        .await
        .expect("kkt did not finish in time")
        .expect("run kkt");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

async fn ok(client: &common::Client, tool: &str, args: Value) -> String {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.call_tool(params).await.expect("call_tool");
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(result.is_error != Some(true), "{tool} failed: {text}");
    text
}

/// The value after `key: ` in a text response.
fn field(text: &str, key: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in {text}"))
        .to_string()
}

/// The backup folders in `folder`, oldest first.
fn backups_in(folder: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .expect("list backups")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found
}

fn messages_in(db: &Path) -> (i64, i64) {
    let conn = Connection::open(db).expect("open copy");
    conn.query_row(
        "SELECT count(*), coalesce(max(id), 0) FROM messages",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("count messages")
}

fn schema_version(db: &Path) -> i64 {
    Connection::open(db)
        .expect("open")
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("user_version")
}

#[tokio::test]
async fn backups_taken_while_agents_write_restore_to_a_working_server() {
    const MESSAGES: usize = 600;
    let home = temp_home("live");
    let bind = free_bind();
    let config_dir = home.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        format!("bind = \"{bind}\"\nno_access_key = true\nsends_per_minute = 100000\n"),
    )
    .expect("config file");
    let (mut server, _) = start(kkt(&home, &["serve"])).await;

    let client = common::connect_new_at(&format!("http://{bind}")).await;
    let token = field(&ok(&client, "register", json!({})).await, "token");
    let created = ok(
        &client,
        "create_channel",
        json!({"token": token, "name": "busy", "open": true, "format": "json"}),
    )
    .await;
    let channel: Value = serde_json::from_str(&created).expect("json");
    let channel_id = channel["id"].as_str().expect("channel id").to_string();
    let sender = tokio::spawn({
        let (token, channel_id) = (token.clone(), channel_id.clone());
        async move {
            for i in 0..MESSAGES {
                ok(
                    &client,
                    "send",
                    json!({"token": token, "channel_id": channel_id, "body": format!("m{i}")}),
                )
                .await;
            }
        }
    });
    let folder = home.join("backups");
    let folder_arg = folder.display().to_string();
    let mut taken = 0;
    while !sender.is_finished() || taken == 0 {
        let (success, text) = finish(kkt(&home, &["backup", &folder_arg])).await;
        assert!(success, "{text}");
        assert!(text.contains("config file"), "{text}");
        assert!(text.contains("no access key"), "{text}");
        taken += 1;
    }
    sender.await.expect("sender");
    server.kill().await.expect("stop kkt");

    let backups = backups_in(&folder);
    assert_eq!(backups.len(), taken);
    let counts: Vec<i64> = backups
        .iter()
        .map(|backup| {
            assert!(backup.join("config.toml").exists());
            assert!(!backup.join("access-key").exists());
            let (count, highest) = messages_in(&backup.join("kashkeshet.db"));
            // Each message is its own transaction: a consistent copy holds
            // the first n and nothing else.
            assert_eq!(count, highest);
            count
        })
        .collect();
    assert!(
        counts.iter().any(|&n| n > 0 && n < MESSAGES as i64),
        "no backup was taken while messages were being sent: {counts:?}"
    );

    // Restore the backup taken mid-stream: copy its database into place and
    // start a server on it.
    let partial = backups
        .iter()
        .find(|b| {
            let (n, _) = messages_in(&b.join("kashkeshet.db"));
            n > 0 && n < MESSAGES as i64
        })
        .expect("a mid-stream backup");
    let restored_home = temp_home("restored");
    let restored = restored_home.join("kashkeshet.db");
    std::fs::copy(partial.join("kashkeshet.db"), &restored).expect("restore");
    let restored_bind = free_bind();
    let restored_arg = restored.display().to_string();
    let (mut server, _) = start(kkt(
        &restored_home,
        &[
            "serve",
            "--db",
            &restored_arg,
            "--bind",
            &restored_bind,
            "--no-access-key",
        ],
    ))
    .await;
    let client = common::connect_new_at(&format!("http://{restored_bind}")).await;
    let token = field(&ok(&client, "register", json!({})).await, "token");
    ok(
        &client,
        "join_channel",
        json!({"token": token, "channel_id": channel_id}),
    )
    .await;
    ok(
        &client,
        "send",
        json!({"token": token, "channel_id": channel_id, "body": "after the restore"}),
    )
    .await;
    let read = ok(
        &client,
        "read",
        json!({"token": token, "channel_id": channel_id, "after_id": 0, "limit": 100}),
    )
    .await;
    assert!(read.contains("m0"), "{read}");
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&restored_home);
}

#[tokio::test]
async fn a_backup_keeps_both_keys_private_and_restores_them() {
    let home = temp_home("key");
    let (success, text) = finish(kkt(&home, &["key", "create"])).await;
    assert!(success, "{text}");
    let (success, text) = finish(kkt(&home, &["key", "create", "--web"])).await;
    assert!(success, "{text}");
    let db = home.join("kashkeshet.db");
    drop(kashkeshet::store::Db::open(&db).expect("make a database"));
    let folder = home.join("backups");
    let (success, text) = finish(kkt(
        &home,
        &[
            "backup",
            &folder.display().to_string(),
            "--db",
            &db.display().to_string(),
        ],
    ))
    .await;
    assert!(success, "{text}");
    assert!(text.contains("access key"), "{text}");
    let backup = backups_in(&folder).pop().expect("one backup");
    let name = backup
        .file_name()
        .expect("name")
        .to_string_lossy()
        .to_string();
    assert!(
        name.starts_with("kkt-") && name.ends_with(&format!("-v{}", env!("CARGO_PKG_VERSION"))),
        "{name}"
    );
    assert_eq!(
        std::fs::read(backup.join("access-key")).expect("key copy"),
        std::fs::read(home.join(".config/kkt/access-key")).expect("key")
    );
    assert_eq!(
        std::fs::read(backup.join("web-key")).unwrap(),
        std::fs::read(home.join(".config/kkt/web-key")).unwrap()
    );
    let restored = temp_home("keys-restored");
    let config_dir = restored.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).unwrap();
    for file in ["access-key", "web-key"] {
        std::fs::copy(backup.join(file), config_dir.join(file)).unwrap();
    }
    let bind = free_bind();
    let mut command = kkt(&restored, &["serve", "--bind", &bind, "--db"]);
    command.arg(backup.join("kashkeshet.db"));
    let (mut server, _) = start(command).await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let web = std::fs::read_to_string(config_dir.join("web-key")).unwrap();
    let response = http
        .post(format!("http://{bind}/login"))
        .header("origin", format!("http://{bind}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("key={}", web.trim()))
        .send()
        .await
        .unwrap();
    assert!(response.headers().contains_key("set-cookie"));
    let access = std::fs::read_to_string(config_dir.join("access-key")).unwrap();
    let response = http
        .post(format!("http://{bind}/mcp"))
        .bearer_auth(access.trim())
        .send()
        .await
        .unwrap();
    assert_ne!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    server.kill().await.unwrap();
    std::fs::remove_dir_all(restored).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&backup), 0o700);
        assert_eq!(mode(&backup.join("access-key")), 0o600);
        assert_eq!(mode(&backup.join("web-key")), 0o600);
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn backing_up_a_missing_database_is_refused() {
    let home = temp_home("missing");
    let folder = home.join("backups");
    let (success, text) = finish(kkt(
        &home,
        &[
            "backup",
            &folder.display().to_string(),
            "--db",
            "/nonexistent/kkt.db",
        ],
    ))
    .await;
    assert!(!success);
    assert!(
        text.contains("no database at /nonexistent/kkt.db"),
        "{text}"
    );
    assert!(!folder.exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// A database at schema version 1 with one message, at `path`.
fn make_v1(path: &Path) {
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

fn serve_args(db: &Path, bind: &str) -> Vec<String> {
    [
        "serve",
        "--db",
        &db.display().to_string(),
        "--bind",
        bind,
        "--no-access-key",
    ]
    .map(String::from)
    .to_vec()
}

#[tokio::test]
async fn an_upgrade_backs_up_the_database_first_and_keeps_the_newest_backups() {
    let home = temp_home("upgrade");
    let data = home.join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let db = data.join("kkt.db");
    make_v1(&db);
    let folder = data.join("upgrade-backups");
    std::fs::create_dir_all(&folder).expect("backups dir");
    for day in 1..=4 {
        std::fs::create_dir(folder.join(format!("kkt-202001{day:02}T000000.000Z-v0.1.0")))
            .expect("old backup");
    }
    std::fs::write(folder.join("notes.txt"), "mine").expect("other file");

    // With the default access key, which the upgrade backup must not copy.
    let mut args = serve_args(&db, &free_bind());
    args.retain(|a| a != "--no-access-key");
    args.extend(["--upgrade-backups-kept".into(), "2".into()]);
    let (mut server, log) = start(kkt(
        &home,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    ))
    .await;
    server.kill().await.expect("stop kkt");

    assert!(log.contains("backed up the database to"), "{log}");
    assert!(log.contains("from schema 1 to"), "{log}");
    let backups = backups_in(&folder);
    let names: Vec<String> = backups
        .iter()
        .map(|b| b.file_name().expect("name").to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(names[0], "kkt-20200104T000000.000Z-v0.1.0");
    assert!(
        names[1].ends_with(&format!("-v{}", env!("CARGO_PKG_VERSION"))),
        "{names:?}"
    );
    assert!(folder.join("notes.txt").exists());
    assert!(home.join(".config/kkt/access-key").exists());
    let mut files: Vec<String> = std::fs::read_dir(&backups[1])
        .expect("list backup")
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .collect();
    files.sort();
    assert_eq!(files, ["about.txt", "kashkeshet.db"]);
    // The backup is the database as it was before the upgrade.
    let copy = backups[1].join("kashkeshet.db");
    assert_eq!(schema_version(&copy), 1);
    assert_eq!(messages_in(&copy), (1, 1));
    assert!(schema_version(&db) > 1);

    // Up to date now, so another start makes no backup.
    let (mut server, log) = start(kkt(
        &home,
        &serve_args(&db, &free_bind())
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    ))
    .await;
    server.kill().await.expect("stop kkt");
    assert!(!log.contains("backed up"), "{log}");
    assert_eq!(backups_in(&folder).len(), 2);
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_failed_backup_before_an_upgrade_stops_the_server_and_changes_nothing() {
    let home = temp_home("upgrade-fails");
    let db = home.join("kkt.db");
    make_v1(&db);
    // A file where the backup folder should be, so the backup cannot be made.
    std::fs::write(home.join("upgrade-backups"), "in the way").expect("blocker");

    let (success, text) = finish(kkt(
        &home,
        &serve_args(&db, &free_bind())
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    ))
    .await;

    assert!(!success, "{text}");
    assert!(text.contains("the backup failed"), "{text}");
    assert!(text.contains("did not start"), "{text}");
    assert!(text.contains("upgrade-backups"), "{text}");
    assert_eq!(schema_version(&db), 1);
    assert_eq!(messages_in(&db), (1, 1));
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_new_database_needs_no_backup() {
    let home = temp_home("fresh");
    let db = home.join("kkt.db");
    let (mut server, log) = start(kkt(
        &home,
        &serve_args(&db, &free_bind())
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    ))
    .await;
    server.kill().await.expect("stop kkt");
    assert!(!log.contains("backed up"), "{log}");
    assert!(!home.join("upgrade-backups").exists());
    let _ = std::fs::remove_dir_all(&home);
}
