//! `kkt serve`: its config file, its defaults, and the access key it makes
//! on first start. Each test runs the real binary with its own home
//! directory, so no test reads or writes the real one.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const KKT: &str = env!("CARGO_BIN_EXE_kkt");

/// Long enough to start or to refuse; a stuck server fails the test then.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A fresh directory to act as the home directory.
fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-serve-{name}-{}", std::process::id()));
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

/// Starts `command` and returns it with its "listening" log line.
async fn start(mut command: Command) -> (Child, String) {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start kkt");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let line = tokio::time::timeout(TIMEOUT, async {
        while let Some(line) = lines.next_line().await.expect("read stdout") {
            if line.contains("listening") {
                return line;
            }
        }
        panic!("kkt exited before listening");
    })
    .await
    .expect("kkt did not start in time");
    (child, line)
}

/// Runs `command` to the end and returns everything it printed.
async fn refused(mut command: Command) -> String {
    let out = tokio::time::timeout(TIMEOUT, command.output())
        .await
        .expect("kkt should refuse to start, not run")
        .expect("run kkt");
    assert!(!out.status.success(), "kkt should have failed");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

async fn status_for_host(bind: &str, host: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("http://{bind}/"))
        .header("Host", host)
        .send()
        .await
        .expect("get /")
        .status()
        .as_u16()
}

