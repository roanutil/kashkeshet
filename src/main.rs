use clap::{Args, Parser, Subcommand};
use kashkeshet::admission::{Admission, SharedKey};
use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::clock::{Clock, SystemClock};
use kashkeshet::config::{self, DEFAULT_BIND, InvalidSetting, LEGACY_DB_PATH, Limits};
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::names::OsEntropy;
use kashkeshet::provenance::{self, Build, ServerRun};
use kashkeshet::settings::{self, Env, Platform, ServeOptions, ServeSettings};
use kashkeshet::shutdown::{Shutdown, StopSignals};
use kashkeshet::store::Db;
use kashkeshet::{Operator, Revoked};
use std::io::Write;
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
    tracing_subscriber::fmt().with_target(false).init();
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
    let (config_file, from_file) = read_config(env, config)?;
    let flags = flags.options();
    let sources = Sources {
        file: config_file.map(|path| (path, from_file.clone())),
        flags: flags.clone(),
    };
    let options = flags.over(from_file);
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

async fn serve(settings: ServeSettings, sources: &Sources) -> Result<(), String> {
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
    let db =
        Db::open(&db_path).map_err(|e| format!("could not open {}: {e}", db_path.display()))?;
    let config = AppConfig {
        admission,
        limits,
        allowed_origins: allow_origins,
        allowed_hosts,
        instructions,
        config_file: sources.file.as_ref().map(|(p, _)| p.display().to_string()),
    };
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
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let key = SharedKey::generate(&OsEntropy);
    write_key_file(&path, &key, force)?;
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
