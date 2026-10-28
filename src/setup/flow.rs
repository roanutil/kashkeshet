//! The `kkt setup` conversation: which questions, in what order, and what
//! follows from each answer. Everything that changes the machine goes
//! through [`Steps`], so the flow is tested against a fake.

use super::ask::{Ask, Question};
use super::record::{DockerSetup, Mode, Record};
use crate::clients::Client;
use std::path::PathBuf;

/// What this machine is for, as chosen with `--mode` or asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Service,
    Docker,
    Client,
}

impl Purpose {
    pub const ALL: [Purpose; 3] = [Purpose::Service, Purpose::Docker, Purpose::Client];

    /// The `--mode` value.
    pub fn name(self) -> &'static str {
        match self {
            Purpose::Service => "service",
            Purpose::Docker => "docker",
            Purpose::Client => "client",
        }
    }

    pub fn from_name(name: &str) -> Option<Purpose> {
        Purpose::ALL.into_iter().find(|p| p.name() == name)
    }

    fn describe(self) -> &'static str {
        match self {
            Purpose::Service => "Run the kkt server here, as a service that starts when you log in",
            Purpose::Docker => "Run the kkt server here, in Docker",
            Purpose::Client => "Connect this machine's clients to a kkt server elsewhere",
        }
    }
}

/// The answers given as flags. `None` means ask.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    pub purpose: Option<Purpose>,
    /// Share the server over Tailscale.
    pub tailscale: Option<bool>,
    /// For a client: the server's MCP endpoint.
    pub url: Option<String>,
    /// For a client: a file holding the server's access key.
    pub key_file: Option<PathBuf>,
    /// The clients to connect. Empty means none.
    pub clients: Option<Vec<Client>>,
    /// Answer yes to every yes-or-no question that has no flag of its own,
    /// such as connecting each client found.
    pub yes: bool,
}

/// A server set up on this machine: how its clients reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub url: String,
    pub key_file: Option<PathBuf>,
}

/// What `kkt setup` does to the machine. The real implementation is in
/// `main.rs`.
pub trait Steps {
    /// Installs and starts the server as a service.
    fn install_service(&self, tailscale: bool) -> Result<Installed, String>;
    /// Sets up and starts the server in Docker.
    fn install_docker(&self, tailscale: bool) -> Result<(Installed, DockerSetup), String>;
    /// Saves an access key typed in for a client-only machine, readable only
    /// by its owner, and returns where.
    fn save_key(&self, key: &str) -> Result<PathBuf, String>;
    /// The clients installed on this machine.
    fn detect_clients(&self) -> Vec<Client>;
    /// Writes the setup record.
    fn write_record(&self, record: &Record) -> Result<(), String>;
    /// Points `client` at the server in `record`. Returns what it did.
    fn connect(&self, client: Client, record: &Record) -> Result<String, String>;
    /// Checks the connection to the server in `record`. Returns the report,
    /// or the report as an error when it found problems.
    fn doctor(&self, record: &Record) -> Result<String, String>;
}

const MODE: Question = Question {
    text: "What is this machine for?",
    flag: "--mode",
};
const TAILSCALE: Question = Question {
    text: "Share the server over Tailscale, so your other machines can reach it?",
    flag: "--tailscale or --no-tailscale",
};
const URL: Question = Question {
    text: "The server's MCP address, such as https://kkt.tail1234.ts.net/mcp:",
    flag: "--url",
};
const KEY: Question = Question {
    text: "The server's access key (run `kkt key show` there), or nothing if it has none:",
    flag: "--key-file",
};

/// Runs `kkt setup`: asks what `answers` leaves open, sets the machine up,
/// records it, connects clients and checks the connection. Returns the
/// record written.
///
/// The record is written only once the server (or, for a client, the key)
/// is in place, so a setup that fails part way leaves any earlier record.
/// Clients are connected after it, so one that fails doesn't undo the rest.
pub fn run(
    answers: &Answers,
    version: &str,
    ask: &dyn Ask,
    steps: &dyn Steps,
) -> Result<Record, String> {
    let purpose = match answers.purpose {
        Some(purpose) => purpose,
        None => {
            let options = Purpose::ALL.map(Purpose::describe);
            Purpose::ALL[ask.choose(MODE, &options)?]
        }
    };
    let tailscale = |ask: &dyn Ask| match answers.tailscale {
        Some(answer) => Ok(answer),
        None => ask.confirm(TAILSCALE),
    };
    let (mode, installed) = match purpose {
        Purpose::Service => {
            let tailscale = tailscale(ask)?;
            (
                Mode::Service { tailscale },
                steps.install_service(tailscale)?,
            )
        }
        Purpose::Docker => {
            let (installed, docker) = steps.install_docker(tailscale(ask)?)?;
            (Mode::Docker(docker), installed)
        }
        Purpose::Client => (Mode::Client, client(answers, ask, steps)?),
    };
    let record = Record {
        mode,
        written_by: version.to_string(),
        url: installed.url,
        key_file: installed.key_file,
    };
    steps.write_record(&record)?;
    ask.tell(&format!(
        "This machine is set up as {}; clients use {}.",
        record.mode.name(),
        record.url
    ));

    let chosen = match &answers.clients {
        Some(chosen) => chosen.clone(),
        None => {
            let mut chosen = Vec::new();
            for client in steps.detect_clients() {
                let question = Question {
                    text: &format!("Connect {} to kkt?", client.display_name()),
                    flag: "--clients",
                };
                if answers.yes || ask.confirm(question)? {
                    chosen.push(client);
                }
            }
            chosen
        }
    };
    let mut failed = Vec::new();
    for client in chosen {
        match steps.connect(client, &record) {
            Ok(done) => ask.tell(&done),
            Err(e) => {
                ask.tell(&format!("Could not connect {}: {e}", client.display_name()));
                failed.push(client.display_name());
            }
        }
    }
    let report = steps.doctor(&record);
    ask.tell(match &report {
        Ok(text) | Err(text) => text,
    });
    match (report, failed.is_empty()) {
        (Ok(_), true) => Ok(record),
        (Ok(_), false) => Err(format!(
            "set up, but these clients were not connected: {}; \
             try `kkt connect` for each",
            failed.join(", ")
        )),
        (Err(_), _) => {
            Err("set up, but the connection check found problems; see the fixes above".to_string())
        }
    }
}

/// A client-only machine: where its server is and the key it needs.
fn client(answers: &Answers, ask: &dyn Ask, steps: &dyn Steps) -> Result<Installed, String> {
    let url = match &answers.url {
        Some(url) => url.clone(),
        None => ask.text(URL)?,
    };
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "{url:?} is not a server address: it starts with http:// or https://"
        ));
    }
    let key_file = match &answers.key_file {
        Some(path) => Some(path.clone()),
        None => {
            let key = ask.text(KEY)?;
            let key = key.trim();
            if key.is_empty() {
                None
            } else {
                Some(steps.save_key(key)?)
            }
        }
    };
    Ok(Installed { url, key_file })
}
