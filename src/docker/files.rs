//! The Compose files that come with this program, and the settings file
//! rendered for them. The Compose files are the repository's own, so the
//! manual route in `docs/docker.md` and setup use the same files.

use super::{DockerError, Folder, Host, Inputs};
use crate::config::DOCKER_PENDING_TAILNET;
use std::path::Path;

const COMPOSE: &str = include_str!("../../compose.yaml");
const COMPOSE_TAILSCALE: &str = include_str!("../../compose.tailscale.yaml");
const SERVE: &str = include_str!("../../tailscale/serve.json");

/// The file Compose reads its settings from, in the managed folder.
pub const ENV_FILE: &str = ".env";

/// The files to write beside `.env`: where, relative to the folder, and what.
pub(super) fn embedded(tailscale: bool) -> Vec<(&'static str, &'static str)> {
    if tailscale {
        vec![
            ("compose.tailscale.yaml", COMPOSE_TAILSCALE),
            ("tailscale/serve.json", SERVE),
        ]
    } else {
        vec![("compose.yaml", COMPOSE)]
    }
}

/// The settings file. `node` is the node's name on the tailnet, `(hostname,
/// tailnet)`, once Tailscale has logged in; until then a placeholder stands
/// in for the tailnet so Compose can start the Tailscale container alone.
pub fn env_text(inputs: &Inputs, node: Option<(&str, &str)>) -> String {
    let mut text = String::from(
        "# Written by `kkt setup`. Setup writes it again when you run it or upgrade.\n",
    );
    text.push_str(&format!("KKT_VERSION={}\n", inputs.version));
    text.push_str(&format!("COMPOSE_PROJECT_NAME={}\n", inputs.project));
    text.push_str(&format!("KKT_PORT={}\n", inputs.port));
    if !inputs.allow_hosts.is_empty() {
        text.push_str(&format!(
            "KKT_ALLOW_HOSTS={}\n",
            inputs.allow_hosts.join(",")
        ));
    }
    if let Some(tailscale) = &inputs.tailscale {
        let (hostname, tailnet) = node.unwrap_or((&tailscale.hostname, DOCKER_PENDING_TAILNET));
        text.push_str("COMPOSE_FILE=compose.tailscale.yaml\n");
        text.push_str(&format!("TS_HOSTNAME={hostname}\n"));
        text.push_str(&format!("TS_TAILNET={tailnet}\n"));
        if let Some(key) = &tailscale.auth_key {
            text.push_str(&format!("TS_AUTHKEY={key}\n"));
        }
    }
    text
}

/// The kkt version `env` runs, from its `KKT_VERSION` line.
pub fn version_in_env(env: &str) -> Option<String> {
    env.lines()
        .find_map(|line| line.strip_prefix("KKT_VERSION="))
        .map(|v| v.trim().to_string())
}

/// `env` with its `KKT_VERSION` line set to `version`, everything else kept.
pub(super) fn set_version(env: &str, version: &str) -> String {
    let mut out: Vec<String> = env
        .lines()
        .map(|line| {
            if line.starts_with("KKT_VERSION=") {
                format!("KKT_VERSION={version}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !out.iter().any(|l| l.starts_with("KKT_VERSION=")) {
        out.push(format!("KKT_VERSION={version}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Writes the Compose files for this program's version into `folder`.
pub(super) fn write_embedded(
    host: &dyn Host,
    folder: &Folder,
    tailscale: bool,
) -> Result<(), DockerError> {
    for (name, contents) in embedded(tailscale) {
        let path = folder.path().join(name);
        if let Some(parent) = path.parent() {
            create_dir(host, parent)?;
        }
        host.write_file(&path, contents, false)
            .map_err(|e| DockerError::io(format!("could not write {}", path.display()), e))?;
    }
    Ok(())
}

pub(super) fn write_env(host: &dyn Host, folder: &Folder, text: &str) -> Result<(), DockerError> {
    // The settings file can hold a Tailscale auth key.
    let path = folder.path().join(ENV_FILE);
    host.write_file(&path, text, true)
        .map_err(|e| DockerError::io(format!("could not write {}", path.display()), e))
}

pub(super) fn create_dir(host: &dyn Host, path: &Path) -> Result<(), DockerError> {
    host.create_dir_all(path)
        .map_err(|e| DockerError::io(format!("could not create {}", path.display()), e))
}
