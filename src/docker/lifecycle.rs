//! Looking at, backing up, upgrading, stopping and removing kkt in Docker.

use super::compose::compose;
use super::files::{ENV_FILE, set_version, version_in_env, write_embedded, write_env};
use super::provision::{run, run_attached};
use super::{DockerError, Folder, Host};
use crate::config::DOCKER_BACKUPS_MOUNT;
use std::path::{Path, PathBuf};

/// How kkt in Docker stands.
#[derive(Debug, PartialEq, Eq)]
pub struct State {
    /// Whether the managed folder has its settings file.
    pub files_present: bool,
    /// Whether the kkt container is running.
    pub running: bool,
    /// The kkt version the settings file names.
    pub version: Option<String>,
    /// Whether it is set up to be served over Tailscale.
    pub tailscale: bool,
}

pub fn status(host: &dyn Host, folder: &Folder) -> Result<State, DockerError> {
    let env_path = folder.path().join(ENV_FILE);
    if !host.exists(&env_path) {
        return Ok(State {
            files_present: false,
            running: false,
            version: None,
            tailscale: false,
        });
    }
    let env = host
        .read_file(&env_path)
        .map_err(|e| DockerError::io(format!("could not read {}", env_path.display()), e))?;
    let ps = compose(folder, &["ps", "--status", "running", "--services"]);
    let done = run(host, &ps)?;
    if !done.success() {
        // Not knowing is not the same as stopped.
        return Err(DockerError::Command {
            command: ps.to_string(),
            detail: done.detail(),
        });
    }
    Ok(State {
        files_present: true,
        running: done.stdout.lines().any(|l| l.trim() == "kkt"),
        version: version_in_env(&env),
        tailscale: env
            .lines()
            .any(|l| l.trim() == "COMPOSE_FILE=compose.tailscale.yaml"),
    })
}

/// Starts the containers.
pub fn up(host: &dyn Host, folder: &Folder) -> Result<(), DockerError> {
    run_attached(host, &compose(folder, &["up", "-d"]))
}

/// Stops and removes the containers. The data stays, in its volumes, unless
/// `remove_data`.
pub fn down(host: &dyn Host, folder: &Folder, remove_data: bool) -> Result<(), DockerError> {
    let args: &[&str] = if remove_data {
        &["down", "--volumes"]
    } else {
        &["down"]
    };
    run_attached(host, &compose(folder, args))
}

/// Backs kkt up with `kkt backup` inside the container, then copies the new
/// backup folder into `dest` on the host, owned by the person running this.
/// Returns where the backup is. The folder in the managed backups folder
/// stays too.
pub fn backup(host: &dyn Host, folder: &Folder, dest: &Path) -> Result<PathBuf, DockerError> {
    let name = backup_in_container(host, folder)?;
    host.create_dir_all(dest)
        .map_err(|e| DockerError::io(format!("could not create {}", dest.display()), e))?;
    let from = format!("kkt:{DOCKER_BACKUPS_MOUNT}/{name}");
    let to = dest.display().to_string();
    let command = compose(folder, &["cp", &from, &to]);
    let done = run(host, &command)?;
    if !done.success() {
        return Err(DockerError::Command {
            command: command.to_string(),
            detail: done.detail(),
        });
    }
    Ok(dest.join(name))
}

/// Moves kkt to `new_version`: a backup first (so a failed backup stops the
/// upgrade), then this program's Compose files and the new version in the
/// settings file, then the new image.
pub fn upgrade(host: &dyn Host, folder: &Folder, new_version: &str) -> Result<(), DockerError> {
    let env_path = folder.path().join(ENV_FILE);
    let env = host
        .read_file(&env_path)
        .map_err(|e| DockerError::io(format!("could not read {}", env_path.display()), e))?;
    backup_in_container(host, folder)?;
    let tailscale = env
        .lines()
        .any(|l| l.trim() == "COMPOSE_FILE=compose.tailscale.yaml");
    write_embedded(host, folder, tailscale)?;
    write_env(host, folder, &set_version(&env, new_version))?;
    run_attached(host, &compose(folder, &["pull"]))?;
    run_attached(host, &compose(folder, &["up", "-d"]))
}

/// Runs `kkt backup` in the container and returns the new folder's name.
fn backup_in_container(host: &dyn Host, folder: &Folder) -> Result<String, DockerError> {
    let command = compose(
        folder,
        &["exec", "-T", "kkt", "kkt", "backup", DOCKER_BACKUPS_MOUNT],
    );
    let done = run(host, &command)?;
    if !done.success() {
        return Err(DockerError::Command {
            command: command.to_string(),
            detail: done.detail(),
        });
    }
    let line = done
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("Backed up to "))
        .ok_or_else(|| DockerError::Unexpected {
            what: "`kkt backup`",
            detail: done.stdout.trim().to_string(),
        })?;
    Path::new(line.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| DockerError::Unexpected {
            what: "`kkt backup`",
            detail: line.to_string(),
        })
}
