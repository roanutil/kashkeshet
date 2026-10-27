use clap::{Args, Parser, Subcommand};
use kashkeshet::admission::{Admission, SharedKey};
use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::clients::{self, Client, Setup};
use kashkeshet::clock::{Clock, SystemClock};
use kashkeshet::config::{
    self, DEFAULT_BIND, DEFAULT_DEPENDENCY_LOG_LEVEL, DEFAULT_LOG_LEVEL, InvalidSetting,
    LOG_OWN_TARGETS, Limits, LogLevel, LogPolicy,
};
use kashkeshet::diagnostics;
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::logs::RotatingFile;
use kashkeshet::names::OsEntropy;
use kashkeshet::provenance::{self, Build, ServerRun};
use kashkeshet::service::{DryRun, Manager, OsSystem, Spec};
use kashkeshet::settings::{self, AccessKey, Env, Platform, ServeOptions, ServeSettings};
use kashkeshet::shutdown::{Shutdown, StopSignals};
use kashkeshet::store::Db;
use kashkeshet::{Operator, Revoked};
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
    /// Run `kkt serve` in the background: launchd on macOS, systemd on Linux.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
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
    /// A browser origin allowed to call /mcp, e.g. http://localhost:6274 for
    /// MCP Inspector. Repeat for more. By default no browser origin is allowed.
    #[arg(long = "allow-origin")]
    allow_origins: Vec<String>,
    /// A host name the server answers to besides localhost, 127.0.0.1 and
    /// ::1, e.g. host.docker.internal or a Tailscale name. Add `:port` to
    /// allow only that port. Repeat for more.
    #[arg(long = "allow-host")]
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
            max_response_bytes: self.max_response_bytes,
            default_limit: self.default_limit,
            max_limit: self.max_limit,
            sends_per_minute: self.sends_per_minute,
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
    let client = match client {
        "claude-code" => Client::ClaudeCode,
        "codex" => Client::Codex,
        "agy" => Client::Agy,
        "opencode" => Client::OpenCode,
        _ => return Err("unknown client".into()),
    };
    let connection = client_connection(env, flags, true)?;
    let executable = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("could not find kkt's path: {e}"))?;
    let setup = Setup {
        env,
        url: &connection.url,
        executable: &executable,
        key_file: connection.key_file.as_deref(),
    };
    println!("{}", clients::configure(client, &setup, write)?);
    Ok(())
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

async fn doctor(env: &Env, flags: ConnectionFlags) -> Result<(), String> {
    let connection = client_connection(env, flags, false)?;
    let mut checks = diagnostics::inspect(&connection.url, connection.key_file.as_deref()).await?;
    match Manager::for_this_os(env).and_then(|manager| manager.state(&OsSystem)) {
        Ok(state) if !state.installed => checks.push(diagnostics::Check::ok(
            "service",
            "not installed; optional when running `kkt serve` in a terminal",
        )),
        Ok(state) if state.running => {
            checks.push(diagnostics::Check::ok("service", "installed and running"))
        }
        Ok(_) => checks.push(diagnostics::Check::problem(
            "service",
            "installed but not running",
            "run `kkt service start`",
        )),
        Err(error) => checks.push(diagnostics::Check::problem(
            "service",
            error.to_string(),
            "check `kkt service status` and your user service manager",
        )),
    }
    let mut all_ok = true;
    for check in checks {
        all_ok &= check.ok;
        println!(
            "{}: {}: {}",
            if check.ok { "ok" } else { "problem" },
            check.name,
            check.detail
        );
        if let Some(fix) = check.fix {
            println!("  fix: {fix}");
        }
    }
    if all_ok {
        Ok(())
    } else {
        Err("doctor found problems; see the fixes above".into())
    }
}

/// Runs one command.
async fn run(env: &Env, command: Command) -> Result<(), String> {
    match command {
        Command::Serve { config, flags } => serve_from_config(env, config, flags).await,
        Command::Key {
            command: KeyCommand::Create { path, force, show },
        } => create_key(env, path, force, show),
        Command::Agent {
            command,
            db,
            config,
        } => agent(env, command, db, config).await,
        Command::Service { command } => service(env, command),
        Command::Key {
            command: KeyCommand::Header { key_file, json },
        } => key_header(env, key_file, json),
        Command::Connect {
            client,
            connection,
            write,
        } => connect(env, &client, connection, write),
        Command::Doctor { connection } => doctor(env, connection).await,
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
    let path = match db {
        Some(path) => path,
        None => match read_config(env, config)?.1.db {
            Some(path) => path,
            None => env.default_db(Platform::CURRENT).ok_or(
                "no home directory to find the database in: set HOME or pass --db".to_string(),
            )?,
        },
    };
    if !path.exists() {
        return Err(format!(
            "no database at {}; pass --db with the server's database file",
            path.display()
        ));
    }
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
        limits,
        allow_origins,
        allow_hosts,
        instructions,
        access_key,
        log_file,
        log_level,
        dependency_log_level,
    } = settings;
    config::check_bind(bind).map_err(|e| sources.invalid(e))?;
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
    let db =
        Db::open(&db_path).map_err(|e| format!("could not open {}: {e}", db_path.display()))?;
    let clock = Arc::new(SystemClock);
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
    let mut args = vec!["serve".to_string()];
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
