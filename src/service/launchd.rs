//! A launchd user agent on macOS. The plist starts the server at login and
//! again only if it exits with a failure; its output goes to a log file.
//! It runs in the user's `gui/<uid>` domain.

use super::{
    Command, ServiceError, Spec, System, create_dir, failure_detail, path_text, plain_text,
    remove_file, run, run_attached, run_ok, write_file,
};
use crate::config::{
    LAUNCH_AGENTS_DIR, LAUNCHD_LABEL, MACOS_LOG_DIR, MACOS_LOG_FILE_NAME, SERVICE_RESTART_DELAY,
    SERVICE_STOP_TIMEOUT,
};
use std::path::{Path, PathBuf};

/// `launchctl print`'s exit status for a service it doesn't have loaded.
const NOT_FOUND: i32 = 113;
/// `launchctl kill`'s exit status when the service has no process (ESRCH).
const NO_PROCESS: i32 = 3;

pub struct Launchd {
    home: PathBuf,
    agents_dir: PathBuf,
    log_dir: PathBuf,
}

impl Launchd {
    pub fn new(home: &Path) -> Launchd {
        Launchd {
            home: home.to_path_buf(),
            agents_dir: home.join(LAUNCH_AGENTS_DIR),
            log_dir: home.join(MACOS_LOG_DIR),
        }
    }

    fn plist(&self) -> PathBuf {
        self.agents_dir.join(format!("{LAUNCHD_LABEL}.plist"))
    }

    fn log_file(&self) -> PathBuf {
        self.log_dir.join(MACOS_LOG_FILE_NAME)
    }

