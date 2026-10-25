//! A systemd user unit on Linux. The unit starts the server at login and
//! again only if it fails; its output goes to the journal.

use super::{
    Command, ServiceError, Spec, System, create_dir, path_text, plain_text, remove_file,
    run_attached, run_ok, write_file,
};
use crate::config::{SERVICE_RESTART_DELAY, SERVICE_STOP_TIMEOUT, SYSTEMD_UNIT, SYSTEMD_USER_DIR};
use std::path::{Path, PathBuf};

/// `systemctl status`'s exit status for a unit that is loaded but not
/// running (stopped or failed): an answer, not a failure.
const INACTIVE: i32 = 3;

pub struct Systemd {
    unit_dir: PathBuf,
}

impl Systemd {
    pub fn new(config_home: &Path) -> Systemd {
        Systemd {
            unit_dir: config_home.join(SYSTEMD_USER_DIR),
        }
    }

    fn unit(&self) -> PathBuf {
        self.unit_dir.join(SYSTEMD_UNIT)
    }

    pub fn render(&self, spec: &Spec) -> Result<String, ServiceError> {
        let mut exec = quote(path_text(&spec.program)?)?;
        for arg in &spec.args {
            exec.push(' ');
            exec.push_str(&quote(arg)?);
        }
        Ok(format!(
            "# Written by `kkt service install`. Install again to change it.
[Unit]
Description=Kashkeshet (kkt), a chat server for AI agents
Documentation=https://github.com/sam-mfb/kashkeshet

[Service]
Type=exec
ExecStart={exec}
Restart=on-failure
RestartSec={restart}
KillSignal=SIGTERM
TimeoutStopSec={stop}

[Install]
WantedBy=default.target
",
            restart = SERVICE_RESTART_DELAY.as_secs(),
            stop = SERVICE_STOP_TIMEOUT.as_secs(),
        ))
    }

    fn systemctl(args: &[&str]) -> Command {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        Command::new("systemctl", &all)
    }

    pub fn install(&self, system: &dyn System, spec: &Spec) -> Result<Vec<String>, ServiceError> {
        let unit = self.render(spec)?;
        create_dir(system, &self.unit_dir)?;
        write_file(system, &self.unit(), &unit)?;
        run_ok(system, &Self::systemctl(&["daemon-reload"]))?;
        run_ok(system, &Self::systemctl(&["enable", SYSTEMD_UNIT]))?;
        // Restart, not start, so a reinstall runs the new unit.
        run_ok(system, &Self::systemctl(&["restart", SYSTEMD_UNIT]))?;
        Ok(vec![
            format!("Installed {}", self.unit().display()),
            "Started kashkeshet; it starts again at each login.".to_string(),
            "Log: `kkt service logs` (the systemd journal).".to_string(),
            "To keep it running while you are logged out: loginctl enable-linger".to_string(),
        ])
    }

    pub fn uninstall(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        if !system.exists(&self.unit()) {
            return Ok(vec!["The service was not installed.".to_string()]);
        }
        run_ok(
            system,
            &Self::systemctl(&["disable", "--now", SYSTEMD_UNIT]),
        )?;
        remove_file(system, &self.unit())?;
        run_ok(system, &Self::systemctl(&["daemon-reload"]))?;
        Ok(vec![
            "Stopped kashkeshet.".to_string(),
            format!("Removed {}", self.unit().display()),
        ])
    }

    pub fn start(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        if !system.exists(&self.unit()) {
            return Err(ServiceError::NotInstalled);
        }
        run_ok(system, &Self::systemctl(&["start", SYSTEMD_UNIT]))?;
        Ok(vec!["Started kashkeshet.".to_string()])
    }

    pub fn stop(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        if !system.exists(&self.unit()) {
            return Err(ServiceError::NotInstalled);
        }
        run_ok(system, &Self::systemctl(&["stop", SYSTEMD_UNIT]))?;
        Ok(vec![
            "Stopped kashkeshet. It starts again at the next login, or with `kkt service start`."
                .to_string(),
        ])
    }

    pub fn status(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        let unit = self.unit();
        if !system.exists(&unit) {
            return Ok(vec![format!(
                "Not installed (no {}). Run `kkt service install`.",
                unit.display()
            )]);
        }
        // systemctl prints the state itself.
        run_attached(
            system,
            &Command::query(
                "systemctl",
                &["--user", "status", SYSTEMD_UNIT, "--no-pager"],
            ),
            &[INACTIVE],
        )?;
        Ok(vec![format!("Installed: {}", unit.display())])
    }

    pub fn logs(
        &self,
        system: &dyn System,
        lines: usize,
        follow: bool,
    ) -> Result<Vec<String>, ServiceError> {
        let lines = lines.to_string();
        let mut args = vec!["--user", "-u", SYSTEMD_UNIT, "-n", &lines, "--no-pager"];
        if follow {
            args.push("-f");
        }
        run_attached(system, &Command::query("journalctl", &args), &[])?;
        Ok(Vec::new())
    }
}

/// One `ExecStart=` argument: double-quoted, with backslashes and quotes
/// escaped, and `%` and `$` doubled so systemd takes them literally rather
/// than as specifiers or variables.
fn quote(arg: &str) -> Result<String, ServiceError> {
    let mut quoted = String::from("\"");
    for c in plain_text(arg)?.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '$' => quoted.push_str("$$"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    Ok(quoted)
}