#[tokio::test]
async fn flags_win_over_the_config_file() {
    let home = temp_home("flags");
    let file_bind = free_bind();
    let flag_bind = free_bind();
    let config = home.join("kkt.toml");
    std::fs::write(
        &config,
        format!(
            "db = \"from-file.db\"\nbind = \"{file_bind}\"\nallow_hosts = [\"host.docker.internal\"]\n"
        ),
    )
    .expect("write config");
    let config_arg = config.display().to_string();
    let (mut child, line) = start(kkt(
        &home,
        &["serve", "--config", &config_arg, "--bind", &flag_bind],
    ))
    .await;

    // The flag's address, the file's database (relative to the file), and
    // the file named in the log.
    assert!(line.contains(&format!("http://{flag_bind}/")), "{line}");
    assert!(!line.contains(&file_bind), "{line}");
    assert!(
        line.contains(&home.join("from-file.db").display().to_string()),
        "{line}"
    );
    assert!(line.contains(&format!("config {config_arg}")), "{line}");
    assert!(home.join("from-file.db").exists());
    // The file's allowed host is in effect.
    assert_eq!(
        status_for_host(&flag_bind, "host.docker.internal").await,
        200
    );
    assert_eq!(status_for_host(&flag_bind, "evil.example").await, 403);

    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn serve_reads_the_default_config_file_and_keeps_data_per_user() {
    let home = temp_home("defaults");
    let config_dir = home.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    let config = config_dir.join("config.toml");
    std::fs::write(&config, "allow_hosts = [\"host.docker.internal\"]\n").expect("write config");
    let bind = free_bind();
    let (mut child, line) = start(kkt(&home, &["serve", "--bind", &bind])).await;

    let expected_db = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/kashkeshet/kashkeshet.db")
    } else {
        home.join(".local/share/kashkeshet/kashkeshet.db")
    };
    assert!(
        line.contains(&format!("database {}", expected_db.display())),
        "{line}"
    );
    assert!(
        line.contains(&format!("config {}", config.display())),
        "{line}"
    );
    assert!(expected_db.exists());
    assert_eq!(status_for_host(&bind, "host.docker.internal").await, 200);

    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn allow_host_takes_a_comma_separated_list_as_well_as_repeats() {
    // The Compose files pass KKT_ALLOW_HOSTS, a comma-separated list, as one
    // --allow-host value beside the Tailscale name's own --allow-host.
    let home = temp_home("allow-list");
    let bind = free_bind();
    let (mut child, _) = start(kkt(
        &home,
        &[
            "serve",
            "--bind",
            &bind,
            "--allow-host",
            "box.tail1234.ts.net",
            "--allow-host",
            "host.docker.internal,other.example",
        ],
    ))
    .await;

    for host in [
        "box.tail1234.ts.net",
        "host.docker.internal",
        "other.example",
    ] {
        assert_eq!(status_for_host(&bind, host).await, 200, "{host}");
    }
    assert_eq!(status_for_host(&bind, "evil.example").await, 403);

    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_bad_config_file_stops_serve_naming_the_setting() {
    let home = temp_home("bad");
    let cases = [
        (
            "bnid = \"127.0.0.1:1\"\n",
            "invalid setting: config has an unknown setting \"bnid\"",
        ),
        (
            "max_limit = \"lots\"\n",
            "invalid setting: max_limit in the config file must be",
        ),
        (
            "bind = \"0.0.0.0:7420\"\n",
            "invalid setting: bind must be a loopback address",
        ),
        (
            "default_limit = 0\n",
            "invalid setting: default_limit must be at least 1 (set in ",
        ),
    ];
    for (i, (text, expected)) in cases.iter().enumerate() {
        let config = home.join(format!("bad-{i}.toml"));
        std::fs::write(&config, text).expect("write config");
        let config_arg = config.display().to_string();
        let db = home.join("never.db").display().to_string();
        let out = refused(kkt(&home, &["serve", "--config", &config_arg, "--db", &db])).await;
        assert!(out.contains(expected), "{text}: {out}");
    }
    let missing = home.join("missing.toml").display().to_string();
    let out = refused(kkt(&home, &["serve", "--config", &missing])).await;
    assert!(
        out.contains("invalid setting: config could not be read"),
        "{out}"
    );
    assert!(!home.join("never.db").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn server_flags_belong_to_serve() {
    let home = temp_home("mix");
    let out = refused(kkt(&home, &["--bind", "127.0.0.1:1", "serve"])).await;
    assert!(out.contains("unexpected argument '--bind'"), "{out}");
    let _ = std::fs::remove_dir_all(&home);
}

/// A bad limit given as a flag is not blamed on the config file.
#[tokio::test]
async fn a_bad_flag_is_not_blamed_on_the_config_file() {
    let home = temp_home("blame");
    let config = home.join("ok.toml");
    std::fs::write(&config, "max_limit = 50\n").expect("write config");
    let config_arg = config.display().to_string();
    let db = home.join("never.db").display().to_string();
    let out = refused(kkt(
        &home,
        &[
            "serve",
            "--config",
            &config_arg,
            "--db",
            &db,
            "--default-limit",
            "0",
        ],
    ))
    .await;
    assert!(
        out.contains("invalid setting: default_limit must be at least 1"),
        "{out}"
    );
    assert!(!out.contains("(set in"), "{out}");
    let _ = std::fs::remove_dir_all(&home);
}

/// `kkt serve` stops cleanly on SIGTERM, as launchd and systemd send.
#[cfg(unix)]
#[tokio::test]
async fn serve_stops_cleanly_on_sigterm() {
    let home = temp_home("sigterm");
    let bind = free_bind();
    let (mut child, _) = start(kkt(&home, &["serve", "--bind", &bind])).await;
    let pid = child.id().expect("pid").to_string();
    let sent = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .await
        .expect("run kill");
    assert!(sent.success());
    let status = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .expect("kkt serve did not stop in time")
        .expect("wait");
    assert!(status.success(), "{status}");
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

/// With no key setting, the first start makes a key at the default path,
/// says where (never what), and requires it; later starts reuse it.
#[tokio::test]
async fn the_first_start_creates_the_default_key_and_requires_it() {
    let home = temp_home("default-key");
    let key_file = home.join(".config/kkt/access-key");
    let bind = free_bind();
    let mut command = kkt(&home, &["serve", "--bind", &bind]);
    command.stdout(Stdio::piped());
    let mut child = command.spawn().expect("start kkt");
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

    let key = std::fs::read_to_string(&key_file).expect("key created");
    let key = key.trim();
    assert!(
        log.contains(&format!(
            "created a new access key at {}",
            key_file.display()
        )),
        "{log}"
    );
    assert!(
        log.contains(&format!("access key from {}", key_file.display())),
        "{log}"
    );
    assert!(!log.contains(key), "the key must not be logged");
    #[cfg(unix)]
    assert_eq!(mode(&key_file), 0o600);

    let http = reqwest::Client::new();
    let mcp = |auth: Option<String>| {
        let mut request = http.post(format!("http://{bind}/mcp"));
        if let Some(auth) = auth {
            request = request.header("Authorization", auth);
        }
        async move { request.send().await.expect("post /mcp").status().as_u16() }
    };
    assert_eq!(mcp(None).await, 401);
    assert_ne!(mcp(Some(format!("Bearer {key}"))).await, 401);
    child.kill().await.expect("stop kkt");

    // A second start uses the same key and makes no new one.
    let (mut child, line) = start(kkt(&home, &["serve", "--bind", &free_bind()])).await;
    assert!(line.contains("access key from"), "{line}");
    assert_eq!(std::fs::read_to_string(&key_file).expect("key").trim(), key);
    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn no_access_key_runs_open_and_makes_no_key() {
    let home = temp_home("no-key");
    let bind = free_bind();
    let (mut child, line) = start(kkt(&home, &["serve", "--bind", &bind, "--no-access-key"])).await;
    assert!(line.contains("no access key"), "{line}");
    assert!(!home.join(".config/kkt/access-key").exists());
    assert!(!home.join(".config/kkt/web-key").exists());
    assert_eq!(
        reqwest::Client::new()
            .get(format!("http://{bind}/"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let status = reqwest::Client::new()
        .post(format!("http://{bind}/mcp"))
        .send()
        .await
        .expect("post /mcp")
        .status()
        .as_u16();
    assert_ne!(status, 401);
    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn no_access_key_and_a_key_file_together_are_refused() {
    let home = temp_home("both");
    let config = home.join("kkt.toml");
    std::fs::write(&config, "no_access_key = true\n").expect("write config");
    let config_arg = config.display().to_string();
    let db = home.join("never.db").display().to_string();
    let out = refused(kkt(
        &home,
        &[
            "serve",
            "--config",
            &config_arg,
            "--db",
            &db,
            "--access-key-file",
            "/keys/k",
        ],
    ))
    .await;
    assert!(
        out.contains("invalid setting: no_access_key cannot be set together with access_key_file"),
        "{out}"
    );
    assert!(!home.join("never.db").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn allow_any_bind_lets_serve_listen_beyond_loopback_only_with_a_key() {
    let home = temp_home("any-bind");
    let db = home.join("any.db").display().to_string();

    let out = refused(kkt(&home, &["serve", "--db", &db, "--bind", "0.0.0.0:0"])).await;
    assert!(
        out.contains("invalid setting: bind must be a loopback address"),
        "{out}"
    );

    let out = refused(kkt(
        &home,
        &[
            "serve",
            "--db",
            &db,
            "--bind",
            "0.0.0.0:0",
            "--allow-any-bind",
            "--no-access-key",
        ],
    ))
    .await;
    assert!(
        out.contains("invalid setting: allow_any_bind needs an access key"),
        "{out}"
    );

    let (mut child, line) = start(kkt(
        &home,
        &[
            "serve",
            "--db",
            &db,
            "--bind",
            "0.0.0.0:0",
            "--allow-any-bind",
        ],
    ))
    .await;
    assert!(line.contains("0.0.0.0"), "{line}");
    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}
