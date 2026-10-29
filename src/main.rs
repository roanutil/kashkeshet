mod browser_login;

use clap::{Args, Parser, Subcommand};
use kashkeshet::admission::{Admission, SharedKey};
use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::backup;
use kashkeshet::clients::{self, Client, Setup};
use kashkeshet::clock::{Clock, SystemClock};
use kashkeshet::config::{
    self, DEFAULT_BIND, DEFAULT_DEPENDENCY_LOG_LEVEL, DEFAULT_LOG_LEVEL, InvalidSetting,
    LOG_OWN_TARGETS, Limits, LogLevel, LogPolicy,
};
use kashkeshet::diagnostics;
use kashkeshet::docker;
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::logs::RotatingFile;
use kashkeshet::names::OsEntropy;
use kashkeshet::provenance::{self, Build, ServerRun};
use kashkeshet::service::{DryRun, Manager, OsSystem, Spec};
use kashkeshet::settings::{self, AccessKey, Env, Platform, ServeOptions, ServeSettings};
use kashkeshet::setup;
use kashkeshet::shutdown::{Shutdown, StopSignals};
use kashkeshet::store::{self, Db};
use kashkeshet::tailscale;
use kashkeshet::{Operator, Revoked};
use sha2::{Digest, Sha256};
use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Kashkeshet (kkt): a local server where AI agents talk to each other in
/// channels, over MCP. Start it with `kkt serve`.
#[derive(Parser, Debug)]
#[command(name = "kashkeshet", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// Parsed once at startup, so the size of the largest variant costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum Command {
    /// Show the service, Docker server, or remote server this machine uses.
    Status,
    /// Upgrade the configured server to this installed kkt version, backing
    /// it up first. Install the new kkt binary before running this command.
    Upgrade {
        /// Allow replacing a newer server or an image tag whose version
        /// cannot be compared. Same-version upgrades still do nothing.
        #[arg(long)]
        allow_downgrade: bool,
    },
    /// Stop and remove this machine's setup. Data, keys, backups and client
    /// configurations stay; remove the kkt binary separately if wanted.
    Uninstall,
    /// Run the server, with settings from a config file as well as flags.
    ///
    /// Flags win over the file, and the file over the defaults. Without
    /// --db, the database is kashkeshet.db in the per-user data directory.
    /// Without --access-key-file, the key is ~/.config/kkt/access-key,
    /// created on first start; --no-access-key runs with none.
    Serve {
        /// The config file. By default ~/.config/kkt/config.toml
        /// ($XDG_CONFIG_HOME/kkt/config.toml), if it exists.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Start even though `kkt setup` set this machine up otherwise: as a
        /// Docker server, a client, or with the service running. The service
        /// passes it, since it is the server setup meant.
        #[arg(long)]
        ignore_setup: bool,
        #[command(flatten)]
        flags: ServeFlags,
    },
    /// Manage the access key.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    /// List agents, or revoke one's token. Works on the database file while
    /// the server runs.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
        /// The server's database file. By default the one `kkt serve` uses:
        /// the config file's `db`, else kashkeshet.db in the per-user data
        /// directory.
        #[arg(long, global = true)]
        db: Option<PathBuf>,
        /// The config file, as for `kkt serve`.
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
    /// Back up the server's database, config file and access key into a new
    /// folder inside FOLDER. Safe while the server runs.
    ///
    /// The new folder is named for the time and this kkt's version, such as
    /// kkt-20261028T131500.123Z-v0.4.0, so backups sort by time. It and the
    /// key in it are readable only by you.
    Backup {
        /// Where to copy the backup. Created if missing. In Docker mode,
        /// omit it to keep only the copy in the managed backups folder.
        /// Required for a local service or an unrecorded local database.
        folder: Option<PathBuf>,
        /// The server's database file. By default the one `kkt serve` uses:
        /// the config file's `db`, else kashkeshet.db in the per-user data
        /// directory.
        #[arg(long)]
        db: Option<PathBuf>,
        /// The config file, as for `kkt serve`.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Run `kkt serve` in the background: launchd on macOS, systemd on Linux.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Set this machine up: run the server here as a service or in Docker,
    /// or connect clients to a server elsewhere. Asks what the flags leave
    /// open; with no terminal, a missing answer is an error naming its flag.
    Setup {
        #[command(flatten)]
        answers: SetupFlags,
    },
    /// Show a client's MCP configuration, or apply it with --write.
    Connect {
        #[arg(value_parser = ["claude-code", "codex", "agy", "opencode"])]
        client: String,
        #[command(flatten)]
        connection: ConnectionFlags,
        /// Back up the client's config, then apply the change.
        #[arg(long)]
        write: bool,
    },
    /// Diagnose the key file, HTTP connection and MCP authentication.
    Doctor {
        #[command(flatten)]
        connection: ConnectionFlags,
    },
}

#[derive(Args, Debug)]
struct SetupFlags {
    /// What this machine is for.
    #[arg(long, value_parser = ["service", "docker", "client"])]
    mode: Option<String>,
    /// Share the server over Tailscale.
    #[arg(long, conflicts_with = "no_tailscale")]
    tailscale: bool,
    /// Don't share the server over Tailscale.
    #[arg(long)]
    no_tailscale: bool,
    /// For a client: the server's MCP address, such as
    /// https://kkt.tail1234.ts.net/mcp.
    #[arg(long)]
    url: Option<String>,
    /// For a client: a file holding the server's access key.
    #[arg(long)]
    key_file: Option<PathBuf>,
    /// The clients to connect, separated by commas (claude-code, codex,
    /// opencode, agy), or `none`. Without it, setup offers each one it finds.
    #[arg(long)]
    clients: Option<String>,
    /// Connect every client found without asking.
    #[arg(long)]
    yes: bool,
    /// For Docker over Tailscale: the machine name to ask for on the
    /// tailnet. (The service uses this machine's own Tailscale name.)
    #[arg(long, default_value = config::DEFAULT_TS_HOSTNAME)]
    ts_hostname: String,
    /// For Docker over Tailscale: a file holding a Tailscale auth key, to log
    /// in with instead of a link.
    #[arg(long)]
    ts_authkey_file: Option<PathBuf>,
    /// A host name the server should answer to besides its own, such as
    /// host.docker.internal for agents in containers on this machine. Repeat
    /// for more; `none` for none. For a service it goes in the config file's
    /// allow_hosts; for Docker, in KKT_ALLOW_HOSTS in the managed .env.
    #[arg(long = "allow-host")]
    allow_hosts: Vec<String>,
    /// For testing: the kkt version whose image Docker runs.
    #[arg(long, hide = true, default_value = Build::CURRENT.version)]
    kkt_version: String,
    /// For testing beside a real setup: the Compose project name.
    #[arg(long, hide = true, default_value = config::DOCKER_PROJECT)]
    docker_project: String,
    /// For testing beside a real setup: the port on 127.0.0.1 Docker
    /// publishes kkt on.
    #[arg(long, hide = true, default_value_t = DEFAULT_BIND.port())]
    docker_port: u16,
}

impl SetupFlags {
    /// The settings for a Docker or Tailscale install, which the flow passes
    /// through without asking about.
    fn install_options(&self) -> InstallOptions {
        InstallOptions {
            ts_hostname: self.ts_hostname.clone(),
            ts_authkey_file: self.ts_authkey_file.clone(),
            kkt_version: self.kkt_version.clone(),
            docker_project: self.docker_project.clone(),
            docker_port: self.docker_port,
        }
    }

