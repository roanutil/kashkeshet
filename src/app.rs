//! Puts the parts together: the rules, the MCP endpoint at `/mcp`, and the
//! web page at `/`.

use crate::clock::Clock;
use crate::config::{
    Limits, MCP_ALLOWED_HOSTS, MCP_MAX_REQUEST_BYTES, MCP_SSE_KEEP_ALIVE, MCP_SSE_RETRY,
};
use crate::db::Db;
use crate::events::Events;
use crate::format::Formatter;
use crate::mcp::{self, ChatServer};
use crate::names::Entropy;
use crate::rules::{Deps, Rules, SendLimiter};
use crate::shutdown::Shutdown;
use crate::web;
use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::sync::Arc;

pub struct AppConfig {
    pub limits: Limits,
    /// Browser origins allowed to call `/mcp`. Requests with any other
    /// `Origin` header are refused; requests without one (agents) pass.
    pub allowed_origins: Vec<String>,
    /// Sent to every agent that connects. See `mcp::DEFAULT_INSTRUCTIONS`.
    pub instructions: String,
}

/// The parts the server is built from. The caller makes them, so tests can
/// supply their own.
pub struct Parts {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub entropy: Arc<dyn Entropy>,
    pub events: Events,
    pub fmt: Formatter,
    pub sends: SendLimiter,
}

impl Parts {
    /// The usual parts around `db`, `clock` and `entropy`, sized by `limits`.
    pub fn new(db: Db, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, limits: &Limits) -> Parts {
        Parts {
            db,
            fmt: Formatter::new(limits.max_response_bytes, clock.clone()),
            clock,
            entropy,
            events: Events::new(),
            sends: SendLimiter::new(limits.sends_per_minute),
        }
    }
}

pub fn build(parts: Parts, config: AppConfig, shutdown: Shutdown) -> Router {
    let fmt = parts.fmt;
    let tools = mcp::tools(&config.limits);
    let rules = Rules::new(Deps {
        db: parts.db,
        events: parts.events,
        clock: parts.clock,
        entropy: parts.entropy,
        sends: parts.sends,
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
                tools.clone(),
            ))
        },
        // A session manager with no event store: there are no sessions, and
        // a broken event stream is not resumed. That is the only state
        // `default()` can build, so there is nothing further to set.
        Arc::new(NeverSessionManager::default()),
        mcp_transport(config.allowed_origins, shutdown),
    );

    web::router(rules).nest_service("/mcp", mcp)
}

/// The MCP transport settings. Every option is set here by name, so a change
/// to the library's defaults cannot change the server's behavior.
fn mcp_transport(allowed_origins: Vec<String>, shutdown: Shutdown) -> StreamableHttpServerConfig {
    let mut config = StreamableHttpServerConfig::default()
        // Serve every request on its own, with no session to resume.
        .with_legacy_session_mode(false)
        // Answer with event streams, never with plain JSON bodies.
        .with_json_response(false)
        .with_sse_keep_alive(Some(MCP_SSE_KEEP_ALIVE))
        .with_sse_retry(Some(MCP_SSE_RETRY))
        .with_allowed_hosts(MCP_ALLOWED_HOSTS.iter().copied())
        // Refuse any browser origin not listed, even when the list is empty.
        .with_allowed_origins(allowed_origins)
        .enforce_origin_validation()
        .with_max_request_body_bytes(MCP_MAX_REQUEST_BYTES)
        // Older clients do not send per-request protocol metadata; accept them.
        .with_stateless_protocol_metadata_required(false)
        .with_cancellation_token(shutdown.cancellation_token());
    config.session_store = None;
    config
}
