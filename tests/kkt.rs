//! The `kkt` command: on its own it explains itself, and `kkt serve` starts
//! and serves the web page.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const KKT: &str = env!("CARGO_BIN_EXE_kkt");

#[tokio::test]
async fn kkt_alone_prints_help() {
    let out = Command::new(KKT).output().await.expect("run kkt");
    assert!(!out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("Usage: kkt <COMMAND>"), "{text}");
    assert!(text.contains("kkt serve"), "{text}");
}

#[tokio::test]
async fn kkt_serve_starts_and_serves() {
    let dir = std::env::temp_dir().join(format!("kashkeshet-kkt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("addr")
        .port();
    let bind = format!("127.0.0.1:{port}");
    let mut child = Command::new(KKT)
        .arg("serve")
        .arg("--db")
        .arg(dir.join("kkt.db"))
        .args(["--bind", &bind, "--no-access-key"])
        .env("HOME", &dir)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("start kkt");

    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(line) = lines.next_line().await.expect("read stdout") {
            if line.contains("listening") {
                return;
            }
        }
        panic!("kkt exited before listening");
    })
    .await
    .expect("kkt did not start in time");

    let page = reqwest::get(format!("http://{bind}/"))
        .await
        .expect("get /");
    assert!(page.status().is_success());
    assert!(
        page.text()
            .await
            .expect("body")
            .contains("Kashkeshet (kkt)")
    );
    assert!(dir.join("kkt.db").exists());

    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&dir);
}
