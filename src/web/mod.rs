//! The human's read-only view: pages for all channels, one channel, and one
//! agent, plus a stream of new messages for live updates. Requests naming a
//! host that is not allowed are refused, as they are on `/mcp`. When the
//! gate requires it, viewers log in first (`sign_in`).
//!
//! The pages read only through their own contract (`source`): the server
//! supplies it over the rules (`real`), and the preview from simulated data
//! (`preview`), so the page can run and be worked on without a server.

mod preview;
mod real;
mod sign_in;
mod source;

pub use preview::Preview;
pub use real::RulesSource;
pub use source::{
    Admit, Agent, AgentView, Channel, ChannelView, Gate, LiveEvent, Message, Overview, ServerRun,
    SignIn, Source, SourceError,
};

use crate::config::WEB_KEEP_ALIVE;
use crate::hosts::AllowedHosts;
use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;

/// What the pages need: their data, and whether to offer a logout button.
#[derive(Clone)]
struct Pages {
    source: Arc<dyn Source>,
    sign_out: bool,
}

/// The web page, reading from `source`. Every route checks the `Host`
/// against `hosts`. When `gate` requires sign-in, every page and the event
/// stream need a session, and `/login` and `/logout` are added.
pub fn router(source: Arc<dyn Source>, hosts: AllowedHosts, gate: Arc<dyn Gate>) -> Router {
    let sign_in_required = gate.sign_in_required();
    let pages = Router::new()
        .route("/", get(index))
        .route("/channels/{id}", get(channel))
        .route("/agents/{name}", get(agent))
        .route("/events", get(events))
        .with_state(Pages {
            source,
            sign_out: sign_in_required,
        });
    let site = if sign_in_required {
        let sign_in = Router::new()
            .route(
                sign_in::LOGIN_PATH,
                get(sign_in::login_form).post(sign_in::login),
            )
            .route("/logout", post(sign_in::logout))
            .with_state(gate.clone());
        pages
            .route_layer(middleware::from_fn_with_state(
                gate,
                sign_in::require_session,
            ))
            .merge(sign_in)
    } else {
        pages
    };
    site.layer(middleware::from_fn_with_state(hosts, check_host))
}

/// Serves the request only if its `Host` is allowed. Without this, a site
/// whose name was pointed at this machine (DNS rebinding) could read every
/// page from the human's browser.
async fn check_host(State(hosts): State<AllowedHosts>, request: Request, next: Next) -> Response {
    // HTTP/2 carries the host in the URI instead of a `Host` header.
    let host = match request.headers().get(header::HOST) {
        Some(value) => value.to_str().ok().map(str::to_string),
        None => request.uri().authority().map(|a| a.to_string()),
    };
    let allowed = host.as_deref().is_some_and(|h| hosts.allows(h));
    if allowed {
        next.run(request).await
    } else {
        tracing::warn!(host = ?host, "refused a web request naming a host that is not allowed");
        (
            StatusCode::FORBIDDEN,
            "Forbidden: Host header is not allowed",
        )
            .into_response()
    }
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    channels: Vec<Channel>,
    agents: Vec<Agent>,
    recent: Vec<Message>,
    runs: Vec<ServerRun>,
    sign_out: bool,
}

#[derive(Template)]
#[template(path = "channel.html")]
struct ChannelPage {
    channel: Channel,
    members: Vec<String>,
    messages: Vec<Message>,
    older_before_id: Option<i64>,
    /// Live updates only make sense when showing the newest messages.
    live: bool,
    sign_out: bool,
}

#[derive(Template)]
#[template(path = "agent.html")]
struct AgentPage {
    agent: Agent,
    channels: Vec<Channel>,
    message_count: i64,
    sign_out: bool,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    status: u16,
    message: String,
    sign_out: bool,
}

#[derive(Deserialize)]
struct ChannelParams {
    before_id: Option<i64>,
}

async fn index(State(pages): State<Pages>) -> Response {
    match pages.source.overview().await {
        Ok(o) => page(IndexPage {
            channels: o.channels,
            agents: o.agents,
            recent: o.recent,
            runs: o.runs,
            sign_out: pages.sign_out,
        }),
        Err(e) => error_page(e, pages.sign_out),
    }
}

