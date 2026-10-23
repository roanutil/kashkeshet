use clap::Parser;
use kashkeshet::app::{self, AppConfig};
use kashkeshet::clock::SystemClock;
use kashkeshet::config::Limits;
use kashkeshet::db::Db;
use kashkeshet::mcp;
use kashkeshet::names::OsEntropy;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
    /// A file whose text is sent to every agent that connects, in place of
    /// the built-in instructions (agent-instructions.md).
    #[arg(long)]
    instructions: Option<PathBuf>,
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
    if !args.bind.ip().is_loopback() {
        return Err(format!(
            "refusing to listen on {}: only loopback addresses (such as 127.0.0.1) are allowed",
            args.bind
        ));
    }
    if args.default_limit == 0 || args.max_limit < args.default_limit {
        return Err("--default-limit must be at least 1 and no more than --max-limit".into());
    }
    let limits = Limits {
        max_response_bytes: args.max_response_bytes,
        default_limit: args.default_limit,
        max_limit: args.max_limit,
        max_body_chars: args.max_body_chars.max(1),
        sends_per_minute: args.sends_per_minute,
        ..Limits::DEFAULT
    };
    let instructions = match &args.instructions {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?,
        None => mcp::DEFAULT_INSTRUCTIONS.to_string(),
    };
    let db =
        Db::open(&args.db).map_err(|e| format!("could not open {}: {e}", args.db.display()))?;
    let shutdown = CancellationToken::new();
    let router = app::build(
        db,
        Arc::new(SystemClock),
        Arc::new(OsEntropy),
        AppConfig {
            limits,
            allowed_origins: args.allow_origins,
            instructions,
        },
        shutdown.clone(),
    );
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .map_err(|e| format!("could not listen on {}: {e}", args.bind))?;
    tracing::info!(
        "kashkeshet {} listening: web page http://{}/ , MCP http://{}/mcp , database {}",
        env!("CARGO_PKG_VERSION"),
        args.bind,
        args.bind,
        args.db.display()
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::error!("could not listen for Ctrl-C: {e}");
            }
            shutdown.cancel();
        })
        .await
        .map_err(|e| format!("server error: {e}"))
}
