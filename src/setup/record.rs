//! The setup record: TOML, written by `kkt setup` once the machine is set up.
//!
//! ```toml
//! format = 1
//! mode = "docker"
//! written_by = "0.4.0"
//! url = "https://kkt.tail1234.ts.net/mcp"
//! key_file = "/home/sam/.config/kkt/docker-access-key"
//!
//! [docker]
//! folder = "/home/sam/.local/share/kashkeshet/docker"
//! tailscale = true
//! hostname = "kkt"
//! tailnet = "tail1234.ts.net"
//! ```
//!
//! `mode` is `service`, `docker` or `client`. Paths are absolute. A
//! `[service]` table holds `tailscale` for the service mode; the client mode
//! has no table.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The only format this version reads and writes.
const FORMAT: u32 = 1;

/// What a machine was set up as, and how clients reach its server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub mode: Mode,
    /// The kkt version that wrote the record.
    pub written_by: String,
    /// The MCP endpoint clients on this machine use.
    pub url: String,
    /// The access key clients on this machine send, if the server has one.
    pub key_file: Option<PathBuf>,
}

/// What the machine is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The server runs here as a launchd or systemd service.
    Service {
        /// Shared over the host's Tailscale (`tailscale serve`).
        tailscale: bool,
    },
    /// The server runs here in Docker, from Compose files kkt manages.
    Docker(DockerSetup),
    /// The server is elsewhere; only clients are set up here.
    Client,
}

/// The Docker install kkt manages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerSetup {
    /// The folder holding the Compose files, `.env` and `backups`.
    pub folder: PathBuf,
    /// Shared over a Tailscale container.
    pub tailscale: bool,
    /// The node's name on the tailnet, once known, such as `kkt` or `kkt-1`.
    pub hostname: Option<String>,
    /// The tailnet's DNS name, once known, such as `tail1234.ts.net`.
    pub tailnet: Option<String>,
}

impl Mode {
    /// The name used for the mode in the record and in messages.
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Service { .. } => "service",
            Mode::Docker(_) => "docker",
            Mode::Client => "client",
        }
    }
}

/// The record as it is in the file.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    format: u32,
    mode: String,
    written_by: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<ServiceTable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    docker: Option<DockerTable>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceTable {
    tailscale: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DockerTable {
    folder: PathBuf,
    tailscale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tailnet: Option<String>,
}

impl Record {
    /// The record in `text`, read from the file at `path`. Errors name the
    /// file and what is wrong with it.
    pub fn parse(text: &str, path: &Path) -> Result<Record, String> {
        let bad = |what: String| format!("the setup record {} {what}", path.display());
        let file: File = toml::from_str(text)
            .map_err(|e: toml::de::Error| bad(format!("is not valid: {}", e.message().trim())))?;
        if file.format != FORMAT {
            return Err(bad(format!(
                "has format {}, but this kkt reads format {FORMAT}; run `kkt setup` again",
                file.format
            )));
        }
        let mode = match (file.mode.as_str(), file.service, file.docker) {
            ("service", Some(service), None) => Mode::Service {
                tailscale: service.tailscale,
            },
            ("docker", None, Some(docker)) => Mode::Docker(DockerSetup {
                folder: docker.folder,
                tailscale: docker.tailscale,
                hostname: docker.hostname,
                tailnet: docker.tailnet,
            }),
            ("client", None, None) => Mode::Client,
            (mode @ ("service" | "docker" | "client"), _, _) => {
                let tables = match mode {
                    "service" => "needs a [service] table and no [docker] table",
                    "docker" => "needs a [docker] table and no [service] table",
                    _ => "has no [service] or [docker] table",
                };
                return Err(bad(format!("has mode {mode:?}, which {tables}")));
            }
            (other, _, _) => {
                return Err(bad(format!(
                    "has an unknown mode {other:?}: expected service, docker or client"
                )));
            }
        };
        let mut paths: Vec<&Path> = file.key_file.iter().map(PathBuf::as_path).collect();
        if let Mode::Docker(docker) = &mode {
            paths.push(&docker.folder);
        }
        if let Some(relative) = paths.iter().find(|p| !p.is_absolute()) {
            return Err(bad(format!("has a relative path {}", relative.display())));
        }
        Ok(Record {
            mode,
            written_by: file.written_by,
            url: file.url,
            key_file: file.key_file,
        })
    }

    /// The record as file text, which [`Record::parse`] reads back.
    pub fn render(&self) -> Result<String, String> {
        let (service, docker) = match &self.mode {
            Mode::Service { tailscale } => (
                Some(ServiceTable {
                    tailscale: *tailscale,
                }),
                None,
            ),
            Mode::Docker(d) => (
                None,
                Some(DockerTable {
                    folder: d.folder.clone(),
                    tailscale: d.tailscale,
                    hostname: d.hostname.clone(),
                    tailnet: d.tailnet.clone(),
                }),
            ),
            Mode::Client => (None, None),
        };
        let file = File {
            format: FORMAT,
            mode: self.mode.name().to_string(),
            written_by: self.written_by.clone(),
            url: self.url.clone(),
            key_file: self.key_file.clone(),
            service,
            docker,
        };
        toml::to_string(&file).map_err(|e| format!("could not write the setup record: {e}"))
    }
}