async fn channel(
    State(pages): State<Pages>,
    Path(id): Path<String>,
    Query(params): Query<ChannelParams>,
) -> Response {
    match pages.source.channel(id, params.before_id).await {
        Ok(v) => page(ChannelPage {
            channel: v.channel,
            members: v.members,
            messages: v.messages,
            older_before_id: v.older_before_id,
            live: params.before_id.is_none(),
            sign_out: pages.sign_out,
        }),
        Err(e) => error_page(e, pages.sign_out),
    }
}

async fn agent(State(pages): State<Pages>, Path(name): Path<String>) -> Response {
    match pages.source.agent(name).await {
        Ok(v) => page(AgentPage {
            agent: v.agent,
            channels: v.channels,
            message_count: v.message_count,
            sign_out: pages.sign_out,
        }),
        Err(e) => error_page(e, pages.sign_out),
    }
}

/// New messages as server-sent events. A `message` event carries one message
/// as JSON. A `lagged` event means this stream fell behind and missed some;
/// the page reloads.
async fn events(State(pages): State<Pages>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = pages.source.live().map(|event| {
        Ok(match event {
            LiveEvent::Message(message) => {
                match Event::default().event("message").json_data(&message) {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::error!(error = %e, "could not encode message event");
                        Event::default().event("lagged").data("encode error")
                    }
                }
            }
            LiveEvent::Missed(n) => Event::default().event("lagged").data(n.to_string()),
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(WEB_KEEP_ALIVE).text("keep-alive"))
}

mod filters {
    /// A stored time, `2026-10-23T14:02:11Z`, shown as `2026-10-23 14:02:11`.
    #[askama::filter_fn]
    pub fn when<T: std::fmt::Display>(stored: T, _: &dyn askama::Values) -> askama::Result<String> {
        Ok(stored.to_string().replace('T', " ").replace('Z', ""))
    }

    /// The first 12 characters of a hash, as git shows a short commit. Any
    /// `-dirty` mark is kept.
    #[askama::filter_fn]
    pub fn short<T: std::fmt::Display>(hash: T, _: &dyn askama::Values) -> askama::Result<String> {
        let text = hash.to_string();
        let (hash, dirty) = match text.strip_suffix("-dirty") {
            Some(h) => (h, "-dirty"),
            None => (text.as_str(), ""),
        };
        Ok(format!(
            "{}{dirty}",
            hash.chars().take(12).collect::<String>()
        ))
    }
}

fn page(t: impl Template) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "could not render page");
            (StatusCode::INTERNAL_SERVER_ERROR, "could not render page").into_response()
        }
    }
}

fn error_page(e: SourceError, sign_out: bool) -> Response {
    let (status, message) = match e {
        SourceError::NotFound(message) => (StatusCode::NOT_FOUND, message),
        SourceError::Invalid(message) => (StatusCode::BAD_REQUEST, message),
        SourceError::Failed(message) => {
            tracing::error!(error = %message, "page failed");
            (StatusCode::INTERNAL_SERVER_ERROR, message)
        }
    };
    let body = ErrorPage {
        status: status.as_u16(),
        message,
        sign_out,
    };
    match body.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(render_error) => {
            tracing::error!(error = %render_error, "could not render error page");
            (status, "error").into_response()
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod filter_tests {
    // Askama finds custom filters through a `filters` module in scope.
    use super::filters;
    use askama::Template;

    #[derive(Template)]
    #[template(source = "{{ h|short }}", ext = "txt")]
    struct Short<'a> {
        h: &'a str,
    }

    fn render(h: &str) -> String {
        Short { h }.render().expect("render")
    }

    #[test]
    fn short_keeps_twelve_characters_and_the_dirty_mark() {
        assert_eq!(render("0123456789abcdef0123"), "0123456789ab");
        assert_eq!(render("0123456789abcdef0123-dirty"), "0123456789ab-dirty");
        assert_eq!(render("unknown"), "unknown");
    }
}
