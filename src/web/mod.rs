//! The human's read-only view: pages for all channels, one channel, and one
//! agent, plus a stream of new messages for live updates.

use crate::config::WEB_KEEP_ALIVE;
use crate::events::Notice;
use crate::rules::{AgentInfo, ChannelInfo, Code, Message, RuleError, Rules};
use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use futures::{Stream, StreamExt};
use serde::Deserialize;
use std::convert::Infallible;

pub fn router(rules: Rules) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/channels/{id}", get(channel))
        .route("/agents/{name}", get(agent))
        .route("/events", get(events))
        .with_state(rules)
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    channels: Vec<ChannelInfo>,
    agents: Vec<AgentInfo>,
    recent: Vec<Message>,
}

#[derive(Template)]
#[template(path = "channel.html")]
struct ChannelPage {
    channel: ChannelInfo,
    members: Vec<String>,
    messages: Vec<Message>,
    older_before_id: Option<i64>,
    /// Live updates only make sense when showing the newest messages.
    live: bool,
}

#[derive(Template)]
#[template(path = "agent.html")]
struct AgentPage {
    agent: AgentInfo,
    channels: Vec<ChannelInfo>,
    message_count: i64,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    status: u16,
    message: String,
}

#[derive(Deserialize)]
struct ChannelParams {
    before_id: Option<i64>,
}

async fn index(State(rules): State<Rules>) -> Response {
    match rules.overview().await {
        Ok(o) => page(IndexPage {
            channels: o.channels,
            agents: o.agents,
            recent: o.recent,
        }),
        Err(e) => error_page(e),
    }
}

async fn channel(
    State(rules): State<Rules>,
    Path(id): Path<String>,
    Query(params): Query<ChannelParams>,
) -> Response {
    match rules.channel_view(id, params.before_id).await {
        Ok(v) => page(ChannelPage {
            channel: v.channel,
            members: v.members,
            messages: v.messages,
            older_before_id: v.older_before_id,
            live: params.before_id.is_none(),
        }),
        Err(e) => error_page(e),
    }
}

async fn agent(State(rules): State<Rules>, Path(name): Path<String>) -> Response {
    match rules.agent_view(name).await {
        Ok(v) => page(AgentPage {
            agent: v.agent,
            channels: v.channels,
            message_count: v.message_count,
        }),
        Err(e) => error_page(e),
    }
}

/// New messages as server-sent events. A `message` event carries one message
/// as JSON. A `lagged` event means this stream fell behind and missed some;
/// the page reloads.
async fn events(State(rules): State<Rules>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = rules.events().subscribe().into_stream().map(|notice| {
        Ok(match notice {
            Notice::Message(message) => match Event::default().event("message").json_data(&message)
            {
                Ok(event) => event,
                Err(e) => {
                    tracing::error!(error = %e, "could not encode message event");
                    Event::default().event("lagged").data("encode error")
                }
            },
            Notice::Missed(n) => Event::default().event("lagged").data(n.to_string()),
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

fn error_page(e: RuleError) -> Response {
    let status = match e.code {
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::InvalidArgument => StatusCode::BAD_REQUEST,
        _ => {
            tracing::error!(error = %e, "page failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    let body = ErrorPage {
        status: status.as_u16(),
        message: e.message,
    };
    match body.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(render_error) => {
            tracing::error!(error = %render_error, "could not render error page");
            (status, "error").into_response()
        }
    }
}