    pub fn render(&self, spec: &Spec) -> Result<String, ServiceError> {
        let mut arguments = format!("    <string>{}</string>\n", xml(path_text(&spec.program)?)?);
        for arg in &spec.args {
            arguments.push_str(&format!("    <string>{}</string>\n", xml(arg)?));
        }
        let log = xml(path_text(&self.log_file())?)?;
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by `kkt service install`. Install again to change it. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}  </array>
  <key>WorkingDirectory</key>
  <string>{home}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>{restart}</integer>
  <key>ExitTimeOut</key>
  <integer>{stop}</integer>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
            label = xml(LAUNCHD_LABEL)?,
            home = xml(path_text(&self.home)?)?,
            restart = SERVICE_RESTART_DELAY.as_secs(),
            stop = SERVICE_STOP_TIMEOUT.as_secs(),
        ))
    }

    /// The user's launchd domain, `gui/<uid>`.
    fn domain(&self, system: &dyn System) -> Result<String, ServiceError> {
        let uid = system.user_id().map_err(|source| ServiceError::Run {
            command: "id -u".to_string(),
            source,
        })?;
        Ok(format!("gui/{uid}"))
    }

    /// This service in the user's domain, `gui/<uid>/<label>`.
    fn target(&self, system: &dyn System) -> Result<String, ServiceError> {
        Ok(format!("{}/{LAUNCHD_LABEL}", self.domain(system)?))
    }

    fn bootstrap(&self, system: &dyn System) -> Result<(), ServiceError> {
        let domain = self.domain(system)?;
        let plist = path_text(&self.plist())?.to_string();
        run_ok(
            system,
            &Command::new("launchctl", &["bootstrap", &domain, &plist]),
        )?;
        Ok(())
    }

    /// What `launchctl print` says about the service, or `None` if launchd
    /// doesn't have it loaded. Any other failure is an error.
    fn print(&self, system: &dyn System) -> Result<Option<String>, ServiceError> {
        let target = self.target(system)?;
        let print = Command::query("launchctl", &["print", &target]);
        let finished = run(system, &print)?;
        if finished.success() {
            Ok(Some(finished.stdout))
        } else if finished.code == Some(NOT_FOUND) || finished.said("Could not find service") {
            Ok(None)
        } else {
            Err(ServiceError::Failed {
                command: print.to_string(),
                detail: failure_detail(&finished),
            })
        }
    }

    /// Whether launchd has the service loaded (installed and not booted out).
    fn loaded(&self, system: &dyn System) -> Result<bool, ServiceError> {
        Ok(self.print(system)?.is_some())
    }

    pub fn install(&self, system: &dyn System, spec: &Spec) -> Result<Vec<String>, ServiceError> {
        let plist = self.render(spec)?;
        create_dir(system, &self.agents_dir)?;
        create_dir(system, &self.log_dir)?;
        write_file(system, &self.plist(), &plist)?;
        // launchd keeps the plist it loaded, so a loaded one is unloaded
        // (which stops it) before the new one is loaded.
        if self.loaded(system)? {
            let target = self.target(system)?;
            run_ok(system, &Command::new("launchctl", &["bootout", &target]))?;
        }
        self.bootstrap(system)?;
        Ok(vec![
            format!("Installed {}", self.plist().display()),
            "Started kashkeshet; it starts again at each login.".to_string(),
            format!("Log: {}", self.log_file().display()),
        ])
    }

    pub fn uninstall(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        let mut said = Vec::new();
        if self.loaded(system)? {
            let target = self.target(system)?;
            run_ok(system, &Command::new("launchctl", &["bootout", &target]))?;
            said.push("Stopped kashkeshet.".to_string());
        }
        if remove_file(system, &self.plist())? {
            said.push(format!("Removed {}", self.plist().display()));
        } else {
            said.push("The service was not installed.".to_string());
        }
        Ok(said)
    }

    pub fn start(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        if !system.exists(&self.plist()) {
            return Err(ServiceError::NotInstalled);
        }
        if self.loaded(system)? {
            let target = self.target(system)?;
            run_ok(system, &Command::new("launchctl", &["kickstart", &target]))?;
        } else {
            // Loading it starts it, since the plist says RunAtLoad.
            self.bootstrap(system)?;
        }
        Ok(vec!["Started kashkeshet.".to_string()])
    }

    pub fn stop(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        if !system.exists(&self.plist()) {
            return Err(ServiceError::NotInstalled);
        }
        if !self.loaded(system)? {
            return Ok(vec!["kashkeshet was not running.".to_string()]);
        }
        // The server exits cleanly on SIGTERM, and launchd starts it again
        // only after a failure, so it stays stopped until the next login.
        let target = self.target(system)?;
        let kill = Command::new("launchctl", &["kill", "SIGTERM", &target]);
        let finished = run(system, &kill)?;
        if finished.success() {
            Ok(vec![
                "Stopped kashkeshet. It starts again at the next login, or with `kkt service start`."
                    .to_string(),
            ])
        } else if finished.code == Some(NO_PROCESS) || finished.said("No such process") {
            Ok(vec!["kashkeshet was not running.".to_string()])
        } else {
            Err(ServiceError::Failed {
                command: kill.to_string(),
                detail: failure_detail(&finished),
            })
        }
    }

    pub fn status(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        let plist = self.plist();
        if !system.exists(&plist) {
            return Ok(vec![format!(
                "Not installed (no {}). Run `kkt service install`.",
                plist.display()
            )]);
        }
        let mut said = vec![format!("Installed: {}", plist.display())];
        match self.print(system)? {
            None => {
                said.push("Not loaded, so not running. Run `kkt service start`.".to_string());
            }
            Some(printed) => {
                let state = PrintedState::read(&printed);
                said.push(match (state.state, state.pid) {
                    (Some(s), Some(pid)) => format!("State: {s} (pid {pid})"),
                    (Some(s), None) => format!("State: {s}"),
                    (None, _) => "State: unknown (launchctl print did not say)".to_string(),
                });
                if let Some(code) = state.last_exit {
                    said.push(format!("Last exit: {code}"));
                }
            }
        }
        said.push(format!("Log: {}", self.log_file().display()));
        Ok(said)
    }

    pub fn logs(
        &self,
        system: &dyn System,
        lines: usize,
        follow: bool,
    ) -> Result<Vec<String>, ServiceError> {
        let log = self.log_file();
        if !system.exists(&log) {
            return Ok(vec![format!("No log yet at {}", log.display())]);
        }
        let lines = lines.to_string();
        let log_text = path_text(&log)?;
        let mut args = vec!["-n", &lines];
        if follow {
            // -F keeps following if the file is replaced.
            args.push("-F");
        }
        args.push(log_text);
        run_attached(system, &Command::query("tail", &args), &[])?;
        Ok(Vec::new())
    }
}

/// The parts of `launchctl print` that `status` reports.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct PrintedState {
    pub state: Option<String>,
    pub pid: Option<String>,
    pub last_exit: Option<String>,
}

impl PrintedState {
    /// Reads `key = value` lines, keeping the first of each key: later ones
    /// belong to nested sections.
    pub fn read(printed: &str) -> PrintedState {
        let mut found = PrintedState::default();
        for line in printed.lines() {
            let Some((key, value)) = line.trim().split_once(" = ") else {
                continue;
            };
            let slot = match key {
                "state" => &mut found.state,
                "pid" => &mut found.pid,
                "last exit code" => &mut found.last_exit,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(value.trim().to_string());
            }
        }
        found
    }
}

/// `text` escaped for an XML element.
fn xml(text: &str) -> Result<String, ServiceError> {
    Ok(plain_text(text)?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}