    fn answers(self) -> Result<setup::Answers, String> {
        let clients = match self.clients.as_deref() {
            None => None,
            Some("none") => Some(Vec::new()),
            Some(list) => Some(
                list.split(',')
                    .map(|name| {
                        Client::from_name(name.trim()).ok_or_else(|| {
                            format!(
                                "unknown client {name:?} in --clients: use claude-code, codex, \
                                 opencode, agy or none"
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            ),
        };
        let key_file = match self.key_file {
            Some(path) => Some(
                std::path::absolute(&path)
                    .map_err(|e| format!("could not resolve {}: {e}", path.display()))?,
            ),
            None => None,
        };
        Ok(setup::Answers {
            purpose: self.mode.as_deref().and_then(setup::Purpose::from_name),
            tailscale: match (self.tailscale, self.no_tailscale) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            url: self.url,
            key_file,
            allow_hosts: match self.allow_hosts.as_slice() {
                [] => None,
                [none] if none == "none" => Some(Vec::new()),
                hosts => Some(hosts.to_vec()),
            },
            clients,
            yes: self.yes,
        })
    }
}

#[derive(Args, Debug)]
struct ConnectionFlags {
    /// The serve config file (otherwise the default, if present).
    #[arg(long)]
    config: Option<PathBuf>,
    /// The HTTP MCP endpoint (otherwise http://<serve bind>/mcp).
    #[arg(long)]
    url: Option<String>,
    /// The access key file (otherwise the one used by serve).
    #[arg(long)]
    key_file: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum AgentCommand {
    /// Every agent, most recently seen first, with revoked ones marked.
    List,
    /// Revoke an agent's token. Its later calls are refused as
    /// `unauthenticated`; its name and messages stay.
    Revoke {
        /// The agent's name, such as amber-falcon.
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceCommand {
    /// Install the service, start it, and have it start at each login.
    ///
    /// The service runs `kkt serve` with absolute paths: this program, the
    /// config file if there is one, and the database unless the config file
    /// names it. Other settings are read from the config file each time the
    /// service starts. Installing again replaces the service.
    Install {
        /// The config file. By default ~/.config/kkt/config.toml
        /// ($XDG_CONFIG_HOME/kkt/config.toml), if it exists.
        #[arg(long)]
        config: Option<PathBuf>,
        /// The database file, in place of the config file's or the default.
        #[arg(long)]
        db: Option<PathBuf>,
        /// The access key file, in place of the config file's.
        #[arg(long)]
        access_key_file: Option<PathBuf>,
        /// Run the service with no access key: anyone who can reach it can
        /// use it.
        #[arg(long)]
        no_access_key: bool,
        /// Show what would be written and run, and change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop the service and remove it. The database, config and logs stay.
    Uninstall,
    /// Start the installed service.
    Start,
    /// Stop the service until it is started again or you next log in.
    Stop,
    /// Show whether the service is installed and running.
    Status,
    /// Show the service's log.
    Logs {
        /// How many of the last lines to show.
        #[arg(long, default_value_t = config::DEFAULT_LOG_LINES)]
        lines: usize,
        /// Keep showing new lines as they come.
        #[arg(long, short)]
        follow: bool,
    },
}

#[derive(Subcommand, Debug)]
enum KeyCommand {
    /// Print the access-key header for client helper commands.
    Header {
        /// The key file (otherwise the default access-key path).
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// Print the JSON header map required by Claude Code and Codex.
        #[arg(long)]
        json: bool,
    },
    /// Print the access key, and nothing else. The server never logs it.
    Show {
        /// The key file (otherwise the default access-key path).
        #[arg(long)]
        key_file: Option<PathBuf>,
    },
    /// Write a new random access key to a file only you can read.
    Create {
        /// Where to write it. By default ~/.config/kkt/access-key
        /// ($XDG_CONFIG_HOME/kkt/access-key).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Replace the file if it exists.
        #[arg(long)]
        force: bool,
        /// Also print the key.
        #[arg(long)]
        show: bool,
    },
}

/// The server's settings as flags. Each is optional so that a setting not
/// given here can come from the config file or the default.
#[derive(Args, Debug)]
struct ServeFlags {
    /// The SQLite database file. Created if missing [default: kashkeshet.db
    /// in the per-user data directory]
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long, help = format!(
        "Address to listen on. Must be a loopback address [default: {DEFAULT_BIND}]"
    ))]
    bind: Option<SocketAddr>,
    /// Let --bind be a non-loopback address, such as 0.0.0.0 inside a
    /// container. Needs an access key.
    #[arg(long)]
    allow_any_bind: bool,
    #[arg(long, help = format!(
        "Largest response, in bytes, before it is cut short [default: {}]",
        Limits::DEFAULT.max_response_bytes
    ))]
    max_response_bytes: Option<usize>,
    #[arg(long, help = format!(
        "Items returned when an agent gives no `limit` [default: {}]",
        Limits::DEFAULT.default_limit
    ))]
    default_limit: Option<usize>,
    #[arg(long, help = format!(
        "Largest `limit` an agent may ask for [default: {}]",
        Limits::DEFAULT.max_limit
    ))]
    max_limit: Option<usize>,
    #[arg(long, help = format!(
        "Messages one agent may send per minute [default: {}]",
        Limits::DEFAULT.sends_per_minute
    ))]
    sends_per_minute: Option<u32>,
    #[arg(long, help = format!("Seconds completed waits remain listening [default: {}]", Limits::DEFAULT.waiting_grace.as_secs()))]
    waiting_grace_seconds: Option<u32>,
    #[arg(long, help = format!(
        "Backups the server keeps of those it makes before upgrading the database [default: {}]",
        Limits::DEFAULT.upgrade_backups_kept
    ))]
    upgrade_backups_kept: Option<usize>,
    /// A browser origin allowed to call /mcp, e.g. http://localhost:6274 for
    /// MCP Inspector. Repeat for more. By default no browser origin is allowed.
    #[arg(long = "allow-origin")]
    allow_origins: Vec<String>,
    /// A host name the server answers to besides localhost, 127.0.0.1 and
    /// ::1, e.g. host.docker.internal or a Tailscale name. Add `:port` to
    /// allow only that port. Repeat, or separate names with commas, for more.
    #[arg(long = "allow-host", value_delimiter = ',')]
    allow_hosts: Vec<String>,
    /// A file whose text is sent to every agent that connects, in place of
    /// the built-in instructions (agent-instructions.md).
    #[arg(long)]
    instructions: Option<PathBuf>,
    /// A file holding the access key. Every /mcp request must send
    /// `Authorization: Bearer <key>`, and the web page asks for it [default:
    /// ~/.config/kkt/access-key, created if missing]
    #[arg(long)]
    access_key_file: Option<PathBuf>,
    /// Run with no access key: anyone who can reach the server can use it.
    #[arg(long)]
    no_access_key: bool,
    /// Write the log to this file instead of stdout. It stays under about
    /// 60 MB: rolled over at 10 MB, five rolled files kept, none older than
    /// 30 days.
    #[arg(long)]
    log_file: Option<PathBuf>,
    /// How much kkt's own code logs: error, warn, info, debug or trace
    /// [default: info]
    #[arg(long, value_parser = |text: &str| LogLevel::parse("log_level", text))]
    log_level: Option<LogLevel>,
    /// How much the libraries kkt uses (the MCP library, the web server)
    /// log: error, warn or info. At info, the MCP library logs several lines
    /// for every request; debug and trace are refused, because there it logs
    /// request contents, tokens included [default: warn]
    #[arg(long, value_parser = |text: &str| LogLevel::parse("dependency_log_level", text))]
    dependency_log_level: Option<LogLevel>,
}

