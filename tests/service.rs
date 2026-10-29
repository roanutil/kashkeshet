//! `kkt service install --dry-run` with the real binary: what the service
//! would run, worked out from a temporary home, with nothing changed. The
//! other service commands are tested against a fake system in
//! `src/service/tests.rs`, so no test here starts a real service.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

const KKT: &str = env!("CARGO_BIN_EXE_kkt");
const TIMEOUT: Duration = Duration::from_secs(20);

fn temp_home(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("kashkeshet-service-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp home");
    // Absolute and free of symlinks, as the paths in the service will be.
    dir.canonicalize().expect("canonical temp home")
}

async fn kkt(home: &Path, args: &[&str]) -> Output {
    let run = Command::new(KKT)
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(TIMEOUT, run)
        .await
        .expect("kkt should finish")
        .expect("run kkt")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn service_file(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents/com.sam-mfb.kashkeshet.plist")
    } else {
        home.join(".config/systemd/user/kashkeshet.service")
    }
}

fn default_db(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/kashkeshet/kashkeshet.db")
    } else {
        home.join(".local/share/kashkeshet/kashkeshet.db")
    }
}

#[tokio::test]
async fn a_dry_run_shows_absolute_paths_and_changes_nothing() {
    let home = temp_home("dry");
    let config_dir = home.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        "allow_hosts = [\"host.docker.internal\"]\n",
    )
    .expect("config");

    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.starts_with("Dry run: nothing was changed."), "{said}");
    assert!(
        said.contains(&format!("Would write {}:", service_file(&home).display())),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "Would create a new web key at {}",
            config_dir.join("web-key").display()
        )),
        "{said}"
    );
    assert!(!config_dir.join("web-key").exists());
    let exe = Path::new(KKT).canonicalize().expect("kkt path");
    for path in [exe, config_dir.join("config.toml"), default_db(&home)] {
        assert!(
            said.contains(&path.display().to_string()),
            "{} missing from: {said}",
            path.display()
        );
    }
    // The service is the server setup meant, so `kkt serve` lets it start.
    assert!(said.contains("--ignore-setup"), "{said}");
    // With no key set, the default one is made at install and passed.
    let key = config_dir.join("access-key");
    assert!(
        said.contains(&format!(
            "Would create a new access key at {}",
            key.display()
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!("\"--access-key-file\" \"{}\"", key.display()))
            || said.contains(&format!("<string>{}</string>", key.display())),
        "{said}"
    );
    // Nothing was written: not the service file, not the database's folder,
    // not the key.
    assert!(!key.exists());
    assert!(!service_file(&home).exists());
    assert!(!default_db(&home).parent().expect("db dir").exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_database_named_in_the_config_file_is_left_to_it() {
    let home = temp_home("configdb");
    let config_dir = home.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(config_dir.join("config.toml"), "db = \"nested/chat.db\"\n").expect("config");

    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    // The service reads the database from the config file each time it
    // starts, so it isn't passed and the default isn't used.
    assert!(!said.contains("--db"), "{said}");
    assert!(
        !said.contains(&default_db(&home).display().to_string()),
        "{said}"
    );
    // serve won't make a configured database's folder, so install does.
    assert!(
        said.contains(&format!(
            "Would create {}",
            config_dir.join("nested").display()
        )),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn relative_flags_become_absolute() {
    let home = temp_home("relative");
    let out = Command::new(KKT)
        .args(["service", "install", "--dry-run", "--db", "data/chat.db"])
        .current_dir(&home)
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .await
        .expect("run kkt");
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains(&home.join("data/chat.db").display().to_string()),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn bad_settings_fail_at_install_not_in_a_restart_loop() {
    let home = temp_home("bad");
    let config_dir = home.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(config_dir.join("config.toml"), "default_limit = 0\n").expect("config");
    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("invalid setting: default_limit"), "{said}");

    std::fs::write(config_dir.join("config.toml"), "").expect("config");
    let missing = home.join("no-such-key");
    let out = kkt(
        &home,
        &[
            "service",
            "install",
            "--dry-run",
            "--access-key-file",
            missing.to_str().expect("utf-8"),
        ],
    )
    .await;
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("access_key_file"), "{said}");
    assert!(!service_file(&home).exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn log_output_has_no_color_codes_when_not_a_terminal() {
    // A service's log is a file or the journal, not a terminal; color codes
    // would show up there as raw escape characters.
    let home = temp_home("ansi");
    let out = kkt(&home, &["serve", "--default-limit", "0"]).await;
    let said = text(&out);
    assert!(said.contains("invalid setting"), "{said}");
    assert!(!said.contains('\u{1b}'), "{said:?}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn an_existing_key_is_used_and_a_configured_one_left_to_the_config() {
    let home = temp_home("keys");
    let out = kkt(&home, &["key", "create"]).await;
    assert!(out.status.success(), "{}", text(&out));
    let key = home.join(".config/kkt/access-key");

    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(!said.contains("Would create a new access key"), "{said}");
    assert!(said.contains(&key.display().to_string()), "{said}");

    // A key file named in the config is read from it at each start.
    std::fs::write(
        home.join(".config/kkt/config.toml"),
        "access_key_file = \"access-key\"\n",
    )
    .expect("config");
    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(!said.contains("--access-key-file"), "{said}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn no_access_key_installs_without_a_key() {
    let home = temp_home("no-key");
    let key = home.join(".config/kkt/access-key");

    // As a flag: passed on to serve, and no key made.
    let out = kkt(
        &home,
        &["service", "install", "--dry-run", "--no-access-key"],
    )
    .await;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(!said.contains("Would create a new access key"), "{said}");
    assert!(!said.contains("--access-key-file"), "{said}");
    assert!(said.contains("--no-access-key"), "{said}");
    assert!(!key.exists());

    // From the config file: left to the config, which serve reads at start.
    let config_dir = home.join(".config/kkt");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(config_dir.join("config.toml"), "no_access_key = true\n").expect("config");
    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(!said.contains("Would create a new access key"), "{said}");
    assert!(!said.contains("--access-key-file"), "{said}");
    assert!(!said.contains("--no-access-key"), "{said}");

    // With a key file as well, install is refused, as serve would be.
    let out = kkt(
        &home,
        &[
            "service",
            "install",
            "--dry-run",
            "--access-key-file",
            "/keys/k",
        ],
    )
    .await;
    assert!(!out.status.success());
    assert!(
        text(&out).contains("invalid setting: no_access_key cannot be set together"),
        "{}",
        text(&out)
    );
    assert!(!key.exists());
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn a_bad_existing_web_key_is_checked_even_when_the_access_key_is_new() {
    let home = temp_home("bad-web");
    let keys = home.join(".config/kkt");
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(keys.join("web-key"), "too-short").unwrap();
    let out = kkt(&home, &["service", "install", "--dry-run"]).await;
    assert!(!out.status.success());
    assert!(text(&out).contains("web_key_file"));
    assert!(!keys.join("access-key").exists());
    std::fs::remove_dir_all(home).unwrap();
}
