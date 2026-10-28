//! `kkt serve` on a machine `kkt setup` set up: run with the real binary in
//! its own home directory, with a setup record written by hand.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-setup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".config/kkt")).expect("temp home");
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

fn write_record(home: &Path, text: &str) {
    std::fs::write(home.join(".config/kkt/setup.toml"), text).expect("record");
}

/// `kkt serve` with its own database and port, plus `extra`.
fn serve(home: &Path, extra: &[&str]) -> Command {
    let mut command = Command::new(KKT);
    command
        .arg("serve")
        .args(extra)
        .arg("--db")
        .arg(home.join("kkt.db"))
        .args(["--bind", &free_bind(), "--no-access-key"])
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .kill_on_drop(true);
    command
}

/// Runs `command`, which should refuse to start; returns what it printed.
async fn refused(mut command: Command) -> String {
    let out = tokio::time::timeout(TIMEOUT, command.output())
        .await
        .expect("kkt should refuse, not run")
        .expect("run kkt");
    assert!(!out.status.success());
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Starts `command` and stops it once it is listening.
async fn starts(mut command: Command) {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
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
    child.kill().await.expect("stop kkt");
}

const CLIENT: &str = "format = 1\nmode = \"client\"\nwritten_by = \"0.4.0\"\n\
                      url = \"https://kkt.tail1234.ts.net/mcp\"\n";

#[tokio::test]
async fn a_client_machine_refuses_to_serve_unless_told_to_ignore_setup() {
    let home = temp_home("client");
    write_record(&home, CLIENT);
    let said = refused(serve(&home, &[])).await;
    assert!(
        said.contains("client of the kkt server at https://kkt.tail1234.ts.net/mcp"),
        "{said}"
    );
    assert!(
        !home.join("kkt.db").exists(),
        "refused before opening anything"
    );
    starts(serve(&home, &["--ignore-setup"])).await;
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_docker_machine_refuses_to_serve_naming_its_folder() {
    let home = temp_home("docker");
    write_record(
        &home,
        "format = 1\nmode = \"docker\"\nwritten_by = \"0.4.0\"\n\
         url = \"http://127.0.0.1:7420/mcp\"\n\n[docker]\n\
         folder = \"/srv/kkt-docker\"\ntailscale = false\n",
    );
    let said = refused(serve(&home, &[])).await;
    assert!(said.contains("Docker, from /srv/kkt-docker"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_broken_record_is_named_and_can_be_ignored() {
    let home = temp_home("broken");
    write_record(&home, "mode = ");
    let said = refused(serve(&home, &[])).await;
    assert!(said.contains("setup.toml"), "{said}");
    assert!(said.contains("is not valid"), "{said}");
    starts(serve(&home, &["--ignore-setup"])).await;
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn with_no_record_serve_runs_as_before() {
    let home = temp_home("none");
    starts(serve(&home, &[])).await;
    let _ = std::fs::remove_dir_all(&home);
}
