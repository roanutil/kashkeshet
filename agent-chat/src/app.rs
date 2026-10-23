//! Puts the parts together: the rules, the MCP endpoint at `/mcp`, and the
//! web page at `/`.

use crate::clock::Clock;
use crate::config::Limits;
use crate::db::Db;
use crate::events::Events;
use crate::format::Formatter;
use crate::mcp::ChatServer;
use crate::names::Entropy;
use crate::rules::{Deps, Rules};
use crate::web;
use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct AppConfig {
    pub limits: Limits,
    /// Browser origins allowed to call `/mcp`. Requests with any other
    /// `Origin` header are refused; requests without one (agents) pass.
    pub allowed_origins: Vec<String>,
    /// Sent to every agent that connects. See `mcp::DEFAULT_INSTRUCTIONS`.
    pub instructions: String,
}

pub fn build(
    db: Db,
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    config: AppConfig,
    shutdown: CancellationToken,
) -> Router {
    let fmt = Formatter::new(config.limits.max_response_bytes, clock.clone());
    let rules = Rules::new(Deps {
        db,
        events: Events::new(),
        clock,
        entropy,
        limits: config.limits,
    });

    let mcp_rules = rules.clone();
    let instructions: Arc<str> = config.instructions.into();
    let mcp = StreamableHttpService::new(
        move || {
            Ok(ChatServer::new(
                mcp_rules.clone(),
                fmt.clone(),
                instructions.clone(),
            ))
        },
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_allowed_origins(config.allowed_origins)
            .enforce_origin_validation()
            .with_cancellation_token(shutdown),
    );

    web::router(rules).nest_service("/mcp", mcp)
}