impl ServeFlags {
    /// The flags given. A list flag not given at all counts as not given, so
    /// the config file's list applies.
    fn options(self) -> ServeOptions {
        let list = |items: Vec<String>| (!items.is_empty()).then_some(items);
        ServeOptions {
            db: self.db,
            bind: self.bind,
            allow_any_bind: self.allow_any_bind.then_some(true),
            max_response_bytes: self.max_response_bytes,
            default_limit: self.default_limit,
            max_limit: self.max_limit,
            sends_per_minute: self.sends_per_minute,
            waiting_grace_seconds: self.waiting_grace_seconds,
            upgrade_backups_kept: self.upgrade_backups_kept,
            allow_origins: list(self.allow_origins),
            allow_hosts: list(self.allow_hosts),
            instructions: self.instructions,
            access_key_file: self.access_key_file,
            no_access_key: self.no_access_key.then_some(true),
            log_file: self.log_file,
            log_level: self.log_level,
            dependency_log_level: self.dependency_log_level,
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let env = process_env();
    // `serve` starts logging once its settings say where the log goes.
    let logging = match &cli.command {
        Command::Serve { .. } => Ok(()),
        _ => init_logging(None, DEFAULT_LOG_LEVEL, DEFAULT_DEPENDENCY_LOG_LEVEL),
    };
    let result = match (logging, cli.command) {
        (Err(e), _) => Err(e),
        (Ok(()), command) => run(&env, command).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // An error from before logging started still needs saying.
            if tracing::dispatcher::has_been_set()
                || init_logging(None, DEFAULT_LOG_LEVEL, DEFAULT_DEPENDENCY_LOG_LEVEL).is_ok()
            {
                tracing::error!("{e}");
            } else {
                eprintln!("error: {e}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

struct Connection {
    url: String,
    key_file: Option<PathBuf>,
}

/// Resolve the same serve settings without creating a database. Only connect
/// creates a missing default key; doctor is a read-only diagnostic.
fn client_connection(
    env: &Env,
    flags: ConnectionFlags,
    create: bool,
) -> Result<Connection, String> {
    // With nothing given, clients use the server `kkt setup` recorded.
    // Without --config, clients use the server `kkt setup` recorded; --url
    // and --key-file each replace only their own part of it.
    if flags.config.is_none()
        && let Some(record) = read_setup(env)?
    {
        let key_file = match flags.key_file {
            Some(path) => Some(
                std::path::absolute(&path)
                    .map_err(|e| format!("could not resolve key path: {e}"))?,
            ),
            None => record.key_file,
        };
        return Ok(Connection {
            url: flags.url.unwrap_or(record.url),
            key_file,
        });
    }
    serve_connection(env, flags, create)
}

/// How clients reach the server `kkt serve` would run with these settings.
fn serve_connection(env: &Env, flags: ConnectionFlags, create: bool) -> Result<Connection, String> {
    let key_override = flags.key_file.is_some();
    let loaded = load_serve_options(
        env,
        flags.config,
        ServeOptions {
            access_key_file: flags.key_file,
            no_access_key: key_override.then_some(false),
            ..ServeOptions::NONE
        },
    )?;
    let sources = loaded.sources();
    let settings = loaded
        .flags
        .over(loaded.from_file)
        .resolve(
            env.default_db(Platform::CURRENT).unwrap_or_default(),
            env.access_key_file(),
        )
        .map_err(|e| sources.invalid(e))?;
    let url = diagnostics::endpoint(settings.bind, flags.url.as_deref())?;
    if create {
        make_default_key(&settings.access_key)?;
    }
    let key_file = match settings.access_key {
        AccessKey::None => None,
        AccessKey::Default(path) | AccessKey::File(path) => {
            if create {
                read_key(&path, &sources)?;
            }
            Some(
                std::path::absolute(&path)
                    .map_err(|e| format!("could not resolve key path: {e}"))?,
            )
        }
    };
    Ok(Connection { url, key_file })
}

fn connect(env: &Env, client: &str, flags: ConnectionFlags, write: bool) -> Result<(), String> {
    let client = Client::from_name(client).ok_or("unknown client")?;
    let connection = client_connection(env, flags, true)?;
    let executable = kkt_path()?;
    let setup = Setup {
        env,
        url: &connection.url,
        executable: &executable,
        key_file: connection.key_file.as_deref(),
    };
    println!("{}", clients::configure(client, &setup, write)?);
    Ok(())
}

/// This program's absolute path, for client configs that run it.
fn kkt_path() -> Result<PathBuf, String> {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("could not find kkt's path: {e}"))
}

fn key_header(env: &Env, path: Option<PathBuf>, json: bool) -> Result<(), String> {
    let path = path
        .or_else(|| env.access_key_file())
        .ok_or("set HOME or pass --key-file")?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read key file {}: {e}", path.display()))?;
    println!("{}", clients::header(text.trim(), json)?);
    Ok(())
}

fn key_show(env: &Env, path: Option<PathBuf>) -> Result<(), String> {
    let path = path
        .or_else(|| env.access_key_file())
        .ok_or("set HOME or pass --key-file")?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read key file {}: {e}", path.display()))?;
    println!("{}", text.trim());
    Ok(())
}

/// The name of the doctor line about the service on this machine.
const SERVICE_CHECK: &str = "this machine's kkt service";

async fn doctor(env: &Env, flags: ConnectionFlags) -> Result<(), String> {
    let connection = client_connection(env, flags, false)?;
    let mut checks = diagnostics::inspect(&connection.url, connection.key_file.as_deref()).await?;
    // The last line is about this machine, whatever server `--url` points
    // at: how `kkt setup` set it up, or else its kkt service.
    match read_setup(env)?.map(|record| record.mode) {
        // The server is elsewhere; the lines above are about it.
        Some(setup::Mode::Client) => point_server_fix_at(
            &mut checks,
            "the server runs on another machine: check its URL and key (running `kkt setup` again can change them)",
        ),
        Some(setup::Mode::Docker(install)) => {
            point_server_fix_at(
                &mut checks,
                "this machine runs kkt in Docker: see the lines below, or run `kkt status`",
            );
            let folder = docker::Folder(install.folder);
            let state = docker::status(&docker::OsHost, &folder);
            checks.extend(docker_checks(state, &folder));
        }
        Some(setup::Mode::Service { .. }) | None => checks.push(service_check(env)),
    }
    let (report, all_ok) = doctor_report(checks);
    print!("{report}");
    if all_ok {
        Ok(())
    } else {
        Err("doctor found problems; see the fixes above".into())
    }
}

/// Replaces the fix on a failed server check, which tells the person to
/// start the kkt service, with `fix`, for a machine where that is not how kkt
/// runs.
fn point_server_fix_at(checks: &mut [diagnostics::Check], fix: &str) {
    for check in checks.iter_mut() {
        if check.name == diagnostics::SERVER_CHECK && !check.ok {
            check.fix = Some(fix.to_string());
        }
    }
}

/// The doctor's line about the kkt service on this machine.
fn service_check(env: &Env) -> diagnostics::Check {
    match Manager::for_this_os(env).and_then(|manager| manager.state(&OsSystem)) {
        Ok(state) if !state.installed => diagnostics::Check::ok(
            SERVICE_CHECK,
            "not installed; optional when running `kkt serve` in a terminal",
        ),
        Ok(state) if state.running => {
            diagnostics::Check::ok(SERVICE_CHECK, "installed and running")
        }
        Ok(_) => diagnostics::Check::problem(
            SERVICE_CHECK,
            "installed but not running",
            "run `kkt service start`",
        ),
        Err(error) => diagnostics::Check::problem(
            SERVICE_CHECK,
            error.to_string(),
            "check `kkt service status` and your user service manager",
        ),
    }
}

/// The name of the doctor line about kkt in Docker on this machine.
const DOCKER_CHECK: &str = "this machine's kkt in Docker";

/// The name of the doctor line about the Tailscale login of kkt in Docker.
const DOCKER_TAILSCALE_CHECK: &str = "this machine's Tailscale login";

/// The doctor's lines about kkt in Docker, from what `docker::status` found
/// in the managed `folder`: kkt itself, then, if it is served over Tailscale,
/// the Tailscale login.
fn docker_checks(
    state: Result<docker::State, docker::DockerError>,
    folder: &docker::Folder,
) -> Vec<diagnostics::Check> {
    let login = match &state {
        Ok(state) => state.login.clone(),
        Err(_) => None,
    };
    let mut checks = vec![docker_check(state, folder)];
    checks.extend(login.map(|login| tailscale_login_check(&login, folder)));
    checks
}

/// The doctor's line about the Tailscale container's login.
fn tailscale_login_check(login: &docker::Login, folder: &docker::Folder) -> diagnostics::Check {
    match login {
        docker::Login::LoggedIn => diagnostics::Check::ok(DOCKER_TAILSCALE_CHECK, "logged in"),
        docker::Login::NeedsLogin => diagnostics::Check::problem(
            DOCKER_TAILSCALE_CHECK,
            "not logged in: the machine's key expired, or it was logged out, so it is off the tailnet and its HTTPS address does not answer",
            "run `kkt setup` again and log in with the link it shows",
        ),
        docker::Login::Unreachable => diagnostics::Check::problem(
            DOCKER_TAILSCALE_CHECK,
            "the Tailscale container does not answer",
            format!(
                "start it with `cd {} && docker compose up -d`",
                clients::shell_word(&folder.path().display().to_string())
            ),
        ),
        docker::Login::Other(state) => diagnostics::Check::problem(
            DOCKER_TAILSCALE_CHECK,
            format!("Tailscale says: {state}"),
            "check the machine in the Tailscale admin console, or run `kkt setup` again",
        ),
    }
}

/// The doctor's line about kkt in Docker, from what `docker::status` found
/// in the managed `folder`.
fn docker_check(
    state: Result<docker::State, docker::DockerError>,
    folder: &docker::Folder,
) -> diagnostics::Check {
    let shown = folder.path().display().to_string();
    match state {
        Ok(state) if !state.files_present => diagnostics::Check::problem(
            DOCKER_CHECK,
            format!("its files are missing from {shown}"),
            "run `kkt setup` again",
        ),
        Ok(state) if state.running => diagnostics::Check::ok(
            DOCKER_CHECK,
            format!(
                "running kkt {}",
                state.version.as_deref().unwrap_or("(version unknown)")
            ),
        ),
        Ok(_) => diagnostics::Check::problem(
            DOCKER_CHECK,
            "not running",
            format!(
                // From inside the folder: Compose reads a relative
                // COMPOSE_FILE in .env against the current folder.
                "start it with `cd {} && docker compose up -d`",
                clients::shell_word(&shown)
            ),
        ),
        Err(error) => diagnostics::Check::problem(
            DOCKER_CHECK,
            error.to_string(),
            "check that Docker is running, then run `kkt status`",
        ),
    }
}

/// The doctor's lines for `checks`, and whether all of them are ok.
fn doctor_report(checks: Vec<diagnostics::Check>) -> (String, bool) {
    let mut report = String::new();
    let mut all_ok = true;
    for check in checks {
        all_ok &= check.ok;
        report.push_str(&format!(
            "{}: {}: {}\n",
            if check.ok { "ok" } else { "problem" },
            check.name,
            check.detail
        ));
        if let Some(fix) = check.fix {
            report.push_str(&format!("  fix: {fix}\n"));
        }
    }
    (report, all_ok)
}

/// Runs one command.
async fn run(env: &Env, command: Command) -> Result<(), String> {
    match command {
        Command::Serve {
            config,
            ignore_setup,
            flags,
        } => {
            if !ignore_setup {
                check_setup(env)?;
            }
            serve_from_config(env, config, flags).await
        }
        Command::Key {
            command: KeyCommand::Create { path, force, show },
        } => create_key(env, path, force, show),
        Command::Agent {
            command,
            db,
            config,
        } => agent(env, command, db, config).await,
        Command::Service { command } => service(env, command),
        Command::Backup { folder, db, config } => mode_backup(env, folder.as_deref(), db, config),
        Command::Status => mode_status(env),
        Command::Upgrade { allow_downgrade } => mode_upgrade(env, allow_downgrade),
        Command::Uninstall => mode_uninstall(env),
        Command::Key {
            command: KeyCommand::Header { key_file, json },
        } => key_header(env, key_file, json),
        Command::Key {
            command: KeyCommand::Show { key_file },
        } => key_show(env, key_file),
        Command::Connect {
            client,
            connection,
            write,
        } => connect(env, &client, connection, write),
        Command::Doctor { connection } => doctor(env, connection).await,
        Command::Setup { answers } => run_setup(env, answers).await,
    }
}

/// Sends the log to stdout, or to `log_file`, kept within
/// [`LogPolicy::DEFAULT`]. kkt's own code logs at `own`, the libraries it
/// uses at `dependencies`.
fn init_logging(
    log_file: Option<&Path>,
    own: LogLevel,
    dependencies: LogLevel,
) -> Result<(), String> {
    let filter = LOG_OWN_TARGETS.iter().fold(
        Targets::new().with_default(level_filter(dependencies)),
        |filter, target| filter.with_target(*target, level_filter(own)),
    );
    let format = tracing_subscriber::fmt::layer().with_target(false);
    let started = match log_file {
        // Color codes only for a terminal: the journal would show them as
        // raw escape characters.
        None => tracing_subscriber::registry()
            .with(format.with_ansi(std::io::stdout().is_terminal()))
            .with(filter)
            .try_init(),
        Some(path) => {
            let policy = LogPolicy::DEFAULT;
            policy
                .check()
                .map_err(|e| format!("invalid setting: {e}"))?;
            let file = RotatingFile::open(path, policy, Arc::new(SystemClock)).map_err(|e| {
                format!(
                    "invalid setting: log_file could not be opened at {}: {e}",
                    path.display()
                )
            })?;
            tracing_subscriber::registry()
                .with(format.with_ansi(false).with_writer(Mutex::new(file)))
                .with(filter)
                .try_init()
        }
    };
    started.map_err(|e| format!("could not start logging: {e}"))
}

fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    }
}

/// The environment values that decide default paths.
fn process_env() -> Env {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    Env {
        home: var("HOME"),
        xdg_config_home: var("XDG_CONFIG_HOME"),
        xdg_data_home: var("XDG_DATA_HOME"),
    }
}

/// `kkt setup`: asks on the terminal if there is one, and does the work
/// through [`OsSteps`].
async fn run_setup(env: &Env, flags: SetupFlags) -> Result<(), String> {
    let options = flags.install_options();
    let answers = flags.answers()?;
    let ask = TerminalAsk {
        interactive: std::io::stdin().is_terminal(),
    };
    let steps = OsSteps { env, options };
    // Steps are synchronous; doctor's checks are async, so they block on
    // this runtime from inside.
    tokio::task::block_in_place(|| {
        setup::run(&answers, Build::CURRENT.version, &ask, &steps).map(|_| ())
    })
}

/// Asks on the terminal, or, with none, refuses each question naming the
/// flag that answers it.
struct TerminalAsk {
    interactive: bool,
}

impl TerminalAsk {
    fn choose_with(
        question: setup::Question,
        options: &[&str],
        default: usize,
        mut line: impl FnMut(&str) -> Result<String, String>,
    ) -> Result<usize, String> {
        let mut prompt = format!("{}\n", question.text);
        for (i, option) in options.iter().enumerate() {
            prompt.push_str(&format!("  {}. {option}\n", i + 1));
        }
        prompt.push_str(&format!("Choose a number [{}]: ", default + 1));
        loop {
            let answer = line(&prompt)?;
            if answer.trim().is_empty() {
                return Ok(default);
            }
            match answer.trim().parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                _ => println!("Type a number from 1 to {}.", options.len()),
            }
        }
    }

    fn confirm_with(
        question: setup::Question,
        mut line: impl FnMut(&str) -> Result<String, String>,
    ) -> Result<bool, String> {
        let choices = match question.default {
            Some(true) => "[Y/n]",
            Some(false) => "[y/N]",
            None => "[y/n]",
        };
        let prompt = format!("{} {choices} ", question.text);
        loop {
            let answer = line(&prompt)?;
            match answer.trim().to_lowercase().as_str() {
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                "" => {
                    if let Some(default) = question.default {
                        return Ok(default);
                    }
                    println!("Type y or n.");
                }
                _ => println!("Type y or n."),
            }
        }
    }

    fn line(&self, question: setup::Question, prompt: &str) -> Result<String, String> {
        if !self.interactive {
            return Err(setup::unanswered(question));
        }
        print!("{prompt}");
        std::io::stdout()
            .flush()
            .map_err(|e| format!("could not write to the terminal: {e}"))?;
        let mut line = String::new();
        let read = std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("could not read the answer: {e}"))?;
        if read == 0 {
            return Err(format!("no answer: {}", setup::unanswered(question)));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

impl setup::Ask for TerminalAsk {
    fn choose(
        &self,
        question: setup::Question,
        options: &[&str],
        default: usize,
    ) -> Result<usize, String> {
        Self::choose_with(question, options, default, |prompt| {
            self.line(question, prompt)
        })
    }

    fn confirm(&self, question: setup::Question) -> Result<bool, String> {
        Self::confirm_with(question, |prompt| self.line(question, prompt))
    }

    fn text(&self, question: setup::Question) -> Result<String, String> {
        self.line(question, &format!("{} ", question.text))
    }

    fn tell(&self, text: &str) {
        println!("{text}");
    }
}

/// How to install, beyond what setup asks.
struct InstallOptions {
    ts_hostname: String,
    ts_authkey_file: Option<PathBuf>,
    kkt_version: String,
    docker_project: String,
    docker_port: u16,
}

/// What `kkt setup` does to this machine.
struct OsSteps<'a> {
    env: &'a Env,
    options: InstallOptions,
}

/// The address `kkt serve` would listen on with the default config file.
fn serve_bind(env: &Env) -> Result<SocketAddr, String> {
    let loaded = load_serve_options(env, None, ServeOptions::NONE)?;
    let sources = loaded.sources();
    let settings = loaded
        .flags
        .over(loaded.from_file)
        .resolve(
            env.default_db(Platform::CURRENT).unwrap_or_default(),
            env.access_key_file(),
        )
        .map_err(|e| sources.invalid(e))?;
    Ok(settings.bind)
}

/// Adds `host` to `allow_hosts` in the default config file, making the file
/// if there is none.
fn allow_host_in_config(env: &Env, host: &str) -> Result<(), String> {
    let path = env
        .config_file()
        .ok_or("set HOME to keep kkt's config file")?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    let updated = settings::with_allowed_host(&text, host)
        .map_err(|e| format!("invalid setting: {e} (in {})", path.display()))?;
    if updated == text {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, updated).map_err(|e| format!("could not write {}: {e}", path.display()))
}

impl setup::Steps for OsSteps<'_> {
    fn install_service(
        &self,
        tailscale: bool,
        allow_hosts: &[String],
    ) -> Result<setup::Installed, String> {
        // Before anything changes: Tailscale must be logged in, with HTTPS.
        let node = match tailscale {
            true => Some(tailscale::prepare(&tailscale::OsHost)?),
            false => None,
        };
        if let Some(node) = &node {
            allow_host_in_config(self.env, &node.dns_name)?;
        }
        for host in allow_hosts {
            allow_host_in_config(self.env, host)?;
        }
        service(
            self.env,
            ServiceCommand::Install {
                config: None,
                db: None,
                access_key_file: None,
                no_access_key: false,
                dry_run: false,
            },
        )?;
        let no_flags = ConnectionFlags {
            config: None,
            url: None,
            key_file: None,
        };
        let connection = serve_connection(self.env, no_flags, false)?;
        let url = match &node {
            Some(node) => {
                tailscale::serve(&tailscale::OsHost, serve_bind(self.env)?)?;
                format!("https://{}/mcp", node.dns_name)
            }
            None => connection.url,
        };
        Ok(setup::Installed {
            url,
            key_file: connection.key_file,
        })
    }

    fn install_docker(
        &self,
        tailscale: bool,
        allow_hosts: &[String],
    ) -> Result<(setup::Installed, setup::DockerSetup), String> {
        let options = &self.options;
        let folder = self
            .env
            .data_dir(Platform::CURRENT)
            .ok_or("set HOME to keep kkt's Docker files")?
            .join(config::DOCKER_FOLDER);
        let key_file = self
            .env
            .config_dir()
            .ok_or("set HOME to keep the access key")?
            .join(config::DOCKER_KEY_FILE_NAME);
        let auth_key = match &options.ts_authkey_file {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|e| format!("could not read {}: {e}", path.display()))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        };
        let inputs = docker::Inputs {
            version: options.kkt_version.clone(),
            project: options.docker_project.clone(),
            port: options.docker_port,
            allow_hosts: allow_hosts.to_vec(),
            tailscale: tailscale.then(|| docker::Tailscale {
                hostname: options.ts_hostname.clone(),
                auth_key,
            }),
        };
        let provisioned = tokio::runtime::Handle::current()
            .block_on(docker::provision(
                &docker::OsHost,
                &SystemClock,
                &browser_login::TerminalLink::new(),
                &docker::Folder(folder.clone()),
                &inputs,
                &key_file,
            ))
            .map_err(|e| e.to_string())?;
        let (url, hostname, tailnet) = match provisioned.node {
            Some((hostname, tailnet)) => (
                format!("https://{hostname}.{tailnet}/mcp"),
                Some(hostname),
                Some(tailnet),
            ),
            None => (
                format!("http://127.0.0.1:{}/mcp", options.docker_port),
                None,
                None,
            ),
        };
        Ok((
            setup::Installed {
                url,
                key_file: Some(key_file),
            },
            setup::DockerSetup {
                folder,
                tailscale,
                hostname,
                tailnet,
            },
        ))
    }

    fn stage_key(&self, key: &str) -> Result<PathBuf, String> {
        let dir = self
            .env
            .config_dir()
            .ok_or("set HOME to keep the key in ~/.config/kkt")?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let path = dir.join(format!("{}.partial", config::CLIENT_KEY_FILE_NAME));
        write_key_file(&path, key, true)?;
        Ok(path)
    }

    fn keep_key(&self, staged: &Path, url: &str) -> Result<PathBuf, String> {
        let dir = staged.parent().unwrap_or(Path::new("."));
        // One file per server, so a key for another server, which an
        // earlier setup record may name, is never replaced.
        let server: String = Sha256::digest(url.as_bytes())
            .iter()
            .take(6)
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = dir.join(format!("{}-{server}", config::CLIENT_KEY_FILE_NAME));
        std::fs::rename(staged, &path)
            .map_err(|e| format!("could not move the key to {}: {e}", path.display()))?;
        Ok(path)
    }

    fn discard_key(&self, staged: &Path) -> Result<(), String> {
        std::fs::remove_file(staged)
            .map_err(|e| format!("could not remove {}: {e}", staged.display()))
    }

    fn detect_clients(&self) -> Vec<Client> {
        let dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        let on_path = |program: &str| dirs.iter().any(|dir| dir.join(program).is_file());
        Client::ALL
            .into_iter()
            .filter(|client| client.installed(self.env, &on_path))
            .collect()
    }

    fn write_record(&self, record: &setup::Record) -> Result<(), String> {
        write_setup_record(self.env, record)
    }

    fn connect(&self, client: Client, record: &setup::Record) -> Result<String, String> {
        let executable = kkt_path()?;
        let setup = Setup {
            env: self.env,
            url: &record.url,
            executable: &executable,
            key_file: record.key_file.as_deref(),
        };
        clients::configure(client, &setup, true)
    }

    fn doctor(&self, record: &setup::Record) -> Result<String, String> {
        // A server set up just now may not answer yet; a client's server
        // either answers or the address is wrong, so it isn't waited for.
        let wait = match record.mode {
            setup::Mode::Client => std::time::Duration::ZERO,
            _ => config::SETUP_SERVER_WAIT,
        };
        let checks = tokio::runtime::Handle::current().block_on(inspect_until_reachable(
            &record.url,
            record.key_file.as_deref(),
            &SystemClock,
            wait,
        ))?;
        match doctor_report(checks) {
            (report, true) => Ok(report.trim_end().to_string()),
            (report, false) => Err(report.trim_end().to_string()),
        }
    }

    fn check_address(&self, url: &str) -> Result<(), String> {
        let mut checks =
            tokio::runtime::Handle::current().block_on(diagnostics::inspect_address(url))?;
        point_server_fix_at(
            &mut checks,
            "check the address, and that the server is running on its machine",
        );
        only_problems(checks)
    }

    fn check_key(&self, url: &str, key: &setup::ClientKey) -> Result<(), String> {
        let runtime = tokio::runtime::Handle::current();
        let checks = match key {
            setup::ClientKey::None => runtime.block_on(diagnostics::inspect_with_key(url, None)),
            setup::ClientKey::Typed(key) => {
                runtime.block_on(diagnostics::inspect_with_key(url, Some(key)))
            }
            setup::ClientKey::File(path) => runtime.block_on(diagnostics::inspect(url, Some(path))),
        }?;
        only_problems(checks)
    }
}

/// The doctor's lines for the problems in `checks` as an error, if any.
fn only_problems(checks: Vec<diagnostics::Check>) -> Result<(), String> {
    let problems: Vec<_> = checks.into_iter().filter(|check| !check.ok).collect();
    if problems.is_empty() {
        return Ok(());
    }
    Err(doctor_report(problems).0.trim_end().to_string())
}

/// Runs doctor's connection checks on `url`, again every
/// `SETUP_SERVER_POLL` while the server doesn't answer at all, for up to
/// `wait` on `clock`. Returns the last checks.
async fn inspect_until_reachable(
    url: &str,
    key_file: Option<&Path>,
    clock: &dyn Clock,
    wait: std::time::Duration,
) -> Result<Vec<diagnostics::Check>, String> {
    let end = clock.now() + chrono::Duration::from_std(wait).unwrap_or(chrono::Duration::MAX);
    let every =
        chrono::Duration::from_std(config::SETUP_SERVER_POLL).unwrap_or(chrono::Duration::MAX);
    let mut said = false;
    loop {
        let began = clock.now();
        let checks = diagnostics::inspect(url, key_file).await?;
        if !diagnostics::unreachable(&checks) || began >= end {
            return Ok(checks);
        }
        if !said {
            println!(
                "Waiting for the server to answer at {url} (a new Tailscale node gets its \
                 HTTPS certificate on first use)..."
            );
            said = true;
        }
        clock.sleep_until(began + every).await;
    }
}

/// This machine's setup record, if `kkt setup` wrote one.
fn read_setup(env: &Env) -> Result<Option<setup::Record>, String> {
    let Some(path) = setup::record_path(env) else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => setup::Record::parse(&text, &path).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// Refuses to start a server on a machine `kkt setup` set up otherwise.
fn check_setup(env: &Env) -> Result<(), String> {
    let record = read_setup(env)?;
    let service_running = match record.as_ref().map(|r| &r.mode) {
        Some(setup::Mode::Service { .. }) => {
            let state = Manager::for_this_os(env).and_then(|m| m.state(&OsSystem));
            match state {
                Ok(state) => Some(state.running),
                Err(e) => {
                    // Not knowing is no reason to refuse: a server already
                    // on the port stops this one with its own message.
                    eprintln!("warning: could not tell whether the kkt service is running: {e}");
                    None
                }
            }
        }
        _ => None,
    };
    setup::guard(record.as_ref(), service_running, false)
}

/// `kkt serve`: flags over the config file over the defaults, with the
/// database in the per-user data directory unless one is given.
async fn serve_from_config(
    env: &Env,
    config: Option<PathBuf>,
    flags: ServeFlags,
) -> Result<(), String> {
    let loaded = load_serve_options(env, config, flags.options())?;
    let sources = loaded.sources();
    let options = loaded.flags.over(loaded.from_file);
    let default_db = match &options.db {
        // Given, so the default is never used.
        Some(given) => given.clone(),
        None => {
            let db = env.default_db(Platform::CURRENT).ok_or(
                "no home directory to keep the database in: set HOME or pass --db".to_string(),
            )?;
            if let Some(dir) = db.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            }
            db
        }
    };
    let settings = options
        .resolve(default_db, env.access_key_file())
        .map_err(|e| sources.invalid(e))?;
    // Before logging starts at that level, not after.
    config::check_dependency_log_level(settings.dependency_log_level)
        .map_err(|e| sources.invalid(e))?;
    let log_file = settings.log_file.clone();
    init_logging(
        log_file.as_deref(),
        settings.log_level,
        settings.dependency_log_level,
    )?;
    let served = serve(settings, &sources).await;
    if let (Some(file), Err(e)) = (&log_file, &served) {
        // Someone at a terminal reads stderr, not the log file.
        eprintln!("error: {e} (logged in {})", file.display());
    }
    served
}

/// What `kkt serve` reads before applying defaults: the config file, if
/// any, what it gives, and what the flags give.
struct Loaded {
    config_file: Option<PathBuf>,
    from_file: ServeOptions,
    flags: ServeOptions,
}

impl Loaded {
    fn sources(&self) -> Sources {
        Sources {
            file: self
                .config_file
                .clone()
                .map(|path| (path, self.from_file.clone())),
            flags: self.flags.clone(),
        }
    }
}

/// Reads the config file `kkt serve` would read, and puts it with `flags`.
fn load_serve_options(
    env: &Env,
    config: Option<PathBuf>,
    flags: ServeOptions,
) -> Result<Loaded, String> {
    let (config_file, from_file) = read_config(env, config)?;
    Ok(Loaded {
        config_file,
        from_file,
        flags,
    })
}

/// The config file to use (`config`, else the default one if it exists),
/// and what it gives.
fn read_config(
    env: &Env,
    config: Option<PathBuf>,
) -> Result<(Option<PathBuf>, ServeOptions), String> {
    let config_file = match config {
        Some(path) => Some(path),
        None => env.config_file().filter(|path| path.exists()),
    };
    let from_file = match &config_file {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                format!(
                    "invalid setting: config could not be read from {}: {e}",
                    path.display()
                )
            })?;
            let dir = path.parent().unwrap_or(Path::new("."));
            settings::parse_file(&text, dir, env)
                .map_err(|e| format!("invalid setting: {e} (in {})", path.display()))?
        }
        None => ServeOptions::NONE,
    };
    Ok((config_file, from_file))
}

