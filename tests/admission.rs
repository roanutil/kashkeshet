//! The access key for `/mcp`: with `--access-key-file`, every MCP request
//! must send `Authorization: Bearer <key>`.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

mod common;

use kashkeshet::admission::{Admission, SharedKey};
use rmcp::model::CallToolRequestParams;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const KEY: &str = "0123456789abcdef0123456789abcdef";

async fn start_keyed() -> common::Server {
    let mut config = common::test_config();
    config.admission = Admission::SharedKey(SharedKey::new(KEY, "test").expect("key"));
    common::start_with(config).await
}

/// A `tools/list` request to `/mcp`, with `authorization` as the header if given.
async fn post_mcp(server: &common::Server, authorization: Option<&str>) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("{}/mcp", server.base))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#);
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    request.send().await.expect("post /mcp")
}

fn assert_refused(response: &reqwest::Response) {
    assert_eq!(response.status(), 401);
    assert_eq!(
        response
            .headers()
            .get("www-authenticate")
            .map(|v| v.as_bytes()),
        Some(&b"Bearer"[..])
    );
}

#[tokio::test]
async fn no_key_is_refused() {
    let server = start_keyed().await;
    assert_refused(&post_mcp(&server, None).await);
}

#[tokio::test]
async fn a_wrong_key_is_refused() {
    let server = start_keyed().await;
    assert_refused(&post_mcp(&server, Some("Bearer 0123456789abcdef0123456789abcdeX")).await);
    assert_refused(&post_mcp(&server, Some(&format!("Basic {KEY}"))).await);
}

#[tokio::test]
async fn the_right_key_lets_tools_work() {
    let server = start_keyed().await;
    let client = common::connect_new_with_key(&server, KEY).await;
    let registered = client
        .call_tool(CallToolRequestParams::new("register"))
        .await
        .expect("register");
    assert_ne!(registered.is_error, Some(true));
    client.cancel().await.expect("close");
}

#[tokio::test]
async fn without_a_key_file_nothing_changes() {
    let server = common::start().await;
    let response = post_mcp(&server, None).await;
    assert!(response.status().is_success(), "{}", response.status());
    let client = common::connect_new(&server).await;
    let registered = client
        .call_tool(CallToolRequestParams::new("register"))
        .await
        .expect("register");
    assert_ne!(registered.is_error, Some(true));
    client.cancel().await.expect("close");
}

/// Starts the server with `--access-key-file <file>` and returns what it
/// printed, expecting it to refuse to start.
async fn refused_start(file: &std::path::Path, db: &std::path::Path) -> String {
    let home = db.parent().expect("temp dir");
    let child = Command::new(env!("CARGO_BIN_EXE_kkt"))
        .arg("serve")
        .arg("--db")
        .arg(db)
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--access-key-file")
        .arg(file)
        .own_home(home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start kkt");
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("server should exit, not start")
        .expect("wait");
    assert!(!out.status.success());
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[tokio::test]
async fn a_bad_key_file_stops_the_server() {
    let dir = std::env::temp_dir().join(format!("kashkeshet-admission-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let db = dir.join("kkt.db");
    let empty = dir.join("empty");
    std::fs::write(&empty, "\n").expect("write");
    let short = dir.join("short");
    std::fs::write(&short, "too-short\n").expect("write");

    for file in [dir.join("missing"), empty, short] {
        let output = refused_start(&file, &db).await;
        assert!(
            output.contains("invalid setting: access_key_file"),
            "{}: {output}",
            file.display()
        );
        assert!(
            !db.exists(),
            "the database is opened only after the key is read"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
