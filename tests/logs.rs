//! `kkt serve --log-file`: the log goes to the file, holds the server's own
//! lines but not the MCP library's per-request ones, and never holds a key,
//! token or session ID.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-logs-{name}-{}", std::process::id()));
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

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Starts `kkt serve` logging to `log`, and waits for its "listening" line
/// to appear there.
async fn start(home: &Path, bind: &str, log: &Path) -> Child {
    let child = Command::new(KKT)
        .arg("serve")
        .arg("--db")
        .arg(home.join("kkt.db"))
        .args(["--bind", bind, "--log-file"])
        .arg(log)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start kkt");
    tokio::time::timeout(TIMEOUT, async {
        while !read(log).contains("listening") {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("kkt did not log that it was listening");
    child
}

/// Calls `register` over MCP with the access key and returns the reply.
async fn register(bind: &str, key: &str) -> String {
    reqwest::Client::new()
        .post(format!("http://{bind}/mcp"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {key}"))
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"register","arguments":{}}}"#)
        .send()
        .await
        .expect("post /mcp")
        .text()
        .await
        .expect("reply")
}

/// The first `act_` token in `text`.
fn token_in(text: &str) -> String {
    let start = text.find("act_").expect("a token in the reply");
    text[start..start + 30].to_string()
}

#[tokio::test]
async fn the_log_file_has_the_servers_lines_and_no_secrets() {
    let home = temp_home("secrets");
    let log = home.join("logs/kkt.log");
    let bind = free_bind();
    let mut child = start(&home, &bind, &log).await;
    let key = read(&home.join(".config/kkt/access-key"))
        .trim()
        .to_string();
    assert!(!key.is_empty(), "the default key was made");

    let mut tokens = Vec::new();
    for _ in 0..5 {
        tokens.push(token_in(&register(&bind, &key).await));
    }
    let login = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
        .post(format!("http://{bind}/login"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("key={key}"))
        .send()
        .await
        .expect("log in");
    let cookie = login
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("session cookie")
        .to_string();
    let session = cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, id)| id.to_string())
        .expect("session id");

    child.kill().await.expect("stop kkt");
    let out = child.wait_with_output().await.expect("output");
    let text = read(&log);

    // The server's own lines go to the file, not stdout.
    assert!(text.contains("created a new access key at"), "{text}");
    assert!(text.contains("listening"), "{text}");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("listening"),
        "nothing should go to stdout"
    );
    // The MCP library's per-request INFO lines are left out.
    assert!(!text.contains("serve_inner"), "{text}");
    assert!(!text.contains("Service initialized"), "{text}");
    // No line is colored.
    assert!(!text.contains('\u{1b}'), "{text}");
    // No secrets.
    assert!(!text.contains(&key), "the key is in the log");
    assert!(!text.contains(&session), "the session ID is in the log");
    for token in &tokens {
        assert!(!text.contains(token.as_str()), "a token is in the log");
    }
    assert!(!text.contains("act_"), "{text}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_log_file_that_cannot_be_opened_stops_the_server() {
    let home = temp_home("unopenable");
    // A folder where the file should be.
    let log = home.join("is-a-folder");
    std::fs::create_dir_all(&log).expect("folder");
    let out = tokio::time::timeout(
        TIMEOUT,
        Command::new(KKT)
            .arg("serve")
            .arg("--db")
            .arg(home.join("kkt.db"))
            .args(["--bind", &free_bind(), "--no-access-key", "--log-file"])
            .arg(&log)
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("kkt should refuse to start")
    .expect("run kkt");
    assert!(!out.status.success());
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        said.contains("invalid setting: log_file could not be opened"),
        "{said}"
    );
    assert!(!home.join("kkt.db").exists());
    let _ = std::fs::remove_dir_all(&home);
}