/// `kkt agent`: the operator's commands, on the database `kkt serve` would
/// use unless `--db` names another. Never creates a database.
async fn agent(
    env: &Env,
    command: AgentCommand,
    db: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<(), String> {
    // An explicit --db needs nothing from the config file, so it is not read.
    let config_db = match db {
        Some(_) => None,
        None => read_config(env, config)?.1.db,
    };
    let path = existing_db(env, db, config_db)?;
    let db = Db::open(&path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let operator = Operator::new(db, Arc::new(SystemClock));
    match command {
        AgentCommand::List => {
            let agents = operator.agents().await.map_err(|e| e.to_string())?;
            if agents.is_empty() {
                println!("no agents in {}", path.display());
            }
            for a in agents {
                let seen = a.last_seen.as_deref().unwrap_or("never");
                let revoked = match &a.revoked_at {
                    Some(at) => format!("  revoked {at}"),
                    None => String::new(),
                };
                println!(
                    "{}  created {}  last seen {seen}{revoked}",
                    a.name, a.created_at
                );
            }
        }
        AgentCommand::Revoke { name } => {
            match operator
                .revoke(name.clone())
                .await
                .map_err(|e| e.to_string())?
            {
                Revoked::Now { at } => println!("revoked {name} at {at}"),
                Revoked::Already { at } => println!("{name} was already revoked at {at}"),
            }
        }
    }
    Ok(())
}

/// The database file `kkt serve` would use given the config file's `db`,
/// unless `db` names another. It must exist.
fn existing_db(
    env: &Env,
    db: Option<PathBuf>,
    config_db: Option<PathBuf>,
) -> Result<PathBuf, String> {
    let path = match db.or(config_db) {
        Some(path) => path,
        None => env.default_db(Platform::CURRENT).ok_or(
            "no home directory to find the database in: set HOME or pass --db".to_string(),
        )?,
    };
    if !path.exists() {
        return Err(format!(
            "no database at {}; pass --db with the server's database file",
            path.display()
        ));
    }
    Ok(path)
}

/// Resolve lifecycle commands through the setup record. An unrecorded
/// machine still supports the original local-database backup command.
fn write_setup_record(env: &Env, record: &setup::Record) -> Result<(), String> {
    let path = setup::record_path(env).ok_or("set HOME to keep the setup record")?;
    let text = record.render()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    // Written whole, then renamed, so a failure leaves the earlier record.
    let partial = path.with_extension("toml.partial");
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

fn mode_backup(
    env: &Env,
    folder: Option<&Path>,
    db: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<(), String> {
    match read_setup(env)?.map(|record| record.mode) {
        Some(setup::Mode::Docker(install)) => {
            if db.is_some() || config.is_some() {
                return Err("Docker backup uses the configured container; --db and --config apply only to a local service".into());
            }
            let written = docker::backup(&docker::OsHost, &docker::Folder(install.folder), folder)
                .map_err(|e| e.to_string())?;
            println!("Backed up to {}", written.display());
            Ok(())
        }
        Some(setup::Mode::Client) => {
            Err("this machine is a client; back up the server on the machine where it runs".into())
        }
        Some(setup::Mode::Service { .. }) | None => {
            let folder = folder.ok_or("a local backup needs FOLDER; run `kkt backup <folder>`")?;
            backup(env, folder, db, config)
        }
    }
}

fn mode_status(env: &Env) -> Result<(), String> {
    let Some(record) = read_setup(env)? else {
        println!(
            "No setup record; checking this machine's kkt service. Run `kkt setup` to configure this machine."
        );
        return service(env, ServiceCommand::Status);
    };
    println!("Mode: {}\nServer: {}", record.mode.name(), record.url);
    match record.mode {
        setup::Mode::Service { .. } => service(env, ServiceCommand::Status),
        setup::Mode::Docker(install) => {
            let folder = docker::Folder(install.folder);
            let state = docker::status(&docker::OsHost, &folder).map_err(|e| e.to_string())?;
            println!("Docker folder: {}", folder.path().display());
            println!(
                "Settings present: {}\nRunning: {}\nVersion: {}\nTailscale: {}",
                state.files_present,
                state.running,
                state.version.as_deref().unwrap_or("unknown"),
                state.tailscale
            );
            match &state.login {
                Some(docker::Login::LoggedIn) => println!("Tailscale login: logged in"),
                Some(docker::Login::NeedsLogin) => println!(
                    "Tailscale login: needed. The machine's key expired, or it was logged out, so it is off the tailnet. Run `kkt setup` to log in again."
                ),
                Some(docker::Login::Unreachable) => {
                    println!("Tailscale login: unknown; the Tailscale container does not answer")
                }
                Some(docker::Login::Other(state)) => println!("Tailscale login: {state}"),
                None => {}
            }
            Ok(())
        }
        setup::Mode::Client => {
            println!("Client only; the server runs on another machine.");
            Ok(())
        }
    }
}

fn recorded_setup(env: &Env) -> Result<setup::Record, String> {
    read_setup(env)?.ok_or_else(|| "this machine has no setup record; run `kkt setup` first".into())
}

fn mode_upgrade(env: &Env, allow_downgrade: bool) -> Result<(), String> {
    let mut record = recorded_setup(env)?;
    let current = match &record.mode {
        setup::Mode::Client => {
            return Err(
                "this machine is a client; upgrade the server on the machine where it runs".into(),
            );
        }
        setup::Mode::Docker(install) => {
            let path = install.folder.join(docker::ENV_FILE);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            docker::version_in_env(&text)
                .ok_or_else(|| format!("{} has no KKT_VERSION", path.display()))?
        }
        setup::Mode::Service { .. } => record.written_by.clone(),
    };
    if setup::upgrade(&current, Build::CURRENT.version, allow_downgrade)? == setup::Upgrade::Already
    {
        println!("Server is already at kkt {}", Build::CURRENT.version);
        return Ok(());
    }
    match &record.mode {
        setup::Mode::Docker(install) => {
            docker::upgrade(
                &docker::OsHost,
                &docker::Folder(install.folder.clone()),
                Build::CURRENT.version,
            )
            .map_err(|e| e.to_string())?;
        }
        setup::Mode::Service { .. } => {
            // Validate the replacement service before doing anything. Its
            // program is current_exe, i.e. the binary just installed by the user.
            let plan = service_plan(env, None, None, None, false)?;
            let folder = env
                .config_dir()
                .ok_or("set HOME to find the backup folder")?
                .join(config::SERVICE_BACKUP_FOLDER);
            backup(env, &folder, None, None)?;
            let manager = Manager::for_this_os(env).map_err(|e| e.to_string())?;
            for line in manager
                .install(&OsSystem, &plan.spec)
                .map_err(|e| e.to_string())?
            {
                println!("{line}");
            }
        }
        setup::Mode::Client => {
            return Err(
                "this machine is a client; upgrade the server on the machine where it runs".into(),
            );
        }
    }
    record.written_by = Build::CURRENT.version.to_string();
    write_setup_record(env, &record)?;
    println!("Upgraded the server to kkt {}", Build::CURRENT.version);
    Ok(())
}

fn mode_uninstall(env: &Env) -> Result<(), String> {
    let record = recorded_setup(env)?;
    match record.mode {
        setup::Mode::Docker(install) => {
            docker::down(&docker::OsHost, &docker::Folder(install.folder), false)
                .map_err(|e| e.to_string())?;
        }
        setup::Mode::Service { tailscale: shared } => {
            if shared {
                let loaded = load_serve_options(env, None, ServeOptions::NONE)?;
                let sources = loaded.sources();
                let settings = loaded
                    .from_file
                    .resolve(
                        env.default_db(Platform::CURRENT).unwrap_or_default(),
                        env.access_key_file(),
                    )
                    .map_err(|e| sources.invalid(e))?;
                tailscale::stop(&tailscale::OsHost, settings.bind)?;
            }
            service(env, ServiceCommand::Uninstall)?;
        }
        setup::Mode::Client => {}
    }
    let path = setup::record_path(env).ok_or("set HOME to find the setup record")?;
    std::fs::remove_file(&path).map_err(|e| format!("could not remove {}: {e}", path.display()))?;
    println!(
        "Removed this machine's setup. Data, keys, backups and client configurations were kept."
    );
    Ok(())
}

/// `kkt backup`: backs up the database `kkt serve` would use (or `db`),
/// with the config file and access key it would read, into `folder`.
fn backup(
    env: &Env,
    folder: &Path,
    db: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<(), String> {
    let (config_file, options) = read_config(env, config)?;
    let database = existing_db(env, db, options.db.clone())?;
    let access_key = match (&options.access_key_file, options.no_access_key) {
        (Some(path), _) => Some(path.clone()),
        (None, Some(true)) => None,
        (None, _) => env.access_key_file().filter(|path| path.exists()),
    };
    let contents = backup::Contents {
        database: &database,
        config: config_file.as_deref(),
        access_key: access_key.as_deref(),
    };
    let written = write_backup(folder, &SystemClock, &contents)?;
    println!("Backed up to {}", written.display());
    println!("  database {}", database.display());
    if let Some(path) = &config_file {
        println!("  config file {}", path.display());
    }
    match &access_key {
        Some(path) => println!("  access key {}", path.display()),
        None => println!("  no access key"),
    }
    Ok(())
}

/// Writes a backup of `contents` into `folder`, copying the database with
/// SQLite's online backup.
fn write_backup(
    folder: &Path,
    clock: &dyn Clock,
    contents: &backup::Contents,
) -> Result<PathBuf, String> {
    backup::write(
        folder,
        clock.now(),
        Build::CURRENT.version,
        contents,
        |from, to| store::copy_database(from, to).map_err(|e| e.to_string()),
    )
}

/// Before the server upgrades the tables in `database`, backs it up beside
/// it and prunes those backups to the newest `keep`. Only the database: an
/// upgrade changes nothing else, and a copy of the key in the data folder
/// would be one more place it could leak from. Any failure to back up stops
/// the server before the database is changed.
fn backup_before_upgrade(database: &Path, keep: usize, clock: &dyn Clock) -> Result<(), String> {
    let Some(upgrade) = store::pending_upgrade(database)
        .map_err(|e| format!("could not read {}: {e}", database.display()))?
    else {
        return Ok(());
    };
    let folder = database
        .parent()
        .unwrap_or(Path::new("."))
        .join(config::UPGRADE_BACKUP_FOLDER);
    let contents = backup::Contents {
        database,
        config: None,
        access_key: None,
    };
    let written = write_backup(&folder, clock, &contents).map_err(|e| {
        format!(
            "the database needs upgrading (schema {} to {}), and kkt backs it up first, \
             but the backup failed, so the server did not start and the database is \
             unchanged: {e}",
            upgrade.from, upgrade.to
        )
    })?;
    tracing::info!(
        "backed up the database to {} before upgrading it from schema {} to {}",
        written.display(),
        upgrade.from,
        upgrade.to
    );
    // The backup is made, so the upgrade is safe; an old backup left behind
    // only takes space.
    match backup::prune(&folder, keep) {
        Ok(removed) => {
            for path in removed {
                tracing::info!("removed the old backup {}", path.display());
            }
        }
        Err(e) => tracing::warn!("could not remove old backups: {e}"),
    }
    Ok(())
}

/// Where a server's settings came from, so a bad one can be traced to the
/// config file when that is where it was set.
struct Sources {
    flags: ServeOptions,
    /// The config file read, and what it gave.
    file: Option<(PathBuf, ServeOptions)>,
}

impl Sources {
    /// The message for a bad setting, naming the config file if the setting
    /// came from there.
    fn invalid(&self, e: InvalidSetting) -> String {
        match &self.file {
            Some((path, file)) if file.gives(e.name) && !self.flags.gives(e.name) => {
                format!("invalid setting: {e} (set in {})", path.display())
            }
            _ => format!("invalid setting: {e}"),
        }
    }
}

/// Settings that passed every startup check, with the files they name read.
struct Checked {
    db_path: PathBuf,
    bind: SocketAddr,
    config: AppConfig,
}

/// Runs the startup checks on `settings` and reads the files they name.
/// The errors name the bad setting, and the config file if it set it.
fn check_settings(settings: ServeSettings, sources: &Sources) -> Result<Checked, String> {
    let ServeSettings {
        db: db_path,
        bind,
        allow_any_bind,
        limits,
        allow_origins,
        allow_hosts,
        instructions,
        access_key,
        log_file,
        log_level,
        dependency_log_level,
    } = settings;
    config::check_bind(bind, allow_any_bind, access_key != AccessKey::None)
        .map_err(|e| sources.invalid(e))?;
    config::check_dependency_log_level(dependency_log_level).map_err(|e| sources.invalid(e))?;
    limits.check().map_err(|e| sources.invalid(e))?;
    let allowed_hosts = AllowedHosts::new(&allow_hosts).map_err(|e| sources.invalid(e))?;
    let instructions = match &instructions {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?,
        None => kashkeshet::DEFAULT_INSTRUCTIONS.to_string(),
    };
    // Only reads the key: `serve` makes a missing default key first, and
    // `kkt service install` checks settings without making anything.
    let admission = match access_key {
        AccessKey::None => Admission::Open,
        AccessKey::File(path) | AccessKey::Default(path) => {
            Admission::SharedKey(read_key(&path, sources)?)
        }
    };
    let config = AppConfig {
        admission,
        limits,
        allowed_origins: allow_origins,
        allowed_hosts,
        instructions,
        config_file: sources.file.as_ref().map(|(p, _)| p.display().to_string()),
        log_file: log_file.map(|p| p.display().to_string()),
        log_level,
        dependency_log_level,
        allow_any_bind,
    };
    Ok(Checked {
        db_path,
        bind,
        config,
    })
}

async fn serve(settings: ServeSettings, sources: &Sources) -> Result<(), String> {
    make_default_key(&settings.access_key)?;
    let Checked {
        db_path,
        bind,
        config,
    } = check_settings(settings, sources)?;
    let clock = Arc::new(SystemClock);
    backup_before_upgrade(&db_path, config.limits.upgrade_backups_kept, clock.as_ref())?;
    let db =
        Db::open(&db_path).map_err(|e| format!("could not open {}: {e}", db_path.display()))?;
    let run = ServerRun::new(Build::CURRENT, &config);
    let access = match config.admission.key_source() {
        Some(file) => format!("access key from {file}"),
        None => "no access key".to_string(),
    };
    let settings_from = match &config.config_file {
        Some(file) => format!(", config {file}"),
        None => String::new(),
    };
    let run_id = provenance::record(&db, &run, clock.now())
        .await
        .map_err(|e| format!("could not record this server run: {e}"))?;
    let shutdown = Shutdown::new();
    let signals =
        StopSignals::listen().map_err(|e| format!("could not listen for shutdown signals: {e}"))?;
    let parts = Parts::new(db, clock, Arc::new(OsEntropy), &config.limits, run_id);
    let router = app::build(parts, config, shutdown.clone());
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("could not listen on {bind}: {e}"))?;
    tracing::info!(
        "kashkeshet {} (commit {}, run {run_id}) listening: web page http://{bind}/ , MCP http://{bind}/mcp ({access}), database {}{settings_from}",
        run.version,
        run.commit,
        db_path.display()
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            match signals.stop_on_signal(shutdown).await {
                Ok(signal) => tracing::info!("received {signal}; shutting down"),
                Err(e) => tracing::error!("could not listen for shutdown signals: {e}"),
            }
        })
        .await
        .map_err(|e| format!("server error: {e}"))
}

/// `kkt key create`: writes a new key to a file only the owner can read,
/// and says how to use it. The key itself is printed only with `--show`.
/// Makes the default key file if it is the one in use and does not exist
/// yet, and logs where it is (never the key).
fn make_default_key(access_key: &AccessKey) -> Result<(), String> {
    let AccessKey::Default(path) = access_key else {
        return Ok(());
    };
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    write_key_file(path, &SharedKey::generate(&OsEntropy), false)?;
    tracing::info!("created a new access key at {}", path.display());
    Ok(())
}

/// The access key in the file at `path`.
fn read_key(path: &Path, sources: &Sources) -> Result<SharedKey, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "invalid setting: access_key_file could not be read from {}: {e}",
            path.display()
        )
    })?;
    SharedKey::new(&text, path.display().to_string()).map_err(|e| sources.invalid(e))
}

