//! Running the server as a background service of the user's login: a
//! launchd user agent on macOS, a systemd user unit on Linux.
//!
//! The service files are written here rather than through a library. They
//! are short; the restart policy we want (start again only after a failure)
//! can't be expressed through the cross-platform crates for launchd; and
//! `status` needs code for each platform anyway.
//!
//! Everything that touches the system (files, `launchctl`, `systemctl`,
//! `journalctl`, `tail`, `id`) goes through [`System`], so tests use a fake
//! and never start a real service. [`OsSystem`] is the real one; [`DryRun`]
//! reports what would change without changing anything.

mod launchd;
mod os;
mod systemd;
#[cfg(test)]
mod tests;

pub use os::{DryRun, OsSystem};

use crate::settings::Env;
use launchd::Launchd;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use systemd::Systemd;

/// What the service runs, and what it needs in place first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The absolute path of the program.
    pub program: PathBuf,
    /// Its arguments. Paths among them should be absolute: services start
    /// in a working directory of the manager's choosing.
    pub args: Vec<String>,
    /// Directories the server needs that it won't create itself, such as
    /// the database's.
    pub dirs: Vec<PathBuf>,
}

/// The service manager of one platform.
pub enum Manager {
    Launchd(Launchd),
    Systemd(Systemd),
}

/// What the system must do for the service. The real implementation is
/// [`OsSystem`].
pub trait System {
    /// The user's numeric ID, which names their launchd domain.
    fn user_id(&self) -> io::Result<u32>;
    fn exists(&self, path: &Path) -> bool;
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    fn write_file(&self, path: &Path, contents: &str) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Runs `command` and collects its output.
    fn run(&self, command: &Command) -> io::Result<Finished>;
    /// Runs `command` with its output going straight to the terminal, as
    /// for a log someone is following. The returned output is empty.
    fn run_attached(&self, command: &Command) -> io::Result<Finished>;
}

/// A program to run and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: &'static str,
    pub args: Vec<String>,
    /// True if running it only asks a question and changes nothing, so a
    /// dry run may still run it.
    pub query: bool,
}

impl Command {
    /// A command that changes something.
    fn new(program: &'static str, args: &[&str]) -> Command {
        Command {
            program,
            args: args.iter().map(|a| a.to_string()).collect(),
            query: false,
        }
    }

    /// A command that only asks a question.
    fn query(program: &'static str, args: &[&str]) -> Command {
        Command {
            query: true,
            ..Command::new(program, args)
        }
    }
}

/// Shown the way a person would type it, for messages.
impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program)?;
        for arg in &self.args {
            if arg.is_empty() || arg.contains(|c: char| c.is_whitespace() || c == '"') {
                write!(f, " {arg:?}")?;
            } else {
                write!(f, " {arg}")?;
            }
        }
        Ok(())
    }
}

