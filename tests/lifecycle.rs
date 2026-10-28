//! Mode dispatch with an isolated home and fake external commands.
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
        written_by: "old-version".into(),
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_kkt"));
    command
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME");
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
    std::fs::write(folder.join(".env"), "KKT_VERSION=old\nCOMPOSE_FILE=compose.yaml\nCOMPOSE_PROJECT_NAME=isolated\nKKT_PORT=7421\n").unwrap();
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
    let out = run(&home, &["uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!unit.exists());
    assert!(!home.join(".config/kkt/setup.toml").exists());
    assert!(database.exists());
    assert!(saved.exists());
    std::fs::remove_dir_all(home).unwrap();
}