fn create_key(env: &Env, path: Option<PathBuf>, force: bool, show: bool) -> Result<(), String> {
    let path = match path {
        Some(path) => path,
        None => env
            .access_key_file()
            .ok_or("no home directory to keep the key in: set HOME or pass --path".to_string())?,
    };
    let key = new_key_file(&path, force)?;
    let shown = path.display();
    println!("Wrote a new access key to {shown}");
    println!();
    println!("Start the server with it:");
    println!("  kkt serve --access-key-file {shown}");
    println!("or put this line in the config file:");
    println!("  access_key_file = \"{shown}\"");
    println!();
    println!("Clients send it as the header `Authorization: Bearer <key>`. For example:");
    println!("  export KKT_ACCESS_KEY=\"$(cat {shown})\"");
    if show {
        println!();
        println!("The key:");
        println!("{key}");
    }
    Ok(())
}

/// Makes a new random key and writes it to `path`, creating its folder if
/// needed. Returns the key.
fn new_key_file(path: &Path, force: bool) -> Result<String, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let key = SharedKey::generate(&OsEntropy);
    write_key_file(path, &key, force)?;
    Ok(key)
}

/// Writes `key` to `path`, readable and writable only by its owner. Refuses
/// to replace an existing file unless `force`.
fn write_key_file(path: &Path, key: &str, force: bool) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(config::ACCESS_KEY_FILE_MODE);
    }
    let mut file = options.open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!(
            "{} already exists; pass --force to replace it",
            path.display()
        ),
        _ => format!("could not write {}: {e}", path.display()),
    })?;
    // The mode above applies only to a new file; an existing one replaced
    // with --force keeps its old mode unless it is set here.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(
            config::ACCESS_KEY_FILE_MODE,
        ))
        .map_err(|e| format!("could not restrict {}: {e}", path.display()))?;
    }
    writeln!(file, "{key}").map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// `kkt service ...`: runs the command against this system's service
