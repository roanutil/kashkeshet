use clap::{Args, Parser, Subcommand};
use kashkeshet::admission::{Admission, SharedKey};
use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::clock::{Clock, SystemClock};
use kashkeshet::config::{self, DEFAULT_BIND, InvalidSetting, LEGACY_DB_PATH, Limits};
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::names::OsEntropy;
use kashkeshet::provenance::{self, Build, ServerRun};
use kashkeshet::service::{DryRun, Manager, OsSystem, Spec};
use kashkeshet::settings::{self, Env, Platform, ServeOptions, ServeSettings};
use kashkeshet::shutdown::{Shutdown, StopSignals};
use kashkeshet::store::Db;
use kashkeshet::{Operator, Revoked};
use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A local server that lets AI agents talk to each other over MCP.
///
/// With no command, runs the server with the flags below, as it always has.
#[derive(Parser, Debug)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    flags: ServeFlags,
}

// Parsed once at startup, so the size of the largest variant costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum Command {
    /// Run the server, with settings from a config file as well as flags.
    ///
    /// Flags win over the file, and the file over the defaults. Without
    /// --db, the database is kashkeshet.db in the per-user data directory.
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
        /// directory. A server started without `serve` uses ./kashkeshet.db.
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
    #[arg(long, help = format!(
        "The SQLite database file. Created if missing [default: {LEGACY_DB_PATH}; \
         with `serve`, kashkeshet.db in the per-user data directory]"
    ))]
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
        "Message bodies longer than this many characters are cut in listings [default: {}]",
        Limits::DEFAULT.max_body_chars
    ))]
    max_body_chars: Option<usize>,
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
    /// A file holding the access key. When set, every /mcp request must send
    /// `Authorization: Bearer <key>`. By default /mcp needs no key.
    #[arg(long)]
    access_key_file: Option<PathBuf>,
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
            max_body_chars: self.max_body_chars,
            sends_per_minute: self.sends_per_minute,
            allow_origins: list(self.allow_origins),
            allow_hosts: list(self.allow_hosts),
            instructions: self.instructions,
            access_key_file: self.access_key_file,
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Color codes only for a terminal: a service's log file or the journal
    // would show them as raw escape characters.
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let cli = Cli::parse();
    let env = process_env();
    let result = match cli.command {
        None => {
            let settings = cli.flags.options().resolve(PathBuf::from(LEGACY_DB_PATH));
            serve(settings, &Sources::FLAGS_ONLY).await
        }
        Some(Command::Serve { config, flags }) => serve_from_config(&env, config, flags).await,
        Some(Command::Key {
            command: KeyCommand::Create { path, force, show },
        }) => create_key(&env, path, force, show),
        Some(Command::Agent {
            command,
            db,
            config,
        }) => agent(&env, command, db, config).await,
        Some(Command::Service { command }) => service(&env, command),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
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
    let settings = options.resolve(default_db);
    serve(settings, &sources).await
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
    /// Bare `kkt`: flags and defaults only.
    const FLAGS_ONLY: Sources = Sources {
        flags: ServeOptions::NONE,
        file: None,
    };

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
        access_key_file,
    } = settings;
    config::check_bind(bind).map_err(|e| sources.invalid(e))?;
    limits.check().map_err(|e| sources.invalid(e))?;
    let allowed_hosts = AllowedHosts::new(&allow_hosts).map_err(|e| sources.invalid(e))?;
    let instructions = match &instructions {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?,
        None => kashkeshet::DEFAULT_INSTRUCTIONS.to_string(),
    };
    let admission = match &access_key_file {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                format!(
                    "invalid setting: access_key_file could not be read from {}: {e}",
                    path.display()
                )
            })?;
            let key = SharedKey::new(&text, path.display().to_string())
                .map_err(|e| sources.invalid(e))?;
            Admission::SharedKey(key)
        }
        None => Admission::Open,
    };
    let config = AppConfig {
        admission,
        limits,
        allowed_origins: allow_origins,
        allowed_hosts,
        instructions,
        config_file: sources.file.as_ref().map(|(p, _)| p.display().to_string()),
    };
    Ok(Checked {
        db_path,
        bind,
        config,
    })
}

async fn serve(settings: ServeSettings, sources: &Sources) -> Result<(), String> {
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
            dry_run,
        } => {
            let plan = service_plan(env, config, db, access_key_file)?;
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
    let (passed_key, new_key) = match (
        &loaded.flags.access_key_file,
        &loaded.from_file.access_key_file,
    ) {
        (Some(key), _) => (Some(key.clone()), None),
        // Read from the config file each time the service starts.
        (None, Some(_)) => (None, None),
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
    // A key about to be made is good by construction; check any other.
    options.access_key_file = match &new_key {
        Some(_) => None,
        None => passed_key.clone().or(options.access_key_file),
    };
    let db_folder = db_in_effect.parent().map(Path::to_path_buf);
    check_settings(options.resolve(db_in_effect), &loaded.sources())?;

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
    Ok(ServicePlan {
        spec: Spec {
            program,
            args,
            dirs,
        },
        new_key,
    })
}
