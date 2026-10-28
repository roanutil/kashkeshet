use super::launchd::PrintedState;
use super::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A system that records what is asked of it. Commands succeed with no
/// output unless given an answer.
#[derive(Default)]
struct Fake {
    files: RefCell<BTreeMap<PathBuf, String>>,
    dirs: RefCell<BTreeSet<PathBuf>>,
    ran: RefCell<Vec<String>>,
    answers: HashMap<String, Finished>,
}

impl Fake {
    fn answer(mut self, command: &str, code: i32, stdout: &str, stderr: &str) -> Fake {
        self.answers.insert(
            command.to_string(),
            Finished {
                code: Some(code),
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            },
        );
        self
    }

    /// launchd answers that it doesn't have the service loaded.
    fn not_loaded(self) -> Fake {
        self.answer(
            PRINT,
            113,
            "",
            "Bad request.\nCould not find service \"com.sam-mfb.kashkeshet\" in domain for user gui: 501\n",
        )
    }

    fn with_file(self, path: &str) -> Fake {
        self.files
            .borrow_mut()
            .insert(PathBuf::from(path), String::new());
        self
    }

    fn ran(&self) -> Vec<String> {
        self.ran.borrow().clone()
    }

    fn file(&self, path: &str) -> Option<String> {
        self.files.borrow().get(Path::new(path)).cloned()
    }
}

impl System for Fake {
    fn user_id(&self) -> io::Result<u32> {
        Ok(501)
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path) || self.dirs.borrow().contains(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.dirs.borrow_mut().insert(path.to_path_buf());
        Ok(())
    }

    fn write_file(&self, path: &Path, contents: &str) -> io::Result<()> {
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.files.borrow_mut().remove(path);
        Ok(())
    }

    fn run(&self, command: &Command) -> io::Result<Finished> {
        let shown = command.to_string();
        self.ran.borrow_mut().push(shown.clone());
        Ok(self.answers.get(&shown).cloned().unwrap_or(Finished {
            code: Some(0),
            ..Finished::default()
        }))
    }

    fn run_attached(&self, command: &Command) -> io::Result<Finished> {
        self.run(command).map(|f| Finished {
            code: f.code,
            ..Finished::default()
        })
    }
}

const PLIST: &str = "/Users/sam/Library/LaunchAgents/com.sam-mfb.kashkeshet.plist";
const MAC_LOG: &str = "/Users/sam/Library/Logs/kashkeshet/kashkeshet.log";
const PRINT: &str = "launchctl print gui/501/com.sam-mfb.kashkeshet";
const UNIT: &str = "/home/sam/.config/systemd/user/kashkeshet.service";

fn mac() -> Manager {
    Manager::launchd(Path::new("/Users/sam"))
}

fn linux() -> Manager {
    Manager::systemd(Path::new("/home/sam/.config"))
}

fn mac_spec() -> Spec {
    Spec {
        program: PathBuf::from("/Users/sam/.cargo/bin/kkt"),
        args: [
            "serve",
            "--ignore-setup",
            "--config",
            "/Users/sam/.config/kkt/config.toml",
            "--db",
            "/Users/sam/Library/Application Support/kashkeshet/kashkeshet.db",
        ]
        .map(String::from)
        .to_vec(),
        dirs: vec![PathBuf::from(
            "/Users/sam/Library/Application Support/kashkeshet",
        )],
    }
}

fn linux_spec() -> Spec {
    Spec {
        program: PathBuf::from("/home/sam/.cargo/bin/kkt"),
        args: [
            "serve",
            "--ignore-setup",
            "--config",
            "/home/sam/.config/kkt/config.toml",
            "--db",
            "/home/sam/.local/share/kashkeshet/kashkeshet.db",
        ]
        .map(String::from)
        .to_vec(),
        dirs: vec![PathBuf::from("/home/sam/.local/share/kashkeshet")],
    }
}

/// Compares `actual` with the golden file `name`. With UPDATE_GOLDEN=1,
/// rewrites the file instead; review the diff.
fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/service/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).expect("write golden file");
        return;
    }
    let expected = std::fs::read_to_string(&path).expect("read golden file");
    assert_eq!(
        actual, expected,
        "{name} changed; if intended, run UPDATE_GOLDEN=1 cargo test and review the diff"
    );
}

#[test]
fn the_plist_matches_its_golden_file() {
    golden(
        "com.sam-mfb.kashkeshet.plist",
        &mac().render(&mac_spec()).expect("plist"),
    );
}

#[test]
fn the_unit_matches_its_golden_file() {
    golden(
        "kashkeshet.service",
        &linux().render(&linux_spec()).expect("unit"),
    );
}

