//! `kkt serve --log-file`: the log goes to the file, holds the server's own
//! lines but not the MCP library's per-request ones, and never holds a key,
//! token or session ID.
//!
//! Each test stops the server with SIGTERM and waits for it to exit before
//! reading the log, so every line it logged is in the file: some lines, such
//! as the MCP library's, are written after the reply has been sent.
#![cfg(unix)]

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::process::{Child, Command};

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);

fn temp_home(name: &str) -> PathBuf {
    // Tests run in parallel, and each starts by emptying its folder: two
    // tests sharing a name would delete each other's files mid-test.
    static USED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    {
        let mut used = USED.lock().expect("used names");
        assert!(
            !used.iter().any(|n| n == name),
            "two tests use the temp folder {name:?}"
        );
        used.push(name.to_string());
    }
    let dir =
        std::env::temp_dir().join(format!("kashkeshet-logfile-{name}-{}", std::process::id()));
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
        .own_home(home)
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

/// Stops the server with SIGTERM, as a service manager would, waits for it
/// to exit cleanly, and returns what it printed.
async fn stop(child: Child) -> std::process::Output {
    let pid = child.id().expect("pid").to_string();
    let sent = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .await
        .expect("run kill");
    assert!(sent.success());
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .expect("kkt did not stop in time")
        .expect("wait for kkt");
    assert!(out.status.success(), "kkt exited with {}", out.status);
    out
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
    let child = start(&home, &bind, &log).await;
    let key = read(&home.join(".config/kkt/access-key"))
        .trim()
        .to_string();
    assert!(!key.is_empty(), "the default key was made");

    let mut tokens = Vec::new();
    for _ in 0..5 {
        tokens.push(token_in(&register(&bind, &key).await));
    }
    let web_key = read(&home.join(".config/kkt/web-key")).trim().to_string();
    let login = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
        .post(format!("http://{bind}/login"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("key={web_key}"))
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

    let out = stop(child).await;
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
    assert!(!text.contains(&web_key), "the web key is in the log");
    assert!(text.contains("created a new web key at"));
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
            .own_home(&home)
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

/// Starts `kkt serve` with `args` and no key, logging to `log`, and waits
/// until it answers (not for a log line, which a level may leave out).
async fn start_with(home: &Path, bind: &str, log: &Path, args: &[&str]) -> Child {
    let child = Command::new(KKT)
        .arg("serve")
        .arg("--db")
        .arg(home.join("kkt.db"))
        .args(["--bind", bind, "--no-access-key", "--log-file"])
        .arg(log)
        .args(args)
        .own_home(home)
        .kill_on_drop(true)
        .spawn()
        .expect("start kkt");
    tokio::time::timeout(TIMEOUT, async {
        while reqwest::get(format!("http://{bind}/")).await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("kkt did not start answering");
    child
}

/// One MCP request and one web request naming a host that isn't allowed.
async fn make_some_noise(bind: &str) {
    let client = reqwest::Client::new();
    client
        .post(format!("http://{bind}/mcp"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"register","arguments":{}}}"#)
        .send()
        .await
        .expect("post /mcp");
    client
        .get(format!("http://{bind}/"))
        .header("host", "evil.example")
        .send()
        .await
        .expect("get /");
}

#[tokio::test]
async fn dependency_log_level_info_brings_back_the_mcp_librarys_lines() {
    let home = temp_home("deps-info");
    let log = home.join("kkt.log");
    let bind = free_bind();
    let child = start_with(&home, &bind, &log, &["--dependency-log-level", "info"]).await;
    make_some_noise(&bind).await;
    stop(child).await;
    let text = read(&log);
    assert!(text.contains("Service initialized"), "{text}");
    assert!(text.contains("listening"), "{text}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn log_level_warn_in_the_config_file_leaves_out_info_lines() {
    let home = temp_home("own-warn");
    let log = home.join("kkt.log");
    let config = home.join("kkt.toml");
    std::fs::write(&config, "log_level = \"warn\"\n").expect("config");
    let bind = free_bind();
    let config_arg = config.display().to_string();
    let child = start_with(&home, &bind, &log, &["--config", &config_arg]).await;
    make_some_noise(&bind).await;
    stop(child).await;
    let text = read(&log);
    assert!(!text.contains("listening"), "{text}");
    assert!(text.contains("refused a web request"), "{text}");
    assert!(!text.contains("Service initialized"), "{text}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn an_unknown_level_is_refused_naming_the_setting() {
    let home = temp_home("bad-level");
    let out = Command::new(KKT)
        .args(["serve", "--log-level", "loud"])
        .own_home(&home)
        .kill_on_drop(true)
        .output()
        .await
        .expect("run kkt");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("log_level must be one of"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

/// Registers over MCP (no key) and calls a tool with the new token; returns
/// the secrets the reply held: the token and the recovery code.
async fn register_and_use(bind: &str) -> Vec<String> {
    let client = reqwest::Client::new();
    let post = |body: String| {
        client
            .post(format!("http://{bind}/mcp"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body)
            .send()
    };
    let reply = post(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"register","arguments":{}}}"#
            .to_string(),
    )
    .await
    .expect("register")
    .text()
    .await
    .expect("reply");
    let secrets: Vec<String> = ["act_", "arc_"]
        .iter()
        .filter_map(|prefix| reply.find(prefix).map(|at| reply[at..at + 30].to_string()))
        .collect();
    assert!(!secrets.is_empty(), "{reply}");
    post(format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"whoami","arguments":{{"token":"{}"}}}}}}"#,
        secrets[0]
    ))
    .await
    .expect("whoami");
    secrets
}

#[tokio::test]
async fn no_allowed_dependency_level_logs_secrets() {
    for level in ["error", "warn", "info"] {
        let home = temp_home(&format!("no-secrets-{level}"));
        let log = home.join("kkt.log");
        let bind = free_bind();
        let child = start_with(
            &home,
            &bind,
            &log,
            &["--dependency-log-level", level, "--log-level", "trace"],
        )
        .await;
        let secrets = register_and_use(&bind).await;
        stop(child).await;
        let text = read(&log);
        for secret in &secrets {
            assert!(
                !text.contains(secret.as_str()),
                "{level}: a secret is in the log"
            );
        }
        assert!(
            !text.contains("act_") && !text.contains("arc_"),
            "{level}: {text}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[tokio::test]
async fn dependency_levels_that_would_log_secrets_are_refused() {
    for level in ["debug", "trace"] {
        let home = temp_home(&format!("refuse-{level}"));
        let out = Command::new(KKT)
            .args(["serve", "--no-access-key", "--dependency-log-level", level])
            .arg("--db")
            .arg(home.join("kkt.db"))
            .own_home(&home)
            .kill_on_drop(true)
            .output()
            .await
            .expect("run kkt");
        assert!(!out.status.success(), "{level}");
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            said.contains("invalid setting: dependency_log_level must be error, warn or info"),
            "{said}"
        );
        assert!(!home.join("kkt.db").exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
