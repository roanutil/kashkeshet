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

/// `kkt` with `args`, `home` as its only home directory, no terminal, and
/// an empty PATH so no real client is found.
async fn kkt(home: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(KKT)
        .args(args)
        .env("HOME", home)
        .env("PATH", "")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(TIMEOUT, out)
        .await
        .expect("kkt should finish")
        .expect("run kkt");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// A server with an access key, in its own home; returns it, its MCP URL
/// and its key file.
async fn keyed_server(name: &str) -> (tokio::process::Child, String, PathBuf) {
    let (home, key) = server_key(name).await;
    let bind = free_bind();
    let child = serve_with_key(&home, &key, &bind).await;
    (child, format!("http://{bind}/mcp"), key)
}

/// A home for a server, and a new access key in it.
async fn server_key(name: &str) -> (PathBuf, PathBuf) {
    let home = temp_home(name);
    let key = home.join("server-key");
    let (made, said) = kkt(
        &home,
        &["key", "create", "--path", key.to_str().expect("utf-8")],
    )
    .await;
    assert!(made, "{said}");
    (home, key)
}

/// Starts `kkt serve` with `key` on `bind`, returning once it listens.
async fn serve_with_key(home: &Path, key: &Path, bind: &str) -> tokio::process::Child {
    let mut command = Command::new(KKT);
    command
        .args(["serve", "--bind", bind, "--access-key-file"])
        .arg(key)
        .arg("--db")
        .arg(home.join("kkt.db"))
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().expect("start kkt");
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
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    child
}

#[tokio::test]
async fn a_client_machine_is_set_up_from_flags_alone() {
    let (mut server, url, key) = keyed_server("client-server").await;
    let home = temp_home("client-setup");
    let key_arg = key.to_str().expect("utf-8");
    let (ok, said) = kkt(
        &home,
        &[
            "setup",
            "--mode",
            "client",
            "--url",
            &url,
            "--key-file",
            key_arg,
            "--clients",
            "codex",
        ],
    )
    .await;
    assert!(ok, "{said}");
    assert!(said.contains("set up as client"), "{said}");
    assert!(said.contains("ok: key accepted"), "doctor ran: {said}");

    let record = std::fs::read_to_string(home.join(".config/kkt/setup.toml")).expect("record");
    assert!(record.contains("mode = \"client\""), "{record}");
    assert!(record.contains(&format!("url = \"{url}\"")), "{record}");
    let codex = std::fs::read_to_string(home.join(".codex/config.toml")).expect("codex config");
    assert!(codex.contains(&url), "{codex}");

    // Doctor, with nothing given, checks the recorded server with the
    // recorded key. (Its last line, about this machine's service, depends on
    // the machine, so only the connection checks are asserted.)
    let (_, said) = kkt(&home, &["doctor"]).await;
    assert!(
        said.contains(&format!("ok: server: reachable at {url}")),
        "{said}"
    );
    assert!(said.contains("ok: key accepted"), "{said}");
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn with_no_terminal_a_missing_answer_names_its_flag() {
    let home = temp_home("no-terminal");
    let (ok, said) = kkt(&home, &["setup"]).await;
    assert!(!ok);
    assert!(said.contains("pass --mode"), "{said}");
    let (ok, said) = kkt(&home, &["setup", "--mode", "client"]).await;
    assert!(!ok);
    assert!(said.contains("pass --url"), "{said}");
    for mode in ["docker", "service"] {
        let (ok, said) = kkt(&home, &["setup", "--mode", mode, "--yes"]).await;
        assert!(!ok);
        assert!(
            said.contains("pass --tailscale or --no-tailscale"),
            "{said}"
        );
        let (ok, said) = kkt(&home, &["setup", "--mode", mode, "--no-tailscale", "--yes"]).await;
        assert!(!ok);
        assert!(
            said.contains("pass --allow-host host.docker.internal, or --allow-host none"),
            "{said}"
        );
    }
    assert!(!home.join(".config/kkt/setup.toml").exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// The record of an earlier setup, which a failed setup must leave alone.
const EARLIER: &str = "format = 1\nmode = \"client\"\nwritten_by = \"0.4.0\"\n\
                       url = \"https://old.example/mcp\"\n";

fn record_text(home: &Path) -> String {
    std::fs::read_to_string(home.join(".config/kkt/setup.toml")).expect("record")
}

/// `kkt setup` for a client of `url` with the key in `key`, plus `extra`.
async fn setup_client(home: &Path, url: &str, key: &Path, extra: &[&str]) -> (bool, String) {
    let key = key.to_str().expect("utf-8");
    let mut args = vec!["setup", "--mode", "client", "--url", url, "--key-file", key];
    args.extend(extra);
    kkt(home, &args).await
}

/// The key is checked as soon as it is given, so setup stops before any
/// other step, naming the flag.
#[tokio::test]
async fn a_rejected_key_leaves_the_earlier_setup() {
    let (mut server, url, _) = keyed_server("wrong-key-server").await;
    let home = temp_home("wrong-key");
    write_record(&home, EARLIER);
    let wrong = home.join("wrong-key");
    let (made, said) = kkt(
        &home,
        &["key", "create", "--path", wrong.to_str().expect("utf-8")],
    )
    .await;
    assert!(made, "{said}");
    let (ok, said) = setup_client(&home, &url, &wrong, &["--clients", "none"]).await;
    assert!(!ok, "{said}");
    assert!(said.contains("--key-file"), "{said}");
    assert!(
        said.contains("problem: key accepted: server returned 401"),
        "{said}"
    );
    assert!(
        !said.contains("ok: "),
        "only the problems are shown: {said}"
    );
    assert_eq!(record_text(&home), EARLIER);
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn an_unreachable_server_leaves_the_earlier_setup() {
    let home = temp_home("unreachable");
    write_record(&home, EARLIER);
    let key = home.join("key");
    let (made, said) = kkt(
        &home,
        &["key", "create", "--path", key.to_str().expect("utf-8")],
    )
    .await;
    assert!(made, "{said}");
    // Nothing listens on a port just freed.
    let url = format!("http://{}/mcp", free_bind());
    let (ok, said) = setup_client(&home, &url, &key, &["--clients", "codex"]).await;
    assert!(!ok, "{said}");
    assert!(said.contains("--url"), "{said}");
    assert!(said.contains("problem: server: "), "{said}");
    assert!(
        said.contains("check the address, and that the server is running on its machine"),
        "{said}"
    );
    assert_eq!(record_text(&home), EARLIER);
    assert!(
        !home.join(".codex/config.toml").exists(),
        "no client was touched"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// A server that answers, but at another path, fails the address check,
/// so the problem is put on `--url`, not on the key.
#[tokio::test]
async fn a_wrong_path_on_a_running_server_is_put_on_the_url() {
    let (mut server, url, key) = keyed_server("wrong-path-server").await;
    let home = temp_home("wrong-path");
    write_record(&home, EARLIER);
    let wrong = format!("{url}x");
    let (ok, said) = setup_client(&home, &wrong, &key, &["--clients", "none"]).await;
    assert!(!ok, "{said}");
    assert!(said.contains("given with --url"), "{said}");
    assert!(!said.contains("given with --key-file"), "{said}");
    assert!(said.contains("problem: MCP: "), "{said}");
    assert_eq!(record_text(&home), EARLIER);
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_missing_answer_leaves_the_earlier_setup() {
    let (mut server, url, key) = keyed_server("missing-answer-server").await;
    let home = temp_home("missing-answer");
    write_record(&home, EARLIER);
    // A Codex config, so Codex is found and setup would ask about it.
    std::fs::create_dir_all(home.join(".codex")).expect("codex dir");
    std::fs::write(home.join(".codex/config.toml"), "# mine\n").expect("codex config");
    let (ok, said) = setup_client(&home, &url, &key, &[]).await;
    assert!(!ok, "{said}");
    assert!(said.contains("pass --clients"), "{said}");
    assert_eq!(record_text(&home), EARLIER);
    assert_eq!(
        std::fs::read_to_string(home.join(".codex/config.toml")).expect("codex"),
        "# mine\n"
    );
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_client_that_cannot_be_configured_leaves_the_earlier_setup() {
    let (mut server, url, key) = keyed_server("bad-client-server").await;
    let home = temp_home("bad-client");
    write_record(&home, EARLIER);
    // A folder where Codex's config file should be, so it can't be written.
    std::fs::create_dir_all(home.join(".codex/config.toml")).expect("in the way");
    let (ok, said) = setup_client(&home, &url, &key, &["--clients", "codex"]).await;
    assert!(!ok, "{said}");
    assert!(said.contains("could not connect Codex"), "{said}");
    assert_eq!(record_text(&home), EARLIER);
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn url_and_key_file_each_replace_only_their_part_of_the_record() {
    let (mut server, url, key) = keyed_server("override-server").await;
    let home = temp_home("override");
    let unreachable = format!("http://{}/mcp", free_bind());
    write_record(
        &home,
        &format!(
            "format = 1\nmode = \"client\"\nwritten_by = \"0.4.0\"\nurl = \"{unreachable}\"\n\
             key_file = \"{}\"\n",
            key.display()
        ),
    );
    // --url alone: the given server, with the recorded key.
    let (_, said) = kkt(&home, &["doctor", "--url", &url]).await;
    assert!(
        said.contains(&format!("ok: server: reachable at {url}")),
        "{said}"
    );
    assert!(said.contains("ok: key accepted"), "{said}");
    // --key-file alone: the recorded server, with the given key.
    let other = home.join("other-key");
    let (made, made_said) = kkt(
        &home,
        &["key", "create", "--path", other.to_str().expect("utf-8")],
    )
    .await;
    assert!(made, "{made_said}");
    let (_, said) = kkt(
        &home,
        &["doctor", "--key-file", other.to_str().expect("utf-8")],
    )
    .await;
    assert!(
        said.contains(&format!("key file: {}", other.display())),
        "{said}"
    );
    assert!(said.contains(&unreachable), "{said}");
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

/// A home folder with nothing in it at all, as on a new machine.
fn empty_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-setup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("empty home");
    dir
}

/// A `docker` that does nothing and succeeds, except that asking the kkt
/// container for its key prints `key`, and listing running services lists
/// kkt when `FAKE_KKT_RUNNING` is set. Put in its own folder for PATH.
fn fake_docker(dir: &Path, key: &str) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let docker = bin.join("docker");
    std::fs::write(
        &docker,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n\
             *\"kkt key show\"*) echo '{key}' ;;\n\
             *\"ps --status running --services\"*) [ -n \"$FAKE_KKT_RUNNING\" ] && echo kkt ;;\n\
             esac\nexit 0\n"
        ),
    )
    .expect("fake docker");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755))
            .expect("make it runnable");
    }
    bin
}

#[tokio::test]
async fn docker_setup_works_from_a_home_with_nothing_in_it() {
    // A real server stands in for the container, on the port setup is told
    // Docker publishes, so the connection check is real.
    let (mut server, url, key) = keyed_server("docker-server").await;
    let port = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches("/mcp")
        .to_string();
    let key_text = std::fs::read_to_string(&key)
        .expect("key")
        .trim()
        .to_string();
    let scratch = empty_home("docker-scratch");
    let bin = fake_docker(&scratch, &key_text);
    let home = empty_home("docker-home");

    let out = Command::new(KKT)
        .args([
            "setup",
            "--mode",
            "docker",
            "--no-tailscale",
            "--clients",
            "none",
            "--allow-host",
            "host.docker.internal",
        ])
        .args(["--docker-project", "kkt-test", "--docker-port", &port])
        .env("HOME", &home)
        .env("PATH", &bin)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(TIMEOUT, out)
        .await
        .expect("setup should finish")
        .expect("run kkt");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{said}");
    let saved = home.join(".config/kkt/docker-access-key");
    assert_eq!(
        std::fs::read_to_string(&saved).expect("saved key").trim(),
        key_text
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&saved)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    let record = record_text(&home);
    assert!(record.contains("mode = \"docker\""), "{record}");
    assert!(record.contains(&format!("url = \"{url}\"")), "{record}");
    // The extra host name reaches the managed settings file.
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/kashkeshet")
    } else {
        home.join(".local/share/kashkeshet")
    };
    let env = std::fs::read_to_string(data.join("docker/.env")).expect("managed .env");
    assert!(
        env.contains("KKT_ALLOW_HOSTS=host.docker.internal\n"),
        "{env}"
    );
    assert!(
        said.contains(&format!("the web page is at http://127.0.0.1:{port}/")),
        "{said}"
    );
    server.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// `kkt doctor` with a Docker record for `folder`, `bin` as PATH, and the
/// fake kkt container running or not. Returns what it printed.
async fn doctor_for_docker(home: &Path, bin: &Path, folder: &Path, running: bool) -> String {
    write_record(
        home,
        &format!(
            "format = 1\nmode = \"docker\"\nwritten_by = \"0.4.0\"\n\
             url = \"http://{}/mcp\"\n\n[docker]\nfolder = \"{}\"\ntailscale = false\n",
            free_bind(),
            folder.display()
        ),
    );
    let mut command = Command::new(KKT);
    command
        .arg("doctor")
        .env("HOME", home)
        .env("PATH", bin)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("FAKE_KKT_RUNNING")
        .kill_on_drop(true);
    if running {
        command.env("FAKE_KKT_RUNNING", "1");
    }
    let out = tokio::time::timeout(TIMEOUT, command.output())
        .await
        .expect("doctor should finish")
        .expect("run kkt");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[tokio::test]
async fn on_a_docker_machine_doctor_reports_the_container_not_the_service() {
    let home = temp_home("doctor-docker");
    let bin = fake_docker(&home, "unused");
    let folder = home.join("docker files");
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join(".env"), "KKT_VERSION=0.4.0\n").expect(".env");

    let said = doctor_for_docker(&home, &bin, &folder, true).await;
    assert!(
        said.contains("ok: this machine's kkt in Docker: running kkt 0.4.0"),
        "{said}"
    );
    assert!(!said.contains("this machine's kkt service"), "{said}");

    let said = doctor_for_docker(&home, &bin, &folder, false).await;
    assert!(
        said.contains("problem: this machine's kkt in Docker: not running"),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "cd '{}' && docker compose up -d",
            folder.display()
        )),
        "{said}"
    );

    std::fs::remove_file(folder.join(".env")).expect("remove .env");
    let said = doctor_for_docker(&home, &bin, &folder, true).await;
    assert!(said.contains("its files are missing from"), "{said}");
    assert!(said.contains("run `kkt setup` again"), "{said}");

    // No docker at all.
    std::fs::write(folder.join(".env"), "KKT_VERSION=0.4.0\n").expect(".env");
    let said = doctor_for_docker(&home, &home.join("no-bin"), &folder, true).await;
    assert!(
        said.contains("problem: this machine's kkt in Docker: "),
        "{said}"
    );
    assert!(said.contains("check that Docker is running"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn on_a_client_machine_doctor_has_no_line_about_a_server_here() {
    let home = temp_home("doctor-client");
    write_record(&home, CLIENT);
    let (_, said) = kkt(&home, &["doctor"]).await;
    assert!(said.contains("https://kkt.tail1234.ts.net/mcp"), "{said}");
    assert!(!said.contains("this machine's kkt"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn setup_waits_for_a_server_it_set_up_that_answers_late() {
    // As with a new Tailscale node, whose HTTPS certificate comes on first
    // use: the server only answers a few seconds after setup's check starts.
    let (server_home, key) = server_key("late-server").await;
    let bind = free_bind();
    let port = bind.trim_start_matches("127.0.0.1:").to_string();
    let key_text = std::fs::read_to_string(&key)
        .expect("key")
        .trim()
        .to_string();
    let scratch = empty_home("late-scratch");
    let bin = fake_docker(&scratch, &key_text);
    let home = empty_home("late-home");

    let setup = Command::new(KKT)
        .args([
            "setup",
            "--mode",
            "docker",
            "--no-tailscale",
            "--clients",
            "none",
            "--allow-host",
            "none",
        ])
        .args(["--docker-project", "kkt-test", "--docker-port", &port])
        .env("HOME", &home)
        .env("PATH", &bin)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start setup");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut server = serve_with_key(&server_home, &key, &bind).await;
    let out = tokio::time::timeout(TIMEOUT, setup.wait_with_output())
        .await
        .expect("setup should finish once the server answers")
        .expect("run setup");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Waiting for the server to answer"), "{said}");
    assert!(said.contains("ok: key accepted"), "{said}");
    assert!(record_text(&home).contains("mode = \"docker\""));
    server.kill().await.expect("stop kkt");
    for dir in [&home, &scratch, &server_home] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

const LOGGED_IN: &str = r#"{"BackendState":"Running","Self":{"DNSName":"kkt.tail1.ts.net."},"CertDomains":["kkt.tail1.ts.net"]}"#;
const LOGIN_NEEDED: &str = r#"{"BackendState":"NeedsLogin","AuthURL":"","Self":{"DNSName":""}}"#;

/// Runs `kkt <command>` on a machine set up as Docker with Tailscale, where
/// the Tailscale container says `tailscale_status`, or does not answer if
/// that is `None`. Returns what it printed.
async fn tailscale_docker_output(
    name: &str,
    command: &str,
    tailscale_status: Option<&str>,
) -> String {
    let home = temp_home(name);
    let folder = home.join("docker");
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(
        folder.join(".env"),
        "KKT_VERSION=0.4.0\nCOMPOSE_FILE=compose.tailscale.yaml\n",
    )
    .expect("env");
    write_record(
        &home,
        &format!(
            "format = 1\nmode = \"docker\"\nwritten_by = \"0.4.0\"\n\
             url = \"https://kkt.tail1.ts.net/mcp\"\n\n[docker]\nfolder = \"{}\"\n\
             tailscale = true\nhostname = \"kkt\"\ntailnet = \"tail1.ts.net\"\n",
            folder.display()
        ),
    );
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let docker = bin.join("docker");
    std::fs::write(
        &docker,
        "#!/bin/sh\ncase \"$*\" in\n\
         *\"tailscale status --json\"*) [ -n \"$FAKE_TS_STATUS\" ] && { echo \"$FAKE_TS_STATUS\"; exit 0; } || exit 1 ;;\n\
         *\"ps --status running --services\"*) echo kkt ;;\n\
         esac\nexit 0\n",
    )
    .expect("fake docker");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755))
            .expect("make it runnable");
    }
    let output = || async {
        let mut run = Command::new(KKT);
        run.arg(command)
            .env("HOME", &home)
            .env("PATH", &bin)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("FAKE_TS_STATUS")
            .kill_on_drop(true);
        if let Some(status) = tailscale_status {
            run.env("FAKE_TS_STATUS", status);
        }
        let out = tokio::time::timeout(TIMEOUT, run.output())
            .await
            .expect("kkt should finish")
            .expect("run kkt");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    // Another test's process starting can briefly hold the script open for
    // writing, so that running it fails with "Text file busy": try again.
    let mut printed = output().await;
    for _ in 0..20 {
        if !printed.contains("Text file busy") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        printed = output().await;
    }
    let _ = std::fs::remove_dir_all(&home);
    printed
}

#[tokio::test]
async fn status_and_doctor_say_when_the_tailscale_machine_needs_a_login() {
    let said = tailscale_docker_output("ts-status-login", "status", Some(LOGIN_NEEDED)).await;
    assert!(said.contains("Tailscale login: needed"), "{said}");
    assert!(said.contains("kkt setup"), "{said}");
    let said = tailscale_docker_output("ts-doctor-login", "doctor", Some(LOGIN_NEEDED)).await;
    assert!(
        said.contains("problem: this machine's Tailscale login: not logged in"),
        "{said}"
    );
    assert!(said.contains("fix: run `kkt setup` again"), "{said}");
}

#[tokio::test]
async fn status_and_doctor_are_quiet_about_tailscale_when_it_is_logged_in() {
    let said = tailscale_docker_output("ts-status-ok", "status", Some(LOGGED_IN)).await;
    assert!(said.contains("Tailscale login: logged in"), "{said}");
    let said = tailscale_docker_output("ts-doctor-ok", "doctor", Some(LOGGED_IN)).await;
    assert!(
        said.contains("ok: this machine's Tailscale login: logged in"),
        "{said}"
    );
}

#[tokio::test]
async fn a_silent_tailscale_container_is_not_called_logged_out() {
    let said = tailscale_docker_output("ts-status-down", "status", None).await;
    assert!(said.contains("Tailscale login: unknown"), "{said}");
    assert!(!said.contains("needed"), "{said}");
    let said = tailscale_docker_output("ts-doctor-down", "doctor", None).await;
    assert!(
        said.contains(
            "problem: this machine's Tailscale login: the Tailscale container does not answer"
        ),
        "{said}"
    );
}

#[tokio::test]
async fn doctor_does_not_tell_a_docker_or_client_machine_to_start_the_service() {
    // The server is not answering, as when the Tailscale machine is off the
    // tailnet. The old fix, `kkt service start`, is wrong on these machines.
    let said = tailscale_docker_output("ts-doctor-hint", "doctor", Some(LOGIN_NEEDED)).await;
    assert!(said.contains("problem: server"), "{said}");
    assert!(!said.contains("kkt service start"), "{said}");
    assert!(
        said.contains("see the lines below, or run `kkt status`"),
        "{said}"
    );
    // A client machine's server is on another machine.
    let home = temp_home("doctor-hint-client");
    write_record(
        &home,
        &format!(
            "format = 1\nmode = \"client\"\nwritten_by = \"0.4.0\"\nurl = \"http://{}/mcp\"\n",
            free_bind()
        ),
    );
    let out = tokio::time::timeout(
        TIMEOUT,
        Command::new(KKT)
            .arg("doctor")
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("doctor should finish")
    .expect("run kkt");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!said.contains("kkt service start"), "{said}");
    assert!(
        said.contains("the server runs on another machine"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&home);
}
