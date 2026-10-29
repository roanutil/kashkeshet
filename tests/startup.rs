//! Starting the binary with settings it must refuse.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

use std::time::Duration;
use tokio::process::Command;

const KKT: &str = env!("CARGO_BIN_EXE_kkt");

/// Long enough for a refusal; a server that wrongly starts is killed then.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn a_bad_allowed_host_stops_the_server_naming_the_setting() {
    // Its own database and a free port, so a server that wrongly starts
    // touches nothing shared and is caught by the timeout.
    let dir = std::env::temp_dir().join(format!("kashkeshet-startup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("addr")
        .port();
    let run = Command::new(KKT)
        .arg("serve")
        .arg("--db")
        .arg(dir.join("startup.db"))
        .args(["--bind", &format!("127.0.0.1:{port}")])
        .args([
            "--allow-host",
            "http://host.docker.internal",
            "--no-access-key",
        ])
        .own_home(&dir)
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(REFUSAL_TIMEOUT, run)
        .await
        .expect("the server should refuse to start, not run")
        .expect("run kkt");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("invalid setting: allow_host"), "{text}");
}
