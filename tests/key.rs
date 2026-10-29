//! `kkt key create`: a new key in a file only its owner can read, never
//! printed unless asked, and never written over by accident.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-key-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp home");
    dir
}

async fn kkt(home: &Path, args: &[&str]) -> Output {
    let run = Command::new(KKT)
        .args(args)
        .own_home(home)
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("kkt key create should finish")
        .expect("run kkt")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn read_key(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("read key")
        .trim()
        .to_string()
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

#[tokio::test]
async fn creates_a_private_key_file_and_does_not_print_the_key() {
    let home = temp_home("default");
    let out = kkt(&home, &["key", "create"]).await;
    assert!(out.status.success(), "{}", text(&out));
    let path = home.join(".config/kkt/access-key");
    let key = read_key(&path);
    assert_eq!(key.len(), 50);
    let printed = text(&out);
    assert!(printed.contains(&path.display().to_string()), "{printed}");
    assert!(printed.contains("--access-key-file"), "{printed}");
    assert!(!printed.contains(&key), "the key was printed: {printed}");
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn refuses_to_overwrite_without_force() {
    let home = temp_home("overwrite");
    let path = home.join("my-key");
    let path_arg = path.display().to_string();
    let first = kkt(&home, &["key", "create", "--path", &path_arg]).await;
    assert!(first.status.success(), "{}", text(&first));
    let key = read_key(&path);

    let again = kkt(&home, &["key", "create", "--path", &path_arg]).await;
    assert!(!again.status.success());
    assert!(
        text(&again).contains("already exists; pass --force"),
        "{}",
        text(&again)
    );
    assert_eq!(read_key(&path), key, "the key was replaced without --force");

    // A replaced file is private again, even if it was not before.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    }
    let forced = kkt(&home, &["key", "create", "--path", &path_arg, "--force"]).await;
    assert!(forced.status.success(), "{}", text(&forced));
    assert_ne!(read_key(&path), key);
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn show_prints_the_key() {
    let home = temp_home("show");
    let path = home.join("shown-key");
    let path_arg = path.display().to_string();
    let out = kkt(&home, &["key", "create", "--path", &path_arg, "--show"]).await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(&read_key(&path)));
    let _ = std::fs::remove_dir_all(&home);
}

/// The key `kkt key create` writes is one the server accepts.
#[tokio::test]
async fn the_server_accepts_a_created_key() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let home = temp_home("accepted");
    let out = kkt(&home, &["key", "create"]).await;
    assert!(out.status.success(), "{}", text(&out));
    let key_file = home.join(".config/kkt/access-key");
    let key = read_key(&key_file);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("addr")
        .port();
    let bind = format!("127.0.0.1:{port}");
    let mut child = Command::new(KKT)
        .args(["serve", "--bind", &bind, "--access-key-file"])
        .arg(&key_file)
        .own_home(&home)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
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

    let http = reqwest::Client::new();
    let status = |auth: Option<String>| {
        let mut request = http.post(format!("http://{bind}/mcp"));
        if let Some(auth) = auth {
            request = request.header("Authorization", auth);
        }
        async move { request.send().await.expect("post /mcp").status().as_u16() }
    };
    assert_eq!(status(None).await, 401);
    assert_ne!(status(Some(format!("Bearer {key}"))).await, 401);

    child.kill().await.expect("stop kkt");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn show_prints_only_the_key() {
    let home = temp_home("show-only");
    let path = home.join("key");
    let key = "k".repeat(40);
    std::fs::write(&path, format!("{key}\n")).expect("write key");
    let out = kkt(
        &home,
        &["key", "show", "--key-file", path.to_str().expect("path")],
    )
    .await;
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{key}\n"));
    assert!(out.stderr.is_empty());
}

#[tokio::test]
async fn show_without_a_key_file_is_an_error() {
    let home = temp_home("show-missing");
    let out = kkt(&home, &["key", "show"]).await;
    assert!(!out.status.success());
    assert!(
        text(&out).contains("could not read key file"),
        "{}",
        text(&out)
    );
}

#[tokio::test]
async fn web_key_creation_show_and_rotation_are_separate_from_access_key() {
    let home = temp_home("web");
    for args in [vec!["key", "create"], vec!["key", "create", "--web"]] {
        let out = kkt(&home, &args).await;
        assert!(out.status.success(), "{}", text(&out));
    }
    let access_file = home.join(".config/kkt/access-key");
    let web_file = home.join(".config/kkt/web-key");
    let access = read_key(&access_file);
    let web = read_key(&web_file);
    assert_ne!(access, web);
    #[cfg(unix)]
    assert_eq!(mode(&web_file), 0o600);
    let out = kkt(&home, &["key", "show", "--web"]).await;
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), web);
    let out = kkt(&home, &["key", "header"]).await;
    assert!(out.status.success());
    assert!(!text(&out).contains(&web));
    let out = kkt(&home, &["key", "create", "--web", "--force"]).await;
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains(&read_key(&web_file)));
    assert_ne!(read_key(&web_file), web);
    assert_eq!(read_key(&access_file), access);
    std::fs::remove_dir_all(home).unwrap();
}

async fn start_server(home: &Path) -> (tokio::process::Child, String) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let bind = listener.local_addr().unwrap().to_string();
    drop(listener);
    let mut child = Command::new(KKT)
        .args(["serve", "--bind", &bind])
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(TIMEOUT, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.contains("listening") {
                return;
            }
        }
        panic!("server exited before listening");
    })
    .await
    .unwrap();
    (child, format!("http://{bind}"))
}

#[tokio::test]
async fn rotating_the_web_key_and_restarting_ends_old_sessions() {
    let home = temp_home("web-sessions");
    let (mut server, base) = start_server(&home).await;
    let access = read_key(&home.join(".config/kkt/access-key"));
    let old_web = read_key(&home.join(".config/kkt/web-key"));
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    async fn login(http: &reqwest::Client, base: &str, key: &str) -> reqwest::Response {
        http.post(format!("{base}/login"))
            .header("origin", base)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("key={key}"))
            .send()
            .await
            .unwrap()
    }
    let login_response = login(&http, &base, &old_web).await;
    let cookie = login_response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        http.get(format!("{base}/"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let out = kkt(&home, &["key", "create", "--web", "--force"]).await;
    assert!(out.status.success(), "{}", text(&out));
    let new_web = read_key(&home.join(".config/kkt/web-key"));
    assert_ne!(old_web, new_web);
    server.kill().await.unwrap();
    let (mut server, base) = start_server(&home).await;
    assert_eq!(
        http.get(format!("{base}/"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        303
    );
    assert_eq!(login(&http, &base, &old_web).await.status(), 401);
    assert_eq!(login(&http, &base, &access).await.status(), 401);
    assert!(
        login(&http, &base, &new_web)
            .await
            .headers()
            .contains_key("set-cookie")
    );
    assert_eq!(read_key(&home.join(".config/kkt/access-key")), access);
    server.kill().await.unwrap();
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn a_client_only_setup_cannot_show_or_create_a_web_key() {
    let home = temp_home("web-client");
    let config = home.join(".config/kkt");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("setup.toml"),
        "format = 1\nmode = \"client\"\nwritten_by = \"test\"\nurl = \"http://localhost:7420/mcp\"\n").unwrap();
    for action in ["show", "create"] {
        let out = kkt(&home, &["key", action, "--web"]).await;
        assert!(!out.status.success());
        assert!(
            text(&out).contains("web key is on the server's machine"),
            "{}",
            text(&out)
        );
    }
    assert!(!config.join("web-key").exists());
    std::fs::remove_dir_all(home).unwrap();
}
