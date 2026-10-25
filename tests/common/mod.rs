//! Shared by the integration tests: start a server on a free port and connect
//! an MCP client to it.

use kashkeshet::app::{self, AppConfig, Parts};
use kashkeshet::clock::SystemClock;
use kashkeshet::config::Limits;
use kashkeshet::db::Db;
use kashkeshet::names::OsEntropy;
use kashkeshet::shutdown::Shutdown;
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
        limits: Limits::DEFAULT,
        allowed_origins: vec!["http://localhost:6274".into()],
        instructions: "test instructions: call register".into(),
    }
}

pub async fn start() -> Server {
    start_with(test_config()).await
}

pub async fn start_with(config: AppConfig) -> Server {
    let shutdown = Shutdown::new();
    let parts = Parts::new(
        Db::open_in_memory().expect("db"),
        Arc::new(SystemClock),
        Arc::new(OsEntropy),
        &config.limits,
    );
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
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", server.base)),
    );
    ().serve_with_lifecycle(transport, lifecycle)
        .await
        .expect("connect")
}

pub async fn connect_new(server: &Server) -> Client {
    connect(
        server,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
}
