//! kkt in Docker: the Compose files that come with this program, the folder
//! kkt manages for them, and the commands that set it up, start it, back it
//! up, upgrade it and remove it. All of it goes through [`Host`], so it is
//! tested against a fake host.
//!
//! This part only runs Docker. It does not decide what a person wants (the
//! setup answers) or remember what was set up (the setup record); the caller
//! passes those in.

mod compose;
mod files;
mod lifecycle;
mod login;
mod os;
mod provision;
mod tailscale;
#[cfg(test)]
mod tests;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub use files::{ENV_FILE, env_text, version_in_env};
pub use lifecycle::{State, backup, down, status, up, upgrade};
pub use login::LoginLink;
pub use os::OsHost;
pub use provision::{Provisioned, provision};

/// What the host must do for Docker setup. The real implementation is
/// [`OsHost`].
pub trait Host {
    /// Runs `command` and collects its output.
    fn run(&self, command: &Command) -> io::Result<Finished>;
    /// Runs `command` with its output going straight to the terminal, as for
    /// a download someone is waiting on. The returned output is empty.
    fn run_attached(&self, command: &Command) -> io::Result<Finished>;
    fn exists(&self, path: &Path) -> bool;
    fn read_file(&self, path: &Path) -> io::Result<String>;
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Writes `contents`, replacing any file there. With `private` only the
    /// owner may read it, whether or not the file existed.
    fn write_file(&self, path: &Path, contents: &str, private: bool) -> io::Result<()>;
    /// Tells the person one line.
    fn say(&self, line: &str);
    /// The folder the person ran kkt in, which paths they typed are relative
    /// to. Docker commands run elsewhere, in the managed folder.
    fn working_dir(&self) -> io::Result<PathBuf>;
}

/// A program to run and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: &'static str,
    pub args: Vec<String>,
    /// Environment variables the command must run without.
    pub unset: Vec<String>,
    /// The folder to run in. Compose finds the files `COMPOSE_FILE` names
    /// relative to where it runs, not to `--project-directory`, so Compose
    /// commands run in the managed folder.
    pub current_dir: Option<PathBuf>,
}

impl Command {
    /// A `docker` command that does not see the variables Compose would let
    /// override the managed folder's settings file.
    fn docker(args: Vec<String>) -> Command {
        Command {
            program: "docker",
            args,
            unset: crate::config::DOCKER_MANAGED_VARIABLES
                .iter()
                .map(|v| v.to_string())
                .collect(),
            current_dir: None,
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

    /// What the command said when it failed, for a message.
    fn detail(&self) -> String {
        let said = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        match self.code {
            Some(code) => format!("exit {code}: {}", said.trim()),
            None => format!("ended by a signal: {}", said.trim()),
        }
    }
}

/// The folder kkt manages for the Compose files, the settings file and the
/// backups folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder(pub PathBuf);

impl Folder {
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The backups folder on the host, mounted in the container.
    pub fn backups(&self) -> PathBuf {
        self.0.join(crate::config::DOCKER_BACKUPS_DIR)
    }
}

/// What to set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// The kkt version whose image to run, such as `0.4.0`.
    pub version: String,
    /// The Compose project name, which names the containers and volumes.
    /// [`crate::config::DOCKER_PROJECT`] unless testing beside a real setup.
    pub project: String,
    /// The port on the host's `127.0.0.1` that kkt is published on.
    /// [`crate::config::DEFAULT_BIND`]'s port unless testing beside a real
    /// setup.
    pub port: u16,
    /// Serve over Tailscale, if given.
    pub tailscale: Option<Tailscale>,
}

/// Tailscale settings for [`Inputs`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tailscale {
    /// The name asked for on the tailnet.
    pub hostname: String,
    /// An auth key, if there is one. Without it the person logs in with a
    /// link, which setup shows.
    pub auth_key: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error("Docker is not available: {0}")]
    NotAvailable(String),
    #[error("`{command}` failed ({detail})")]
    Command { command: String, detail: String },
    #[error("{what}: {source}")]
    Io {
        what: String,
        #[source]
        source: io::Error,
    },
    #[error(
        "Tailscale is not logged in yet. Run setup again to continue; the containers are left as they are"
    )]
    LoginTimeout,
    #[error(
        "HTTPS certificates are not enabled for your tailnet. Turn them on at https://login.tailscale.com/admin/dns (\"Enable HTTPS\"), then run setup again"
    )]
    HttpsDisabled,
    #[error(
        "kkt did not start or did not make its access key in time; `docker compose logs kkt` shows why"
    )]
    StartTimeout,
    #[error("unexpected output from {what}: {detail}")]
    Unexpected { what: &'static str, detail: String },
}

impl DockerError {
    fn io(what: impl Into<String>, source: io::Error) -> DockerError {
        DockerError::Io {
            what: what.into(),
            source,
        }
    }
}
