//! Puts the parts together: the rules, the MCP endpoint at `/mcp`, and the
//! web page at `/`.

use crate::admission::{Admission, Credentials, Decision, Sessions, WebAdmission};
use crate::clock::Clock;
use crate::config::{Limits, LogLevel, MCP_MAX_REQUEST_BYTES, MCP_SSE_KEEP_ALIVE, MCP_SSE_RETRY};
use crate::events::Events;
use crate::format::Formatter;
use crate::hosts::AllowedHosts;
use crate::mcp::{self, ChatServer};
use crate::names::Entropy;
use crate::rules::{Deps, RegisterLimiter, Rules, SendLimiter};
use crate::shutdown::Shutdown;
use crate::store::Db;
use crate::web;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::sync::Arc;

pub struct AppConfig {
    /// Who may call `/mcp` at all. The web page is not covered.
    pub admission: Admission,
    /// Who may sign in to the operator web page.
    pub web_admission: Admission,
    pub limits: Limits,
    /// Browser origins allowed to call `/mcp`. Requests with any other
    /// `Origin` header are refused; requests without one (agents) pass.
    pub allowed_origins: Vec<String>,
    /// `Host` names `/mcp` and the web page answer to. Requests naming any
    /// other host are refused.
    pub allowed_hosts: AllowedHosts,
    /// Sent to every agent that connects. See [`crate::DEFAULT_INSTRUCTIONS`].
    pub instructions: String,
    /// The config file the settings were read from, if any, for the record
    /// of the server run.
    pub config_file: Option<String>,
    /// The file the log goes to, if not stdout, for the record of the
    /// server run.
    pub log_file: Option<String>,
    /// How much kkt's own code logs, for the record of the server run.
    pub log_level: LogLevel,
    /// How much the libraries kkt uses log, for the record of the server run.
    pub dependency_log_level: LogLevel,
    /// Whether the server may listen beyond loopback, for the record of the
    /// server run.
    pub allow_any_bind: bool,
}

/// The parts the server is built from. Callers outside the crate supply the
/// database, clock, entropy and run through [`Parts::new`], which makes the
/// rest; unit tests inside the crate can replace those too.
pub struct Parts {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub entropy: Arc<dyn Entropy>,
    pub(crate) events: Events,
    pub(crate) fmt: Formatter,
    pub(crate) waiting: crate::rules::Waiting,
    pub(crate) sends: SendLimiter,
    pub(crate) registrations: RegisterLimiter,
    /// This server run, from `provenance::record`.
    pub run_id: i64,
}

impl Parts {
    /// The usual parts around `db`, `clock` and `entropy`, sized by `limits`,
    /// for the server run `run_id`.
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
        limits: &Limits,
        run_id: i64,
    ) -> Parts {
        Parts {
            waiting: crate::rules::Waiting::default(),
            run_id,
            db,
            fmt: Formatter::new(limits.max_response_bytes, clock.clone()),
            clock,
            entropy,
            events: Events::new(),
            sends: SendLimiter::new(limits.sends_per_minute),
            registrations: RegisterLimiter::new(limits.registrations_per_minute),
        }
    }
}

pub fn build(parts: Parts, config: AppConfig, shutdown: Shutdown) -> Router {
    let fmt = parts.fmt;
    let tools = mcp::tools(&config.limits);
    let admission = Arc::new(config.admission);
    let sessions = Sessions::new(
        parts.clock.clone(),
        parts.entropy.clone(),
        config.limits.web_session_lifetime,
    );
    let web_admission = Arc::new(WebAdmission::new(Arc::new(config.web_admission), sessions));
    let rules = Rules::new(Deps {
        waiting: parts.waiting,
        db: parts.db,
        events: parts.events.until_stopped(shutdown.clone()),
        clock: parts.clock,
        entropy: parts.entropy,
        sends: parts.sends,
        registrations: parts.registrations,
        run_id: parts.run_id,
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
        mcp_transport(config.allowed_origins, &config.allowed_hosts, shutdown),
    );

    let mcp = Router::new()
        .nest_service("/mcp", mcp)
        .route_layer(middleware::from_fn_with_state(admission, admit));
    web::router(
        Arc::new(web::RulesSource::new(rules)),
        config.allowed_hosts,
        web_admission,
    )
    .merge(mcp)
}

/// Lets a request through to `/mcp` only if admission lets its caller in.
/// Refusals get `401` with `WWW-Authenticate: Bearer` and the reason.
async fn admit(State(admission): State<Arc<Admission>>, request: Request, next: Next) -> Response {
    let credentials =
        Credentials::from_authorization(request.headers().get(AUTHORIZATION).map(|v| v.as_bytes()));
    match admission.decide(&credentials) {
        Decision::Admitted(_) => next.run(request).await,
        Decision::Refused(refusal) => (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Bearer")],
            refusal.to_string(),
        )
            .into_response(),
    }
}

/// The MCP transport settings. Every option is set here by name, so a change
/// to the library's defaults cannot change the server's behavior.
fn mcp_transport(
    allowed_origins: Vec<String>,
    allowed_hosts: &AllowedHosts,
    shutdown: Shutdown,
) -> StreamableHttpServerConfig {
    let mut config = StreamableHttpServerConfig::default()
        // Serve every request on its own, with no session to resume.
        .with_legacy_session_mode(false)
        // Answer with event streams, never with plain JSON bodies.
        .with_json_response(false)
        .with_sse_keep_alive(Some(MCP_SSE_KEEP_ALIVE))
        .with_sse_retry(Some(MCP_SSE_RETRY))
        .with_allowed_hosts(allowed_hosts.entries())
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