/// manager and prints what happened.
fn service(env: &Env, command: ServiceCommand) -> Result<(), String> {
    let manager = Manager::for_this_os(env).map_err(|e| e.to_string())?;
    let system = OsSystem;
    let said = match command {
        ServiceCommand::Install {
            config,
            db,
            access_key_file,
            no_access_key,
            dry_run,
        } => {
            let plan = service_plan(env, config, db, access_key_file, no_access_key)?;
            if dry_run {
                let dry = DryRun::new(&system);
                manager
                    .install(&dry, &plan.spec)
                    .map_err(|e| e.to_string())?;
                let mut said = vec!["Dry run: nothing was changed.".to_string()];
                if let Some(key) = &plan.new_key {
                    said.push(format!(
                        "Would create a new access key at {}",
                        key.display()
                    ));
                }
                said.extend(dry.actions());
                Ok(said)
            } else {
                let mut said = Vec::new();
                if let Some(key) = &plan.new_key {
                    new_key_file(key, false)?;
                    said.push(format!("Created a new access key at {}", key.display()));
                }
                manager.install(&system, &plan.spec).map(|done| {
                    said.extend(done);
                    said
                })
            }
        }
        ServiceCommand::Uninstall => manager.uninstall(&system),
        ServiceCommand::Start => manager.start(&system),
        ServiceCommand::Stop => manager.stop(&system),
        ServiceCommand::Status => manager.status(&system),
        ServiceCommand::Logs { lines, follow } => manager.logs(&system, lines, follow),
    }
    .map_err(|e| e.to_string())?;
    for line in said {
        println!("{line}");
    }
    Ok(())
}

