//! `kkt agent`: the operator lists agents and revokes a token in the
//! database file while a separate server process runs on it.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Starts a server on `db` at a free port; returns it and its base URL once
/// it is listening.
async fn start_server(db: &Path) -> (Child, String) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("addr")
        .port();
    let home = db.parent().expect("temp dir");
    let mut child = Command::new(KKT)
        .arg("serve")
        .arg("--db")
        .arg(db)
        .args(["--bind", &format!("127.0.0.1:{port}"), "--no-access-key"])
        .own_home(home)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("start kkt");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    tokio::time::timeout(TIMEOUT, async {
        while let Some(line) = lines.next_line().await.expect("read stdout") {
            if line.contains("listening") {
                return;
            }
        }
        panic!("kkt exited before listening");
    })
    .await
    .expect("kkt did not start in time");
    // Keep reading so the server never blocks on a full pipe.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    (child, format!("http://127.0.0.1:{port}"))
}

async fn kkt_agent(args: &[&str]) -> Output {
    // A home with no config file in it, so only the arguments decide.
    let run = Command::new(KKT)
        .arg("agent")
        .args(args)
        .own_home(temp_dir(&format!(
            "home-{}",
            NEXT_HOME.fetch_add(1, Ordering::Relaxed)
        )))
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("kkt agent should finish")
        .expect("run kkt agent")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Calls `whoami` with `token`; returns the text and whether it was an error.
async fn whoami(client: &common::Client, token: &str) -> (String, bool) {
    let params = CallToolRequestParams::new("whoami").with_arguments(
        json!({ "token": token })
            .as_object()
            .expect("object")
            .clone(),
    );
    let result = client.call_tool(params).await.expect("call whoami");
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    (text, result.is_error == Some(true))
}

#[tokio::test]
async fn revoking_while_the_server_runs_refuses_the_next_call() {
    let dir = temp_dir("revoke");
    let db = dir.join("kkt.db");
    let db_arg = db.to_str().expect("utf-8 path");
    let (mut server, base) = start_server(&db).await;
    let client = common::connect_new_at(&base).await;

    let registered = client
        .call_tool(CallToolRequestParams::new("register"))
        .await
        .expect("register");
    let reply: String = registered
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    let field = |label: &str| {
        reply
            .lines()
            .find_map(|l| l.strip_prefix(label))
            .expect("field")
            .trim()
            .to_string()
    };
    let name = field("name:");
    let token = field("token:");
    let (me, refused) = whoami(&client, &token).await;
    assert!(!refused, "{me}");

    let out = kkt_agent(&["revoke", &name, "--db", db_arg]).await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(&format!("revoked {name}")));

    let (answer, refused) = whoami(&client, &token).await;
    assert!(refused, "{answer}");
    assert!(answer.contains("unauthenticated"), "{answer}");

    let out = kkt_agent(&["list", "--db", db_arg]).await;
    assert!(out.status.success(), "{}", text(&out));
    let listed = text(&out);
    let line = listed
        .lines()
        .find(|l| l.starts_with(&name))
        .expect("agent listed");
    assert!(line.contains("revoked"), "{line}");

    let out = kkt_agent(&["revoke", &name, "--db", db_arg]).await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("already revoked"), "{}", text(&out));

    let out = kkt_agent(&["revoke", "no-such-agent", "--db", db_arg]).await;
    assert!(!out.status.success());
    assert!(text(&out).contains("not_found"), "{}", text(&out));

    client.cancel().await.expect("close");
    server.kill().await.expect("stop server");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_missing_database_is_not_created() {
    let dir = temp_dir("missing");
    let db = dir.join("nothing-here.db");
    let out = kkt_agent(&["list", "--db", db.to_str().expect("utf-8 path")]).await;
    assert!(!out.status.success());
    assert!(text(&out).contains("no database at"), "{}", text(&out));
    assert!(!db.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_explicit_database_does_not_read_the_config_file() {
    let dir = temp_dir("explicit");
    let db = dir.join("given.db");
    let (mut server, _base) = start_server(&db).await;
    server.kill().await.expect("stop server");
    let config = dir.join("config.toml");
    std::fs::write(&config, "not = [valid toml\n").expect("write config");
    let out = kkt_agent(&[
        "list",
        "--db",
        db.to_str().expect("utf-8 path"),
        "--config",
        config.to_str().expect("utf-8 path"),
    ])
    .await;
    assert!(out.status.success(), "{}", text(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_database_is_found_through_the_config_file() {
    let dir = temp_dir("config");
    let db = dir.join("from-config.db");
    let (mut server, _base) = start_server(&db).await;
    server.kill().await.expect("stop server");
    // `db` in the config file is taken from the config file's folder.
    let config = dir.join("config.toml");
    std::fs::write(&config, "db = \"from-config.db\"\n").expect("write config");
    let out = kkt_agent(&["list", "--config", config.to_str().expect("utf-8 path")]).await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains(&format!("no agents in {}", db.display())),
        "{}",
        text(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
