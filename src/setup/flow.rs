//! The `kkt setup` conversation: which questions, in what order, and what
//! follows from each answer. Everything that changes the machine goes
//! through [`Steps`], so the flow is tested against a fake.

use super::ask::{Ask, Question};
use super::record::{DockerSetup, Mode, Record};
use crate::clients::Client;
use crate::config::{DEFAULT_SETUP_CONTAINERS, DEFAULT_SETUP_MODE_INDEX};
use std::path::{Path, PathBuf};

/// What this machine is for, as chosen with `--mode` or asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Service,
    Docker,
    Client,
}

impl Purpose {
    pub const ALL: [Purpose; 3] = [Purpose::Docker, Purpose::Service, Purpose::Client];

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

    pub(super) fn describe(self) -> &'static str {
        match self {
            Purpose::Service => "Run the kkt server here, as a service that starts when you log in",
            Purpose::Docker => "Run the kkt server here, in Docker (Recommended)",
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
    /// For a server: host names it answers to besides its own, such as
    /// `host.docker.internal`. Empty means none.
    pub allow_hosts: Option<Vec<String>>,
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
    /// Installs and starts the server as a service that also answers to
    /// `allow_hosts`.
    fn install_service(&self, tailscale: bool, allow_hosts: &[String])
    -> Result<Installed, String>;
    /// Sets up and starts the server in Docker, also answering to
    /// `allow_hosts`.
    fn install_docker(
        &self,
        tailscale: bool,
        allow_hosts: &[String],
    ) -> Result<(Installed, DockerSetup), String>;
    /// Saves an access key typed in for a client-only machine where nothing
    /// uses it yet, readable only by its owner, and returns where.
    fn stage_key(&self, key: &str) -> Result<PathBuf, String>;
    /// Moves a staged key to where clients of the server at `url` keep it,
    /// and returns that path. Keys for other servers are left alone.
    fn keep_key(&self, staged: &Path, url: &str) -> Result<PathBuf, String>;
    /// Removes a staged key that won't be kept.
    fn discard_key(&self, staged: &Path) -> Result<(), String>;
    /// The clients installed on this machine.
    fn detect_clients(&self) -> Vec<Client>;
    /// Writes the setup record, replacing any earlier one whole.
    fn write_record(&self, record: &Record) -> Result<(), String>;
    /// Points `client` at the server in `record`. Returns what it did.
    fn connect(&self, client: Client, record: &Record) -> Result<String, String>;
    /// Checks the connection to the server in `record`. Returns the report,
    /// or the report as an error when it found problems.
    fn doctor(&self, record: &Record) -> Result<String, String>;
    /// Checks, changing nothing, that a server answers at `url` and accepts
    /// its host name. Returns the problems found as an error.
    fn check_address(&self, url: &str) -> Result<(), String>;
    /// Checks, changing nothing, that the server at `url` accepts `key`.
    /// Returns the problems found as an error.
    fn check_key(&self, url: &str, key: &ClientKey) -> Result<(), String>;
}

const MODE: Question = Question {
    text: "What is this machine for?",
    flag: "--mode",
    default: None,
};
const TAILSCALE: Question = Question {
    text: "Share the server over Tailscale, so your other machines can reach it?",
    flag: "--tailscale or --no-tailscale",
    default: None,
};
const CONTAINERS: Question = Question {
    text: "Will agents in Docker containers on this machine use this server? They reach it as \
           host.docker.internal.",
    flag: "--allow-host host.docker.internal, or --allow-host none",
    default: Some(DEFAULT_SETUP_CONTAINERS),
};
/// The name Docker gives containers for the machine they run on.
const DOCKER_HOST: &str = "host.docker.internal";
const URL: Question = Question {
    text: "The server's MCP address, such as https://kkt.tail1234.ts.net/mcp:",
    flag: "--url",
    default: None,
};
const KEY: Question = Question {
    text: "The server's access key (run `kkt key show` there), or nothing if it has none:",
    flag: "--key-file",
    default: None,
};

/// Runs `kkt setup`: asks what `answers` leaves open, sets the machine up,
/// checks the connection, connects clients, and records it. Returns the
/// record written.
///
/// Every question comes first, so a missing answer changes nothing. The
/// record is written last, and only when everything worked: a setup that
/// fails at any step leaves an earlier record, and the key it names, as
/// they were.
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
            Purpose::ALL[ask.choose(MODE, &options, DEFAULT_SETUP_MODE_INDEX)?]
        }
    };
    let plan = match purpose {
        Purpose::Client => Plan::Client(client_answers(answers, ask, steps)?),
        server => {
            let tailscale = match answers.tailscale {
                Some(answer) => answer,
                None => ask.confirm(TAILSCALE)?,
            };
            let allow_hosts = match &answers.allow_hosts {
                Some(hosts) => hosts.clone(),
                None if ask.confirm(CONTAINERS)? => vec![DOCKER_HOST.to_string()],
                None => Vec::new(),
            };
            Plan::Server(server, tailscale, allow_hosts)
        }
    };
    let chosen = choose_clients(answers, ask, steps)?;

    let (mode, installed, staged) = match plan {
        Plan::Server(Purpose::Docker, tailscale, allow_hosts) => {
            let (installed, docker) = steps.install_docker(tailscale, &allow_hosts)?;
            (Mode::Docker(docker), installed, None)
        }
        Plan::Server(_, tailscale, allow_hosts) => (
            Mode::Service { tailscale },
            steps.install_service(tailscale, &allow_hosts)?,
            None,
        ),
        Plan::Client(ClientAnswers { url, key }) => {
            let (key_file, staged) = match key {
                ClientKey::None => (None, None),
                ClientKey::File(path) => (Some(path), None),
                ClientKey::Typed(key) => {
                    let staged = steps.stage_key(&key)?;
                    (Some(staged.clone()), Some(staged))
                }
            };
            (Mode::Client, Installed { url, key_file }, staged)
        }
    };
    let mut record = Record {
        mode,
        written_by: version.to_string(),
        url: installed.url,
        key_file: installed.key_file,
    };

    match steps.doctor(&record) {
        Ok(report) => ask.tell(&report),
        Err(report) => {
            ask.tell(&report);
            if let Some(staged) = &staged
                && let Err(e) = steps.discard_key(staged)
            {
                ask.tell(&format!("Could not remove the unused key: {e}"));
            }
            return Err(
                "the connection check found problems (see the fixes above), so nothing was \
                 recorded; fix them and run `kkt setup` again"
                    .to_string(),
            );
        }
    }
    if let Some(staged) = &staged {
        record.key_file = Some(steps.keep_key(staged, &record.url)?);
    }

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
    if !failed.is_empty() {
        return Err(format!(
            "could not connect {}, so nothing was recorded; fix that and run `kkt setup` again",
            failed.join(", ")
        ));
    }

    steps.write_record(&record)?;
    let web = record.url.strip_suffix("/mcp").unwrap_or(&record.url);
    ask.tell(&format!(
        "This machine is set up as {}; clients use {}, and the web page is at {web}/.",
        record.mode.name(),
        record.url
    ));
    Ok(record)
}

