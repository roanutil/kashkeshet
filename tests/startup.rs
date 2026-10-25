//! Starting the binary with settings it must refuse.

use tokio::process::Command;

const KASHKESHET: &str = env!("CARGO_BIN_EXE_kashkeshet");

#[tokio::test]
async fn a_bad_allowed_host_stops_the_server_naming_the_setting() {
    // The settings are checked before the database is opened, so this
    // creates no file.
    let out = Command::new(KASHKESHET)
        .args(["--allow-host", "http://host.docker.internal"])
        .output()
        .await
        .expect("run kashkeshet");
    assert!(!out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("invalid setting: allow_host"), "{text}");
}
