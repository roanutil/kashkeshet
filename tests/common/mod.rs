//! Shared by the integration tests: start a server on a free port and connect
//! an MCP client to it.

// Each test file builds this module on its own, and none uses every helper.
#![allow(dead_code)]

use kashkeshet::admission::Admission;
use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::clock::{Clock, SystemClock};
use kashkeshet::config::{Limits, LogLevel};
use kashkeshet::hosts::AllowedHosts;
use kashkeshet::names::OsEntropy;
use kashkeshet::provenance::{self, Build, ServerRun};
use kashkeshet::shutdown::Shutdown;
use kashkeshet::store::Db;
use rmcp::model::ProtocolVersion;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientLifecycleMode, ClientServiceExt};
use std::sync::Arc;

pub struct Server {
    pub base: String,
    shutdown: Shutdown,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.stop();
    }
}

/// The settings `start` uses.
pub fn test_config() -> AppConfig {
    AppConfig {
        admission: Admission::Open,
        web_admission: Admission::Open,
        limits: Limits::DEFAULT,
        allowed_origins: vec!["http://localhost:6274".into()],
        allowed_hosts: AllowedHosts::new(&[]).expect("loopback hosts"),
        instructions: "test instructions: call register".into(),
        config_file: None,
        log_file: None,
        log_level: LogLevel::Info,
        dependency_log_level: LogLevel::Warn,
        allow_any_bind: false,
    }
}

pub async fn start() -> Server {
    start_with(test_config()).await
}

pub async fn start_with(config: AppConfig) -> Server {
    start_with_clock(config, Arc::new(SystemClock)).await
}

/// Starts a server whose time comes from `clock`.
pub async fn start_with_clock(config: AppConfig, clock: Arc<dyn Clock>) -> Server {
    let shutdown = Shutdown::new();
    let db = Db::open_in_memory().expect("db");
    let run = ServerRun::new(Build::CURRENT, &config);
    let run_id = provenance::record(&db, &run, clock.now())
        .await
        .expect("record run");
    let parts = Parts::new(db, clock, Arc::new(OsEntropy), &config.limits, run_id);
    let router = app::build(parts, config, shutdown.child());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let ct = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.stopped().await })
            .await
            .expect("serve");
    });
    Server {
        base: format!("http://{addr}"),
        shutdown,
    }
}

pub type Client = RunningService<RoleClient, ()>;

pub async fn connect(server: &Server, lifecycle: ClientLifecycleMode) -> Client {
    connect_with(
        StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", server.base)),
        lifecycle,
    )
    .await
}

async fn connect_with(
    config: StreamableHttpClientTransportConfig,
    lifecycle: ClientLifecycleMode,
) -> Client {
    let transport = StreamableHttpClientTransport::from_config(config);
    ().serve_with_lifecycle(transport, lifecycle)
        .await
        .expect("connect")
}

/// Connects as `connect_new` does, sending `Authorization: Bearer <key>`
/// with every request.
pub async fn connect_new_with_key(server: &Server, key: &str) -> Client {
    connect_with(
        StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", server.base))
            .auth_header(key),
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
}

pub async fn connect_new(server: &Server) -> Client {
    connect_new_at(&server.base).await
}

/// Connects as `connect_new` does, to a server at `base` (such as
/// `http://127.0.0.1:7420`) that this process did not start.
pub async fn connect_new_at(base: &str) -> Client {
    connect_with(
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/mcp")),
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
}