/// What setup will do, once every question is answered.
enum Plan {
    /// Run the server here: as a service or in Docker, whether over
    /// Tailscale, and the other host names it answers to.
    Server(Purpose, bool, Vec<String>),
    Client(ClientAnswers),
}

/// Where a client-only machine's server is, and its key.
struct ClientAnswers {
    url: String,
    key: ClientKey,
}

/// The key a client-only machine uses for its server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientKey {
    /// The server has no key.
    None,
    /// A file given with `--key-file`, used where it is.
    File(PathBuf),
    /// A key typed in, saved by setup.
    Typed(String),
}

/// Asks for the server's address and key, checking each as soon as it is
/// given and asking again on a problem. An answer given as a flag is
/// checked the same way, and a problem with it ends setup.
fn client_answers(
    answers: &Answers,
    ask: &dyn Ask,
    steps: &dyn Steps,
) -> Result<ClientAnswers, String> {
    loop {
        let url = checked_address(answers, ask, steps)?;
        if let Some(key) = checked_key(&url, answers, ask, steps)? {
            return Ok(ClientAnswers { url, key });
        }
    }
}

/// The server's address, once it passes its check.
fn checked_address(answers: &Answers, ask: &dyn Ask, steps: &dyn Steps) -> Result<String, String> {
    loop {
        let url = match &answers.url {
            Some(url) => url.trim().to_string(),
            None => ask.text(URL)?.trim().to_string(),
        };
        match check_address(&url, steps) {
            Ok(()) => return Ok(url),
            Err(problem) if answers.url.is_some() => return Err(flag_failed(URL, &problem)),
            Err(problem) => ask.tell(&format!("That address did not work:\n{problem}")),
        }
    }
}

/// The key, once the server at `url` accepts it. When a key fails, the
/// address is checked again, so a problem with the address is never taken
/// for one with the key: `None` means it no longer passes and must be asked
/// for again.
fn checked_key(
    url: &str,
    answers: &Answers,
    ask: &dyn Ask,
    steps: &dyn Steps,
) -> Result<Option<ClientKey>, String> {
    loop {
        let key = match &answers.key_file {
            Some(path) => ClientKey::File(path.clone()),
            None => match typed_key(&ask.text(KEY)?) {
                Ok(key) => key,
                Err(problem) => {
                    ask.tell(&problem);
                    continue;
                }
            },
        };
        let Err(problem) = steps.check_key(url, &key) else {
            return Ok(Some(key));
        };
        match steps.check_address(url) {
            Err(problem) if answers.url.is_some() => return Err(flag_failed(URL, &problem)),
            Err(problem) => {
                ask.tell(&format!("That address no longer works:\n{problem}"));
                return Ok(None);
            }
            Ok(()) if answers.key_file.is_some() => return Err(flag_failed(KEY, &problem)),
            Ok(()) => ask.tell(&format!("The server did not accept that key:\n{problem}")),
        }
    }
}

fn check_address(url: &str, steps: &dyn Steps) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "{url:?} is not a server address: it starts with http:// or https://"
        ));
    }
    steps.check_address(url)
}

/// The key typed in answer to [`KEY`]: none when blank.
fn typed_key(text: &str) -> Result<ClientKey, String> {
    match text.trim() {
        "" => Ok(ClientKey::None),
        key => {
            crate::config::check_access_key(key)
                .map_err(|e| format!("that key is not valid: {e}"))?;
            Ok(ClientKey::Typed(key.to_string()))
        }
    }
}

/// The error for an answer given as a flag that failed its check.
fn flag_failed(question: Question, problem: &str) -> String {
    format!(
        "the answer given with {} did not pass its check, so nothing was changed:\n{problem}",
        question.flag
    )
}

/// The clients to connect: those given with `--clients`, or each one found
/// that the person agrees to (all of them with `--yes`).
fn choose_clients(
    answers: &Answers,
    ask: &dyn Ask,
    steps: &dyn Steps,
) -> Result<Vec<Client>, String> {
    if let Some(chosen) = &answers.clients {
        return Ok(chosen.clone());
    }
    let mut chosen = Vec::new();
    for client in steps.detect_clients() {
        let question = Question {
            text: &format!("Connect {} to kkt?", client.display_name()),
            flag: "--clients",
            default: Some(true),
        };
        if answers.yes || ask.confirm(question)? {
            chosen.push(client);
        }
    }
    Ok(chosen)
}
