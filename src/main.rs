use clap::Parser;
use kashkeshet::admission::{Admission, SharedKey};
use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::clock::{Clock, SystemClock};
use kashkeshet::config::{self, Limits};
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::names::OsEntropy;
use kashkeshet::provenance::{self, Build, ServerRun};
use kashkeshet::shutdown::{Shutdown, StopSignals};
use kashkeshet::store::Db;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// A local server that lets AI agents talk to each other over MCP.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// The SQLite database file. Created if missing.
    #[arg(long, default_value = "./kashkeshet.db")]
    db: PathBuf,
    /// Address to listen on. Must be a loopback address.
    #[arg(long, default_value = "127.0.0.1:7420")]
    bind: SocketAddr,
    /// Largest response, in bytes, before it is cut short.
    #[arg(long, default_value_t = Limits::DEFAULT.max_response_bytes)]
    max_response_bytes: usize,
    /// Items returned when an agent gives no `limit`.
    #[arg(long, default_value_t = Limits::DEFAULT.default_limit)]
    default_limit: usize,
    /// Largest `limit` an agent may ask for.
    #[arg(long, default_value_t = Limits::DEFAULT.max_limit)]
    max_limit: usize,
    /// Message bodies longer than this many characters are cut in listings.
    #[arg(long, default_value_t = Limits::DEFAULT.max_body_chars)]
    max_body_chars: usize,
    /// Messages one agent may send per minute.
    #[arg(long, default_value_t = Limits::DEFAULT.sends_per_minute)]
    sends_per_minute: u32,
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

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    match run(Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    let limits = Limits {
        max_response_bytes: args.max_response_bytes,
        default_limit: args.default_limit,
        max_limit: args.max_limit,
        max_body_chars: args.max_body_chars,
        sends_per_minute: args.sends_per_minute,
        ..Limits::DEFAULT
    };
    config::check_bind(args.bind).map_err(|e| format!("invalid setting: {e}"))?;
    limits
        .check()
        .map_err(|e| format!("invalid setting: {e}"))?;
    let allowed_hosts =
        AllowedHosts::new(&args.allow_hosts).map_err(|e| format!("invalid setting: {e}"))?;
    let instructions = match &args.instructions {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?,
        None => kashkeshet::DEFAULT_INSTRUCTIONS.to_string(),
    };
    let admission = match &args.access_key_file {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                format!(
                    "invalid setting: access_key_file could not be read from {}: {e}",
                    path.display()
                )
            })?;
            let key = SharedKey::new(&text, path.display().to_string())
                .map_err(|e| format!("invalid setting: {e}"))?;
            Admission::SharedKey(key)
        }
        None => Admission::Open,
    };
    let db =
        Db::open(&args.db).map_err(|e| format!("could not open {}: {e}", args.db.display()))?;
    let config = AppConfig {
        admission,
        limits,
        allowed_origins: args.allow_origins,
        allowed_hosts,
        instructions,
    };
    let clock = Arc::new(SystemClock);
    let run = ServerRun::new(Build::CURRENT, &config);
    let access = match config.admission.key_source() {
        Some(file) => format!("access key from {file}"),
        None => "no access key".to_string(),
    };
    let run_id = provenance::record(&db, &run, clock.now())
        .await
        .map_err(|e| format!("could not record this server run: {e}"))?;
    let shutdown = Shutdown::new();
    let signals =
        StopSignals::listen().map_err(|e| format!("could not listen for shutdown signals: {e}"))?;
    let parts = Parts::new(db, clock, Arc::new(OsEntropy), &config.limits, run_id);
    let router = app::build(parts, config, shutdown.clone());
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .map_err(|e| format!("could not listen on {}: {e}", args.bind))?;
    tracing::info!(
        "kashkeshet {} (commit {}, run {run_id}) listening: web page http://{}/ , MCP http://{}/mcp ({access}), database {}",
        run.version,
        run.commit,
        args.bind,
        args.bind,
        args.db.display()
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
