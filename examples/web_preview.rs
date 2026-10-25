//! The web page with simulated data: no database, no MCP, no agents. For
//! working on the page's look and live behavior.
//!
//! ```text
//! cargo run --example web_preview -- --bind 127.0.0.1:7421 --seed 7 --rate 60
//! ```
//!
//! The same seed gives the same world and the same feed. Pass
//! `--access-key-file` to see the login page as well.

use clap::Parser;
use kashkeshet::admission::{Admission, Sessions, SharedKey, WebAdmission};
use kashkeshet::clock::SystemClock;
use kashkeshet::config::{self, Limits, PREVIEW_BIND, PREVIEW_SEED, PreviewSettings};
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::names::{OsEntropy, SeededEntropy};
use kashkeshet::shutdown::{Shutdown, StopSignals};
use kashkeshet::web::{self, Preview};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(about = "The kashkeshet web page with simulated data")]
struct Args {
    #[arg(long, help = format!("Address to listen on. Must be loopback [default: {PREVIEW_BIND}]"))]
    bind: Option<SocketAddr>,
    #[arg(long, help = format!("Seed for the simulated world and feed [default: {PREVIEW_SEED}]"))]
    seed: Option<u64>,
    #[arg(long, help = format!(
        "Steps of the simulated feed per minute; most are one message [default: {}]",
        PreviewSettings::DEFAULT.steps_per_minute
    ))]
    rate: Option<u32>,
    /// A host name to answer to besides loopback. Repeat for more.
    #[arg(long = "allow-host")]
    allow_hosts: Vec<String>,
    /// A file holding an access key, to preview the login page.
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
    let bind = args.bind.unwrap_or(PREVIEW_BIND);
    let seed = args.seed.unwrap_or(PREVIEW_SEED);
    let settings = PreviewSettings {
        steps_per_minute: args
            .rate
            .unwrap_or(PreviewSettings::DEFAULT.steps_per_minute),
    };
    config::check_bind(bind).map_err(|e| format!("invalid setting: {e}"))?;
    settings
        .check()
        .map_err(|e| format!("invalid setting: {e}"))?;
    let hosts =
        AllowedHosts::new(&args.allow_hosts).map_err(|e| format!("invalid setting: {e}"))?;
    let admission = match &args.access_key_file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let key = SharedKey::new(&text, path.display().to_string())
                .map_err(|e| format!("invalid setting: {e}"))?;
            Admission::SharedKey(key)
        }
        None => Admission::Open,
    };

    let clock = Arc::new(SystemClock);
    let shutdown = Shutdown::new();
    let gate = WebAdmission::new(
        Arc::new(admission),
        Sessions::new(
            clock.clone(),
            Arc::new(OsEntropy),
            Limits::DEFAULT.web_session_lifetime,
        ),
    );
    let preview = Arc::new(Preview::new(
        clock,
        Arc::new(SeededEntropy::new(seed)),
        settings.clone(),
        &shutdown,
    ));
    let feed = {
        let preview = preview.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move { preview.run(shutdown).await })
    };
    let router = web::router(preview, hosts, Arc::new(gate));

    let signals =
        StopSignals::listen().map_err(|e| format!("could not listen for shutdown signals: {e}"))?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("could not listen on {bind}: {e}"))?;
    tracing::info!(
        "kashkeshet web preview listening: http://{bind}/ (seed {seed}, {} steps a minute)",
        settings.steps_per_minute
    );
    let served = axum::serve(listener, router)
        .with_graceful_shutdown({
            let shutdown = shutdown.clone();
            async move {
                match signals.stop_on_signal(shutdown).await {
                    Ok(signal) => tracing::info!("received {signal}; shutting down"),
                    Err(e) => tracing::error!("could not listen for shutdown signals: {e}"),
                }
            }
        })
        .await
        .map_err(|e| format!("server error: {e}"));
    shutdown.stop();
    feed.await
        .map_err(|e| format!("the simulated feed failed: {e}"))?;
    served
}