/// What `kkt service install` does: the service to install, and the access
/// key to create first, if any.
struct ServicePlan {
    spec: Spec,
    new_key: Option<PathBuf>,
}

/// What the service runs: `kkt serve` with absolute paths, since a service
/// starts in a working directory of the manager's choosing. The config file
/// is passed if there is one. The database and the access key file are
/// passed unless the config file names them, so the service never depends
/// on a default being worked out the same way later. A key is always in
/// effect: with none given, the default key file is used, and made if it
/// is missing. The settings are checked now, so a bad one fails here rather
/// than in a loop of restarts.
fn service_plan(
    env: &Env,
    config: Option<PathBuf>,
    db: Option<PathBuf>,
    access_key_file: Option<PathBuf>,
    no_access_key: bool,
) -> Result<ServicePlan, String> {
    let program = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("could not find this program's path: {e}"))?;
    let config = match config {
        Some(path) => Some(path.canonicalize().map_err(|e| {
            format!(
                "invalid setting: config could not be read from {}: {e}",
                path.display()
            )
        })?),
        None => None,
    };
    let absolute = |path: PathBuf| {
        std::path::absolute(&path)
            .map_err(|e| format!("could not make {} absolute: {e}", path.display()))
    };
    let flags = ServeOptions {
        db: db.map(absolute).transpose()?,
        access_key_file: access_key_file.map(absolute).transpose()?,
        no_access_key: no_access_key.then_some(true),
        ..ServeOptions::NONE
    };
    let loaded = load_serve_options(env, config, flags)?;
    let passed_db = match (&loaded.flags.db, &loaded.from_file.db) {
        (Some(db), _) => Some(db.clone()),
        // Read from the config file each time the service starts.
        (None, Some(_)) => None,
        (None, None) => Some(env.default_db(Platform::CURRENT).ok_or(
            "no home directory to keep the database in: set HOME or pass --db".to_string(),
        )?),
    };
    let key_off = loaded
        .flags
        .no_access_key
        .or(loaded.from_file.no_access_key)
        == Some(true);
    let (passed_key, new_key) = match (
        &loaded.flags.access_key_file,
        &loaded.from_file.access_key_file,
    ) {
        (Some(key), _) => (Some(key.clone()), None),
        // Read from the config file each time the service starts.
        (None, Some(_)) => (None, None),
        // No key: nothing to pass or make. A key file given with it is
        // refused by the check below.
        (None, None) if key_off => (None, None),
        (None, None) => {
            let key = env.access_key_file().ok_or(
                "no home directory to keep the access key in: set HOME or pass --access-key-file"
                    .to_string(),
            )?;
            let new = (!key.exists()).then(|| key.clone());
            (Some(key), new)
        }
    };
    let mut options = loaded.flags.clone().over(loaded.from_file.clone());
    let db_in_effect = match (&passed_db, &options.db) {
        (Some(db), _) | (None, Some(db)) => db.clone(),
        (None, None) => return Err("no database for the service".to_string()),
    };
    // A key about to be made is good by construction, so the check skips
    // it; any other key is read and checked.
    match &new_key {
        Some(_) => {
            options.access_key_file = None;
            options.no_access_key = Some(true);
        }
        None => options.access_key_file = passed_key.clone().or(options.access_key_file),
    }
    let db_folder = db_in_effect.parent().map(Path::to_path_buf);
    let sources = loaded.sources();
    let settings = options
        .resolve(db_in_effect, env.access_key_file())
        .map_err(|e| sources.invalid(e))?;
    check_settings(settings, &sources)?;

    let text = |path: &Path| {
        path.to_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
    };
    // The service is the server `kkt setup` meant, so it never stops itself.
    let mut args = vec!["serve".to_string(), "--ignore-setup".to_string()];
    let mut dirs = Vec::new();
    if let Some(config) = &loaded.config_file {
        args.extend(["--config".to_string(), text(config)?]);
    }
    if let Some(db) = &passed_db {
        args.extend(["--db".to_string(), text(db)?]);
    }
    // serve makes the database's folder only for the default database, so
    // make it here for whichever one the service will use.
    if let Some(dir) = db_folder {
        dirs.push(dir);
    }
    if let Some(key) = &passed_key {
        args.extend(["--access-key-file".to_string(), text(key)?]);
    }
    // From the config file, serve reads it again each time it starts.
    if loaded.flags.no_access_key == Some(true) {
        args.push("--no-access-key".to_string());
    }
    Ok(ServicePlan {
        spec: Spec {
            program,
            args,
            dirs,
        },
        new_key,
    })
}