/// How a command ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Finished {
    /// The exit status, or `None` if a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Finished {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Whether the command said `text`, in either stream.
    fn said(&self, text: &str) -> bool {
        self.stdout.contains(text) || self.stderr.contains(text)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("kkt service works with launchd on macOS and systemd on Linux, not on this system")]
    Unsupported,
    #[error("no home directory to install the service in: set HOME")]
    NoHome,
    #[error("the service is not installed; run `kkt service install` first")]
    NotInstalled,
    #[error("{value:?} can't go in a service file: {reason}")]
    BadValue { value: String, reason: &'static str },
    #[error("could not {action} {}: {source}", path.display())]
    File {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not run `{command}`: {source}")]
    Run { command: String, source: io::Error },
    #[error("`{command}` failed: {detail}")]
    Failed { command: String, detail: String },
}

impl Manager {
    /// The service manager of the system this binary was built for.
    pub fn for_this_os(env: &Env) -> Result<Manager, ServiceError> {
        if cfg!(target_os = "macos") {
            let home = env.home.as_ref().ok_or(ServiceError::NoHome)?;
            Ok(Manager::launchd(home))
        } else if cfg!(target_os = "linux") {
            let config_home = env.config_home().ok_or(ServiceError::NoHome)?;
            Ok(Manager::systemd(&config_home))
        } else {
            Err(ServiceError::Unsupported)
        }
    }

    /// launchd, for the user whose home directory is `home`.
    pub fn launchd(home: &Path) -> Manager {
        Manager::Launchd(Launchd::new(home))
    }

    /// systemd, for the user whose config base (`$XDG_CONFIG_HOME`) is
    /// `config_home`.
    pub fn systemd(config_home: &Path) -> Manager {
        Manager::Systemd(Systemd::new(config_home))
    }

    /// The service file this manager writes for `spec`.
    pub fn render(&self, spec: &Spec) -> Result<String, ServiceError> {
        match self {
            Manager::Launchd(m) => m.render(spec),
            Manager::Systemd(m) => m.render(spec),
        }
    }

    /// Writes the service file, starts the service, and has it start at
    /// each login. Installing again replaces the file and restarts it.
    /// Returns what to tell the user.
    pub fn install(&self, system: &dyn System, spec: &Spec) -> Result<Vec<String>, ServiceError> {
        for dir in &spec.dirs {
            create_dir(system, dir)?;
        }
        match self {
            Manager::Launchd(m) => m.install(system, spec),
            Manager::Systemd(m) => m.install(system, spec),
        }
    }

    /// Stops the service and removes its file. The database, config and
    /// logs stay.
    pub fn uninstall(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        match self {
            Manager::Launchd(m) => m.uninstall(system),
            Manager::Systemd(m) => m.uninstall(system),
        }
    }

    pub fn start(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        match self {
            Manager::Launchd(m) => m.start(system),
            Manager::Systemd(m) => m.start(system),
        }
    }

    /// Stops the service until it is started again or the user next logs in.
    pub fn stop(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        match self {
            Manager::Launchd(m) => m.stop(system),
            Manager::Systemd(m) => m.stop(system),
        }
    }

    /// Whether the service is installed and running.
    pub fn status(&self, system: &dyn System) -> Result<Vec<String>, ServiceError> {
        match self {
            Manager::Launchd(m) => m.status(system),
            Manager::Systemd(m) => m.status(system),
        }
    }

    /// Shows the last `lines` lines of the service's log, and keeps showing
    /// new ones if `follow`.
    pub fn logs(
        &self,
        system: &dyn System,
        lines: usize,
        follow: bool,
    ) -> Result<Vec<String>, ServiceError> {
        match self {
            Manager::Launchd(m) => m.logs(system, lines, follow),
            Manager::Systemd(m) => m.logs(system, lines, follow),
        }
    }
}

fn create_dir(system: &dyn System, dir: &Path) -> Result<(), ServiceError> {
    system
        .create_dir_all(dir)
        .map_err(|source| ServiceError::File {
            action: "create",
            path: dir.to_path_buf(),
            source,
        })
}

fn write_file(system: &dyn System, path: &Path, contents: &str) -> Result<(), ServiceError> {
    system
        .write_file(path, contents)
        .map_err(|source| ServiceError::File {
            action: "write",
            path: path.to_path_buf(),
            source,
        })
}

/// Removes `path` if it is there.
fn remove_file(system: &dyn System, path: &Path) -> Result<bool, ServiceError> {
    if !system.exists(path) {
        return Ok(false);
    }
    system
        .remove_file(path)
        .map_err(|source| ServiceError::File {
            action: "remove",
            path: path.to_path_buf(),
            source,
        })?;
    Ok(true)
}

/// Runs `command` and returns how it ended, failed or not.
fn run(system: &dyn System, command: &Command) -> Result<Finished, ServiceError> {
    system.run(command).map_err(|source| ServiceError::Run {
        command: command.to_string(),
        source,
    })
}

/// Runs `command`, which must succeed.
fn run_ok(system: &dyn System, command: &Command) -> Result<Finished, ServiceError> {
    let finished = run(system, command)?;
    if finished.success() {
        Ok(finished)
    } else {
        Err(ServiceError::Failed {
            command: command.to_string(),
            detail: failure_detail(&finished),
        })
    }
}

/// Runs `command` attached to the terminal. It must succeed, or exit with
/// one of `answers`: statuses that are answers rather than failures, such
/// as systemctl's for a unit that isn't running.
fn run_attached(
    system: &dyn System,
    command: &Command,
    answers: &[i32],
) -> Result<Finished, ServiceError> {
    let finished = system
        .run_attached(command)
        .map_err(|source| ServiceError::Run {
            command: command.to_string(),
            source,
        })?;
    match finished.code {
        Some(0) => Ok(finished),
        Some(code) if answers.contains(&code) => Ok(finished),
        _ => Err(ServiceError::Failed {
            command: command.to_string(),
            detail: failure_detail(&finished),
        }),
    }
}

/// What a failed command said, for an error message.
fn failure_detail(finished: &Finished) -> String {
    let said = [finished.stderr.trim(), finished.stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty());
    match (said, finished.code) {
        (Some(said), _) => said.to_string(),
        (None, Some(code)) => format!("exit status {code}"),
        (None, None) => "ended by a signal".to_string(),
    }
}

/// A path as service-file text. Paths that aren't valid UTF-8 can't be
/// written into one.
fn path_text(path: &Path) -> Result<&str, ServiceError> {
    path.to_str().ok_or_else(|| ServiceError::BadValue {
        value: path.display().to_string(),
        reason: "it is not valid UTF-8",
    })
}

/// Refuses text with control characters (such as newlines), which neither
/// a plist nor a unit file can hold safely.
fn plain_text(text: &str) -> Result<&str, ServiceError> {
    if text.chars().any(char::is_control) {
        return Err(ServiceError::BadValue {
            value: text.to_string(),
            reason: "it contains a control character such as a newline",
        });
    }
    Ok(text)
}