#[test]
fn plist_values_are_escaped() {
    let spec = Spec {
        program: PathBuf::from("/opt/a&b/kkt"),
        args: vec!["<x>".into(), "it's \"q\"".into()],
        dirs: vec![],
    };
    let plist = mac().render(&spec).expect("plist");
    assert!(
        plist.contains("<string>/opt/a&amp;b/kkt</string>"),
        "{plist}"
    );
    assert!(plist.contains("<string>&lt;x&gt;</string>"), "{plist}");
    assert!(
        plist.contains("<string>it&apos;s &quot;q&quot;</string>"),
        "{plist}"
    );
}

#[test]
fn unit_arguments_are_quoted_and_literal() {
    let spec = Spec {
        program: PathBuf::from("/opt/my kkt/kkt"),
        args: vec!["50%".into(), "$HOME".into(), r#"a\"b"#.into()],
        dirs: vec![],
    };
    let unit = linux().render(&spec).expect("unit");
    assert!(
        unit.contains(r#"ExecStart="/opt/my kkt/kkt" "50%%" "$$HOME" "a\\\"b""#),
        "{unit}"
    );
}

#[test]
fn control_characters_are_refused() {
    let spec = Spec {
        program: PathBuf::from("/opt/kkt"),
        args: vec!["--db".into(), "a\nExecStartPre=/bin/evil".into()],
        dirs: vec![],
    };
    for manager in [mac(), linux()] {
        assert!(matches!(
            manager.render(&spec),
            Err(ServiceError::BadValue { .. })
        ));
    }
}

#[test]
fn launchd_install_writes_the_plist_and_loads_it() {
    let system = Fake::default().not_loaded();
    let said = mac().install(&system, &mac_spec()).expect("install");
    assert_eq!(
        system.file(PLIST),
        Some(mac().render(&mac_spec()).expect("plist"))
    );
    for dir in [
        "/Users/sam/Library/LaunchAgents",
        "/Users/sam/Library/Logs/kashkeshet",
        "/Users/sam/Library/Application Support/kashkeshet",
    ] {
        assert!(system.exists(Path::new(dir)), "{dir}");
    }
    assert_eq!(
        system.ran(),
        [PRINT, &format!("launchctl bootstrap gui/501 {PLIST}")]
    );
    assert!(said.iter().any(|l| l.contains(MAC_LOG)), "{said:?}");
}

#[test]
fn launchd_install_again_unloads_the_old_one_first() {
    let system = Fake::default();
    mac().install(&system, &mac_spec()).expect("install");
    assert_eq!(
        system.ran(),
        [
            PRINT,
            "launchctl bootout gui/501/com.sam-mfb.kashkeshet",
            &format!("launchctl bootstrap gui/501 {PLIST}"),
        ]
    );
}

#[test]
fn a_failed_command_is_an_error_saying_what_it_said() {
    let bootstrap = format!("launchctl bootstrap gui/501 {PLIST}");
    let system = Fake::default().not_loaded().answer(
        &bootstrap,
        5,
        "",
        "Bootstrap failed: 5: Input/output error\n",
    );
    let err = mac().install(&system, &mac_spec()).expect_err("fails");
    assert_eq!(
        err.to_string(),
        format!("`{bootstrap}` failed: Bootstrap failed: 5: Input/output error")
    );
}

#[test]
fn launchd_uninstall_unloads_and_removes() {
    let system = Fake::default().with_file(PLIST);
    mac().uninstall(&system).expect("uninstall");
    assert_eq!(
        system.ran(),
        [PRINT, "launchctl bootout gui/501/com.sam-mfb.kashkeshet"]
    );
    assert_eq!(system.file(PLIST), None);

    let nothing = Fake::default().not_loaded();
    let said = mac().uninstall(&nothing).expect("uninstall");
    assert_eq!(said, ["The service was not installed."]);
}

#[test]
fn launchd_start_kicks_a_loaded_service_and_loads_an_unloaded_one() {
    let loaded = Fake::default().with_file(PLIST);
    mac().start(&loaded).expect("start");
    assert_eq!(
        loaded.ran(),
        [PRINT, "launchctl kickstart gui/501/com.sam-mfb.kashkeshet"]
    );

    let unloaded = Fake::default().with_file(PLIST).not_loaded();
    mac().start(&unloaded).expect("start");
    assert_eq!(
        unloaded.ran(),
        [PRINT, &format!("launchctl bootstrap gui/501 {PLIST}")]
    );
}

#[test]
fn start_and_stop_need_an_installed_service() {
    for manager in [mac(), linux()] {
        let system = Fake::default();
        assert!(matches!(
            manager.start(&system),
            Err(ServiceError::NotInstalled)
        ));
        assert!(matches!(
            manager.stop(&system),
            Err(ServiceError::NotInstalled)
        ));
        assert!(system.ran().is_empty());
    }
}

#[test]
fn launchd_stop_sends_sigterm() {
    let system = Fake::default().with_file(PLIST);
    mac().stop(&system).expect("stop");
    assert_eq!(
        system.ran(),
        [
            PRINT,
            "launchctl kill SIGTERM gui/501/com.sam-mfb.kashkeshet"
        ]
    );
}

#[test]
fn launchd_status_reports_state_pid_and_last_exit() {
    let printed = "gui/501/com.sam-mfb.kashkeshet = {\n\tactive count = 1\n\tstate = running\n\
                   \tpid = 4242\n\tlast exit code = 0\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n";
    let system = Fake::default()
        .with_file(PLIST)
        .answer(PRINT, 0, printed, "");
    let said = mac().status(&system).expect("status");
    assert_eq!(
        said,
        [
            format!("Installed: {PLIST}"),
            "State: running (pid 4242)".to_string(),
            "Last exit: 0".to_string(),
            format!("Log: {MAC_LOG}"),
        ]
    );
}

#[test]
fn launchd_status_of_a_missing_or_unloaded_service() {
    let said = mac().status(&Fake::default()).expect("status");
    assert!(said[0].starts_with("Not installed"), "{said:?}");

    let unloaded = Fake::default().with_file(PLIST).not_loaded();
    let said = mac().status(&unloaded).expect("status");
    assert!(said[1].starts_with("Not loaded"), "{said:?}");
}

#[test]
fn the_first_of_each_printed_key_wins() {
    let state = PrintedState::read("\tstate = waiting\n\t\tstate = active\n\tpid = 7\n");
    assert_eq!(state.state.as_deref(), Some("waiting"));
    assert_eq!(state.pid.as_deref(), Some("7"));
    assert_eq!(state.last_exit, None);
}

#[test]
fn launchd_logs_tail_the_log_file() {
    let system = Fake::default().with_file(MAC_LOG);
    mac().logs(&system, 50, true).expect("logs");
    assert_eq!(system.ran(), [format!("tail -n 50 -F {MAC_LOG}")]);

    let said = mac().logs(&Fake::default(), 50, false).expect("logs");
    assert_eq!(said, [format!("No log yet at {MAC_LOG}")]);
}

#[test]
fn systemd_install_writes_enables_and_restarts() {
    let system = Fake::default();
    linux().install(&system, &linux_spec()).expect("install");
    assert_eq!(
        system.file(UNIT),
        Some(linux().render(&linux_spec()).expect("unit"))
    );
    assert!(system.exists(Path::new("/home/sam/.local/share/kashkeshet")));
    assert_eq!(
        system.ran(),
        [
            "systemctl --user daemon-reload",
            "systemctl --user enable kashkeshet.service",
            "systemctl --user restart kashkeshet.service",
        ]
    );
}

#[test]
fn systemd_uninstall_disables_removes_and_reloads() {
    let system = Fake::default().with_file(UNIT);
    linux().uninstall(&system).expect("uninstall");
    assert_eq!(system.file(UNIT), None);
    assert_eq!(
        system.ran(),
        [
            "systemctl --user disable --now kashkeshet.service",
            "systemctl --user daemon-reload",
        ]
    );
    let said = linux().uninstall(&Fake::default()).expect("uninstall");
    assert_eq!(said, ["The service was not installed."]);
}

#[test]
fn systemd_start_stop_status_and_logs() {
    let system = Fake::default().with_file(UNIT).answer(
        "systemctl --user status kashkeshet.service --no-pager",
        3,
        "",
        "",
    );
    linux().start(&system).expect("start");
    linux().stop(&system).expect("stop");
    // A unit that isn't running makes systemctl exit 3; that is a status,
    // not an error.
    linux().status(&system).expect("status");
    linux().logs(&system, 20, false).expect("logs");
    linux().logs(&system, 20, true).expect("logs");
    assert_eq!(
        system.ran(),
        [
            "systemctl --user start kashkeshet.service",
            "systemctl --user stop kashkeshet.service",
            "systemctl --user status kashkeshet.service --no-pager",
            "journalctl --user -u kashkeshet.service -n 20 --no-pager",
            "journalctl --user -u kashkeshet.service -n 20 --no-pager -f",
        ]
    );
}

#[test]
fn a_dry_run_changes_nothing_but_still_asks() {
    let real = Fake::default().not_loaded();
    let dry = DryRun::new(&real);
    mac().install(&dry, &mac_spec()).expect("install");
    let actions = dry.actions();
    assert!(real.files.borrow().is_empty());
    assert!(real.dirs.borrow().is_empty());
    // The query ran for real; the change did not.
    assert_eq!(real.ran(), [PRINT]);
    assert!(
        actions
            .iter()
            .any(|a| a.starts_with(&format!("Would write {PLIST}:")))
    );
    assert!(actions.contains(&format!("Would run: launchctl bootstrap gui/501 {PLIST}")));
}

#[test]
fn commands_show_as_typed() {
    let command = Command::new("tail", &["-n", "5", "/a b/log"]);
    assert_eq!(command.to_string(), r#"tail -n 5 "/a b/log""#);
}

#[test]
fn launchctl_failures_other_than_not_loaded_are_errors() {
    let system = Fake::default()
        .with_file(PLIST)
        .answer(PRINT, 5, "", "Input/output error");
    for result in [
        mac().status(&system),
        mac().stop(&system),
        mac().start(&system),
        mac().install(&system, &mac_spec()),
    ] {
        let err = result.expect_err("a failed print is not \"not loaded\"");
        assert_eq!(
            err.to_string(),
            format!("`{PRINT}` failed: Input/output error")
        );
    }
}

#[test]
fn launchd_stop_tells_not_running_from_failure() {
    let kill = "launchctl kill SIGTERM gui/501/com.sam-mfb.kashkeshet";
    let idle = Fake::default()
        .with_file(PLIST)
        .answer(kill, 3, "", "No such process\n");
    assert_eq!(
        mac().stop(&idle).expect("stop"),
        ["kashkeshet was not running."]
    );
    let broken = Fake::default()
        .with_file(PLIST)
        .answer(kill, 1, "", "Operation not permitted\n");
    let err = mac().stop(&broken).expect_err("a failed kill is an error");
    assert_eq!(
        err.to_string(),
        format!("`{kill}` failed: Operation not permitted")
    );
}

#[test]
fn failed_log_and_status_commands_are_errors() {
    let tail = format!("tail -n 10 {MAC_LOG}");
    let mac_system = Fake::default().with_file(MAC_LOG).answer(&tail, 1, "", "");
    let err = mac().logs(&mac_system, 10, false).expect_err("tail failed");
    assert_eq!(err.to_string(), format!("`{tail}` failed: exit status 1"));

    let journal = "journalctl --user -u kashkeshet.service -n 10 --no-pager";
    let status = "systemctl --user status kashkeshet.service --no-pager";
    let linux_system = Fake::default()
        .with_file(UNIT)
        .answer(journal, 5, "", "")
        .answer(status, 1, "", "Failed to connect to bus\n");
    let err = linux()
        .logs(&linux_system, 10, false)
        .expect_err("journalctl failed");
    assert_eq!(
        err.to_string(),
        format!("`{journal}` failed: exit status 5")
    );
    let err = linux().status(&linux_system).expect_err("systemctl failed");
    assert_eq!(err.to_string(), format!("`{status}` failed: exit status 1"));
}

#[test]
fn launchd_state_reads_installed_and_running() {
    let none = mac().state(&Fake::default()).expect("state");
    assert_eq!(
        none,
        ServiceState {
            installed: false,
            running: false
        }
    );

    let running = Fake::default().with_file(PLIST).answer(
        PRINT,
        0,
        "gui/501/com.sam-mfb.kashkeshet = {\n\tstate = running\n\tpid = 7\n}\n",
        "",
    );
    assert_eq!(
        mac().state(&running).expect("state"),
        ServiceState {
            installed: true,
            running: true
        }
    );

    let waiting = Fake::default().with_file(PLIST).answer(
        PRINT,
        0,
        "gui/501/com.sam-mfb.kashkeshet = {\n\tstate = not running\n}\n",
        "",
    );
    assert!(!mac().state(&waiting).expect("state").running);

    let unloaded = Fake::default().with_file(PLIST).not_loaded();
    assert_eq!(
        mac().state(&unloaded).expect("state"),
        ServiceState {
            installed: true,
            running: false
        }
    );

    let broken = Fake::default()
        .with_file(PLIST)
        .answer(PRINT, 5, "", "Input/output error");
    assert!(matches!(
        mac().state(&broken),
        Err(ServiceError::Failed { .. })
    ));
}

#[test]
fn systemd_state_reads_is_active() {
    let is_active = "systemctl --user is-active kashkeshet.service";
    let missing = Fake::default();
    assert!(!linux().state(&missing).expect("state").installed);
    assert!(missing.ran().is_empty(), "no unit, so nothing to ask");

    let active = Fake::default()
        .with_file(UNIT)
        .answer(is_active, 0, "active\n", "");
    assert_eq!(
        linux().state(&active).expect("state"),
        ServiceState {
            installed: true,
            running: true
        }
    );
    let inactive = Fake::default()
        .with_file(UNIT)
        .answer(is_active, 3, "inactive\n", "");
    assert!(!linux().state(&inactive).expect("state").running);

    let broken =
        Fake::default()
            .with_file(UNIT)
            .answer(is_active, 1, "", "Failed to connect to bus\n");
    let err = linux()
        .state(&broken)
        .expect_err("a failed query is an error");
    assert_eq!(
        err.to_string(),
        format!("`{is_active}` failed: Failed to connect to bus")
    );
}