#[cfg(test)]
mod terminal_ask_tests {
    use super::*;
    use kashkeshet::setup::Ask;

    fn question(default: Option<bool>) -> setup::Question<'static> {
        setup::Question {
            text: "Connect Codex to kkt?",
            flag: "--clients",
            default,
        }
    }

    fn mode_question() -> setup::Question<'static> {
        setup::Question {
            text: "What is this machine for?",
            flag: "--mode",
            default: None,
        }
    }

    const MODES: [&str; 3] = ["Docker (Recommended)", "Service", "Client"];

    #[test]
    fn enter_selects_the_displayed_mode_default() {
        for answer in ["", "  "] {
            assert_eq!(
                TerminalAsk::choose_with(mode_question(), &MODES, 0, |prompt| {
                    assert_eq!(
                        prompt,
                        "What is this machine for?\n  1. Docker (Recommended)\n  2. Service\n  3. Client\nChoose a number [1]: "
                    );
                    Ok(answer.into())
                }),
                Ok(0)
            );
        }
    }

    #[test]
    fn explicit_mode_choices_override_the_default() {
        for (answer, expected) in [("1", 0), (" 2 ", 1), ("3", 2)] {
            assert_eq!(
                TerminalAsk::choose_with(mode_question(), &MODES, 0, |_| Ok(answer.into())),
                Ok(expected)
            );
        }
    }

    #[test]
    fn invalid_mode_choices_ask_again() {
        let mut answers = ["0", "4", "-1", "docker", ""].into_iter();
        assert_eq!(
            TerminalAsk::choose_with(mode_question(), &MODES, 0, |_| {
                Ok(answers.next().expect("another answer").into())
            }),
            Ok(0)
        );
        assert_eq!(answers.next(), None);
    }

    #[test]
    fn the_mode_default_never_replaces_a_missing_answer() {
        let ask = TerminalAsk { interactive: false };
        assert_eq!(
            ask.choose(mode_question(), &MODES, 0),
            Err(setup::unanswered(mode_question()))
        );
        assert_eq!(
            TerminalAsk::choose_with(mode_question(), &MODES, 0, |_| Err("no answer".into())),
            Err("no answer".into())
        );
    }

    #[test]
    fn enter_accepts_the_displayed_default() {
        for (default, prompt, expected) in [
            (true, "Connect Codex to kkt? [Y/n] ", true),
            (false, "Connect Codex to kkt? [y/N] ", false),
        ] {
            for answer in ["", "  "] {
                let got = TerminalAsk::confirm_with(question(Some(default)), |shown| {
                    assert_eq!(shown, prompt);
                    Ok(answer.to_string())
                });
                assert_eq!(got, Ok(expected));
            }
        }
    }

    #[test]
    fn explicit_answers_override_any_default() {
        for default in [None, Some(true), Some(false)] {
            for (answer, expected) in [("y", true), (" YES ", true), ("n", false), (" No ", false)]
            {
                assert_eq!(
                    TerminalAsk::confirm_with(question(default), |_| Ok(answer.into())),
                    Ok(expected)
                );
            }
        }
    }

    #[test]
    fn without_a_default_enter_and_invalid_answers_ask_again() {
        let mut answers = ["", "maybe", "n"].into_iter();
        let got = TerminalAsk::confirm_with(question(None), |prompt| {
            assert_eq!(prompt, "Connect Codex to kkt? [y/n] ");
            Ok(answers.next().expect("another answer").to_string())
        });
        assert_eq!(got, Ok(false));
        assert_eq!(answers.next(), None);
    }

    #[test]
    fn an_invalid_answer_does_not_accept_a_default() {
        let mut answers = ["maybe", "n"].into_iter();
        assert_eq!(
            TerminalAsk::confirm_with(question(Some(true)), |_| {
                Ok(answers.next().expect("another answer").to_string())
            }),
            Ok(false)
        );
        assert_eq!(answers.next(), None);
    }

    #[test]
    fn a_default_never_answers_without_a_terminal() {
        let ask = TerminalAsk { interactive: false };
        for default in [None, Some(true), Some(false)] {
            assert_eq!(
                ask.confirm(question(default)),
                Err(setup::unanswered(question(default)))
            );
        }
    }

    #[test]
    fn a_read_failure_is_returned_even_with_a_default() {
        assert_eq!(
            TerminalAsk::confirm_with(question(Some(true)), |_| Err("no answer".into())),
            Err("no answer".into())
        );
    }
}
