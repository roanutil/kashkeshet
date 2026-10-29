//! Mode dispatch with an isolated home and fake external commands.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;
use kashkeshet::setup::{DockerSetup, Mode, Record};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn home(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("kkt-lifecycle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join(".config/kkt")).unwrap();
    path
}

fn record(home: &Path, mode: Mode) {
    let record = Record {
        mode,
        written_by: "0.0.0".into(),
        url: "https://node.tail.ts.net/mcp".into(),
        key_file: Some(home.join("key")),
    };
    std::fs::write(
        home.join(".config/kkt/setup.toml"),
        record.render().unwrap(),
    )
    .unwrap();
}

fn run(home: &Path, args: &[&str]) -> Output {
    run_in(home, home, args)
}

/// Runs `kkt` with `dir` as the folder it was started in.
fn run_in(home: &Path, dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kkt"));
    command.current_dir(dir);
    command.args(args).own_home(home);
    if home.join("bin").exists() {
        command.env(
            "PATH",
            std::env::join_paths(
                std::iter::once(home.join("bin")).chain(std::env::split_paths(
                    &std::env::var_os("PATH").unwrap_or_default(),
                )),
            )
            .unwrap(),
        );
        command.env("KKT_TEST_COMMAND_LOG", home.join("commands"));
    }
    command.output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn client_commands_explain_remote_ownership_and_uninstall_preserves_files() {
    let home = home("client");
    record(&home, Mode::Client);
    std::fs::write(home.join("key"), "private").unwrap();
    std::fs::write(home.join("client-config"), "preserved").unwrap();
    let out = run(&home, &["status"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("https://node.tail.ts.net/mcp"));
    for args in [vec!["backup", "unused"], vec!["upgrade"]] {
        let out = run(&home, &args);
        assert!(!out.status.success());
        assert!(text(&out).contains("machine where it runs"));
    }
    let out = run(&home, &["uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!home.join(".config/kkt/setup.toml").exists());
    assert_eq!(
        std::fs::read_to_string(home.join("key")).unwrap(),
        "private"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("client-config")).unwrap(),
        "preserved"
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn upgrade_and_uninstall_without_a_record_request_setup() {
    let home = home("unrecorded");
    for command in ["upgrade", "uninstall"] {
        let out = run(&home, &[command]);
        assert!(!out.status.success());
        assert!(text(&out).contains("run `kkt setup`"));
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
fn fake(home: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(home.join("bin")).unwrap();
    let path = home.join("bin").join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$KKT_TEST_COMMAND_LOG\"\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
fn docker_record(home: &Path) -> PathBuf {
    let folder = home.join("managed-docker");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(".env"), "KKT_VERSION=0.0.0\nCOMPOSE_FILE=compose.yaml\nCOMPOSE_PROJECT_NAME=isolated\nKKT_PORT=7421\n").unwrap();
    record(
        home,
        Mode::Docker(DockerSetup {
            folder: folder.clone(),
            tailscale: false,
            hostname: None,
            tailnet: None,
        }),
    );
    folder
}

#[cfg(unix)]
#[test]
fn docker_backup_upgrade_and_uninstall_dispatch_to_managed_folder() {
    let home = home("docker");
    let folder = docker_record(&home);
    fake(
        &home,
        "docker",
        "case \" $* \" in\n*' exec '*) echo 'Backed up to /backups/kkt-fixture' ;;\n*' ps '*) echo kkt ;;\nesac\nexit 0",
    );
    let out = run(&home, &["status"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Running: true"));
    let dest = home.join("export");
    let out = run(&home, &["backup", dest.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out));
    let out = run(&home, &["upgrade"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        std::fs::read_to_string(folder.join(".env"))
            .unwrap()
            .contains(&format!("KKT_VERSION={}", env!("CARGO_PKG_VERSION")))
    );
    let out = run(&home, &["uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    let log = std::fs::read_to_string(home.join("commands")).unwrap();
    assert!(log.contains(folder.to_str().unwrap()));
    assert!(log.contains("exec -T kkt kkt backup /backups"));
    assert!(log.contains("cp kkt:/backups/kkt-fixture"));
    assert!(log.contains("pull"));
    assert!(log.lines().last().unwrap().ends_with("down"));
    assert!(!log.contains("--volumes"));
    assert!(folder.join(".env").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn docker_backup_failure_prevents_upgrade_and_failed_uninstall_keeps_record() {
    let home = home("docker-failed");
    let folder = docker_record(&home);
    let before = std::fs::read(folder.join(".env")).unwrap();
    fake(&home, "docker", "echo 'intentional failure' >&2\nexit 1");
    let out = run(&home, &["upgrade"]);
    assert!(!out.status.success());
    assert_eq!(std::fs::read(folder.join(".env")).unwrap(), before);
    let log = std::fs::read_to_string(home.join("commands")).unwrap();
    assert!(!log.contains("pull"));
    assert!(!log.contains("up -d"));
    let out = run(&home, &["uninstall"]);
    assert!(!out.status.success());
    assert!(home.join(".config/kkt/setup.toml").exists());
    let out = run(&home, &["backup", "unused", "--db", "somewhere"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("--db and --config apply only"));
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn service_backup_failure_prevents_reinstall() {
    let home = home("service-failed");
    record(&home, Mode::Service { tailscale: false });
    std::fs::write(
        home.join(".config/kkt/config.toml"),
        "db = 'missing.db'\nno_access_key = true\n",
    )
    .unwrap();
    fake(&home, "systemctl", "exit 0");
    let out = run(&home, &["upgrade"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no database"), "{}", text(&out));
    assert!(
        !home.join("commands").exists(),
        "service untouched until backup succeeds"
    );
    assert!(home.join(".config/kkt/setup.toml").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn service_upgrade_backs_up_configured_database_then_uninstall_keeps_it() {
    let home = home("service-success");
    record(&home, Mode::Service { tailscale: false });
    let database = home.join(".config/kkt/chosen.db");
    drop(kashkeshet::store::Db::open(&database).unwrap());
    std::fs::write(
        home.join(".config/kkt/config.toml"),
        "db = 'chosen.db'\nno_access_key = true\n",
    )
    .unwrap();
    fake(&home, "systemctl", "exit 0");
    let out = run(&home, &["upgrade"]);
    assert!(out.status.success(), "{}", text(&out));
    let backups: Vec<_> = std::fs::read_dir(home.join(".config/kkt/backups"))
        .unwrap()
        .collect();
    assert_eq!(backups.len(), 1);
    let saved = backups[0].as_ref().unwrap().path();
    assert!(saved.join("kashkeshet.db").exists());
    assert_eq!(
        std::fs::read_to_string(saved.join("config.toml")).unwrap(),
        "db = 'chosen.db'\nno_access_key = true\n"
    );
    let log = std::fs::read_to_string(home.join("commands")).unwrap();
    assert!(log.contains("restart kashkeshet.service"));
    let unit = home.join(".config/systemd/user/kashkeshet.service");
    let text_unit = std::fs::read_to_string(&unit).unwrap();
    assert!(text_unit.contains(env!("CARGO_BIN_EXE_kkt")));
    assert!(text_unit.contains("--ignore-setup"));
    let log_before = std::fs::read(home.join("commands")).unwrap();
    let out = run(&home, &["upgrade"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("already at"));
    assert_eq!(std::fs::read(home.join("commands")).unwrap(), log_before);
    assert_eq!(
        std::fs::read_dir(home.join(".config/kkt/backups"))
            .unwrap()
            .count(),
        1
    );
    let out = run(&home, &["uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!unit.exists());
    assert!(!home.join(".config/kkt/setup.toml").exists());
    assert!(database.exists());
    assert!(saved.exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn service_backup_and_upgrade_preserve_existing_keys_and_create_a_missing_web_key() {
    for has_web_key in [false, true] {
        let home = home(if has_web_key {
            "existing-web-key"
        } else {
            "missing-web-key"
        });
        record(&home, Mode::Service { tailscale: false });
        let config = home.join(".config/kkt");
        let database = config.join("chosen.db");
        drop(kashkeshet::store::Db::open(&database).unwrap());
        std::fs::write(config.join("config.toml"), "db = 'chosen.db'\n").unwrap();
        let access_key = "0123456789abcdef0123456789abcdef";
        let web_key = "abcdef0123456789abcdef0123456789";
        std::fs::write(config.join("access-key"), access_key).unwrap();
        if has_web_key {
            std::fs::write(config.join("web-key"), web_key).unwrap();
        }
        // No server has started with the separate web key feature yet.
        fake(&home, "systemctl", "exit 0");
        let manual = home.join("manual-backups");
        let out = run(&home, &["backup", manual.to_str().unwrap()]);
        assert!(out.status.success(), "{}", text(&out));
        assert_eq!(config.join("web-key").exists(), has_web_key);
        let out = run(&home, &["upgrade"]);
        assert!(out.status.success(), "{}", text(&out));
        for folder in [manual, config.join("backups")] {
            let backups: Vec<_> = std::fs::read_dir(folder).unwrap().collect();
            assert_eq!(backups.len(), 1);
            let saved = backups[0].as_ref().unwrap().path();
            assert!(saved.join("kashkeshet.db").exists());
            assert!(saved.join("config.toml").exists());
            assert_eq!(
                std::fs::read_to_string(saved.join("access-key")).unwrap(),
                access_key
            );
            assert_eq!(saved.join("web-key").exists(), has_web_key);
            let about = std::fs::read_to_string(saved.join("about.txt")).unwrap();
            if has_web_key {
                assert_eq!(
                    std::fs::read_to_string(saved.join("web-key")).unwrap(),
                    web_key
                );
                assert!(about.contains(&format!(
                    "web-key: copy of {}",
                    config.join("web-key").display()
                )));
            } else {
                assert!(about.contains("web-key: none"));
            }
        }
        let created_web_key = std::fs::read_to_string(config.join("web-key")).unwrap();
        if has_web_key {
            assert_eq!(created_web_key, web_key);
        } else {
            assert!(!created_web_key.trim().is_empty());
            assert_ne!(created_web_key.trim(), access_key);
        }
        assert_eq!(
            std::fs::read_to_string(config.join("access-key")).unwrap(),
            access_key
        );
        let log = std::fs::read_to_string(home.join("commands")).unwrap();
        assert!(log.contains("restart kashkeshet.service"));
        let updated = std::fs::read_to_string(config.join("setup.toml")).unwrap();
        assert!(updated.contains(env!("CARGO_PKG_VERSION")));
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn docker_backup_without_a_folder_does_not_copy_out() {
    let home = home("docker-no-dest");
    let folder = docker_record(&home);
    fake(
        &home,
        "docker",
        "echo 'Backed up to /backups/kkt-fixture'\nexit 0",
    );
    let out = run(&home, &["backup"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains(folder.join("backups/kkt-fixture").to_str().unwrap()));
    let log = std::fs::read_to_string(home.join("commands")).unwrap();
    assert_eq!(log.lines().count(), 1);
    assert!(log.contains("exec -T kkt kkt backup /backups"));
    assert!(!log.contains(" cp "));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_local_backup_still_requires_a_folder() {
    let home = home("local-dest");
    for mode in [None, Some(Mode::Service { tailscale: false })] {
        if let Some(mode) = mode {
            record(&home, mode);
        }
        let out = run(&home, &["backup"]);
        assert!(!out.status.success());
        assert!(text(&out).contains("a local backup needs FOLDER"));
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn same_version_is_a_noop_and_downgrade_requires_an_override() {
    let home = home("versions");
    let folder = docker_record(&home);
    fake(
        &home,
        "docker",
        "echo 'Backed up to /backups/kkt-fixture'\nexit 0",
    );
    for (version, success, expected) in [
        (env!("CARGO_PKG_VERSION"), true, "already at"),
        ("9999.0.0", false, "refusing to downgrade"),
        ("local-test", false, "cannot compare"),
    ] {
        let env_text = format!("KKT_VERSION={version}\nCOMPOSE_FILE=compose.yaml\n");
        std::fs::write(folder.join(".env"), &env_text).unwrap();
        let before = std::fs::read(home.join(".config/kkt/setup.toml")).unwrap();
        let out = run(&home, &["upgrade"]);
        assert_eq!(out.status.success(), success, "{}", text(&out));
        assert!(text(&out).contains(expected), "{}", text(&out));
        assert!(
            !home.join("commands").exists(),
            "no backup/pull/restart for skipped or refused upgrades"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join(".env")).unwrap(),
            env_text
        );
        assert_eq!(
            std::fs::read(home.join(".config/kkt/setup.toml")).unwrap(),
            before
        );
    }
    std::fs::write(
        folder.join(".env"),
        "KKT_VERSION=9999.0.0\nCOMPOSE_FILE=compose.yaml\n",
    )
    .unwrap();
    let out = run(&home, &["upgrade", "--allow-downgrade"]);
    assert!(out.status.success(), "{}", text(&out));
    let log = std::fs::read_to_string(home.join("commands")).unwrap();
    assert!(log.contains("kkt backup /backups"));
    assert!(log.contains("pull"));
    let out = run(&home, &["upgrade", "--allow-downgrade"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("already at"));
    assert_eq!(std::fs::read_to_string(home.join("commands")).unwrap(), log);
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn a_relative_backup_folder_is_made_where_kkt_was_run_not_in_the_managed_folder() {
    let home = home("docker-relative");
    let folder = docker_record(&home);
    let elsewhere = home.join("where-i-am");
    std::fs::create_dir_all(&elsewhere).unwrap();
    fake(
        &home,
        "docker",
        "case \" $* \" in\n*' exec '*) echo 'Backed up to /backups/kkt-fixture' ;;\nesac\nexit 0",
    );
    let out = run_in(&home, &elsewhere, &["backup", "export"]);
    assert!(out.status.success(), "{}", text(&out));
    let wanted = std::fs::canonicalize(&elsewhere).unwrap().join("export");
    let log = std::fs::read_to_string(home.join("commands")).unwrap();
    let copy = log.lines().find(|l| l.contains(" cp ")).unwrap();
    assert!(
        copy.ends_with(wanted.to_str().unwrap()),
        "{copy} should end with {}",
        wanted.display()
    );
    assert!(wanted.is_dir());
    assert!(!folder.join("export").exists());
    std::fs::remove_dir_all(home).unwrap();
}
