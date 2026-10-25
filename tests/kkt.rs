//! `kkt` is the short name for the kashkeshet command: same flags, same
//! defaults, same server.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const KASHKESHET: &str = env!("CARGO_BIN_EXE_kashkeshet");

async fn help(bin: &str) -> String {
    let out = Command::new(bin)
        .arg("--help")
        .output()
        .await
        .expect("run --help");
    assert!(out.status.success(), "{bin} --help failed");
    String::from_utf8(out.stdout).expect("utf-8 help")
}

#[tokio::test]
async fn kkt_has_the_same_flags_and_defaults() {
    let kkt = help(KKT).await;
    let long = help(KASHKESHET).await;
    assert!(kkt.contains("Usage: kkt "), "{kkt}");
    // Usage lines name the binary: one for flags, one for subcommands.
    let renamed = kkt
        .replace("Usage: kkt ", "Usage: kashkeshet ")
        .replace("\n       kkt ", "\n       kashkeshet ");
    assert_eq!(renamed, long);
}

/// With two binaries, `cargo run` (as the README says to start the server)
/// fails unless the package names a default. Running cargo from inside
/// `cargo test` would wait on the build lock, so this checks the manifest.
#[test]
fn cargo_run_starts_kashkeshet() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest
            .lines()
            .any(|l| l.trim() == r#"default-run = "kashkeshet""#),
        "Cargo.toml must set default-run = \"kashkeshet\""
    );
}

#[tokio::test]
async fn kkt_starts_and_serves() {
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
        .arg("--db")
        .arg(dir.join("kkt.db"))
        .arg("--bind")
        .arg(&bind)
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
