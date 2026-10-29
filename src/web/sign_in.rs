//! Signing in to the web page when a web key is set: the login form,
//! the session cookie, logout, and the check in front of every page. The
//! decisions are made by the [`Gate`]; this file only reads and writes
//! HTTP.

use super::change::{Change, NoFields};
use super::page;
use super::source::{Admit, Gate, SignIn};
use crate::config::WEB_SESSION_COOKIE;
use crate::hosts;
use askama::Template;
use axum::Form;
use axum::extract::{Request, State};
use axum::http::header::{COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Where the login form lives.
pub const LOGIN_PATH: &str = "/login";
/// The live-update stream, which gets `401` rather than a redirect.
const EVENTS_PATH: &str = "/events";

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    error: Option<String>,
    sign_out: bool,
    /// Signing in needs no form token: there is no session yet to tie one
    /// to. The same-origin check and the key itself guard it.
    form_token: String,
}

#[derive(Deserialize)]
pub struct LoginForm {
    key: String,
}

/// Lets a request through only with a live session. Pages without one are
/// sent to the login form; the event stream gets `401`.
pub async fn require_session(
    State(gate): State<Arc<dyn Gate>>,
    request: Request,
    next: Next,
) -> Response {
    let session_id = session_cookie(request.headers());
    match gate.admits(session_id.as_deref()) {
        Admit::Yes => next.run(request).await,
        Admit::No(reason) if request.uri().path() == EVENTS_PATH => {
            (StatusCode::UNAUTHORIZED, reason).into_response()
        }
        Admit::No(_) => Redirect::to(LOGIN_PATH).into_response(),
    }
}

pub async fn login_form() -> Response {
    page(LoginPage {
        error: None,
        sign_out: false,
        form_token: String::new(),
    })
}

/// Checks the key from the form. On success, sets the session cookie and
/// goes to the front page.
pub async fn login(
    State(gate): State<Arc<dyn Gate>>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    match gate.sign_in(&form.key) {
        SignIn::Started {
            session_id,
            lifetime,
        } => (
            StatusCode::SEE_OTHER,
            [
                (LOCATION, "/".to_string()),
                (SET_COOKIE, cookie(&session_id, lifetime, secure(&headers))),
            ],
        )
            .into_response(),
        SignIn::Refused(reason) => (
            StatusCode::UNAUTHORIZED,
            page(LoginPage {
                error: Some(reason),
                sign_out: false,
                form_token: String::new(),
            }),
        )
            .into_response(),
    }
}

/// Ends the session, clears the cookie, and goes to the login form. Ending
/// a live session is a change, so it needs the form token; a session that
/// has already ended only has its cookie cleared.
pub async fn logout(
    State(gate): State<Arc<dyn Gate>>,
    headers: HeaderMap,
    change: Result<Change<NoFields>, Response>,
) -> Response {
    if !same_origin(&headers) {
        return cross_site();
    }
    let session = session_cookie(&headers);
    if gate.admits(session.as_deref()) == Admit::Yes {
        if let Err(refusal) = change {
            return refusal;
        }
        if let Some(id) = &session {
            gate.sign_out(id);
        }
    }
    (
        StatusCode::SEE_OTHER,
        [
            (LOCATION, LOGIN_PATH.to_string()),
            (SET_COOKIE, cookie("", Duration::ZERO, secure(&headers))),
        ],
    )
        .into_response()
}

fn cross_site() -> Response {
    (
        StatusCode::FORBIDDEN,
        "refused: the form was sent from another site",
    )
        .into_response()
}

/// The session cookie, lasting `lifetime`, marked `Secure` if `secure`.
fn cookie(session_id: &str, lifetime: Duration, secure: bool) -> String {
    let secure = if secure { " Secure;" } else { "" };
    format!(
        "{WEB_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict;{secure} Max-Age={}",
        lifetime.as_secs()
    )
}

/// Whether the cookie for this request is marked `Secure`. It is, unless the
/// request was sent to a loopback name: some browsers (Safari) drop `Secure`
/// cookies on `http://localhost`, and loopback traffic never leaves the
/// machine. Other names, such as a tailnet name behind `tailscale serve`'s
/// HTTPS, keep it. A request without a `Host` header keeps it too.
fn secure(headers: &HeaderMap) -> bool {
    !headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(hosts::is_loopback)
}

/// The session ID from the request's cookies, if it sent one.
pub(super) fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == WEB_SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
}

/// Whether a form post came from this site. A browser sends `Origin` with
/// every form post; it must name the host the request was sent to. A
/// request without `Origin` is not from a browser form and is let through.
pub(super) fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let host = headers.get(HOST).and_then(|h| h.to_str().ok());
    let origin_host = origin.to_str().ok().and_then(|o| {
        o.strip_prefix("http://")
            .or_else(|| o.strip_prefix("https://"))
    });
    match (origin_host, host) {
        (Some(o), Some(h)) => o.eq_ignore_ascii_case(h),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(axum::http::HeaderName, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(name.clone(), HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let h = headers(&[(COOKIE, "a=1; kkt_session=abc ; b=2")]);
        assert_eq!(session_cookie(&h).as_deref(), Some("abc"));
        let h = headers(&[(COOKIE, "a=1"), (COOKIE, "kkt_session=xyz")]);
        assert_eq!(session_cookie(&h).as_deref(), Some("xyz"));
        let h = headers(&[(COOKIE, "kkt_sessionx=1; xkkt_session=2")]);
        assert_eq!(session_cookie(&h), None);
        assert_eq!(session_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn form_posts_must_come_from_this_host() {
        assert!(same_origin(&HeaderMap::new()));
        assert!(same_origin(&headers(&[
            (ORIGIN, "http://localhost:7420"),
            (HOST, "localhost:7420"),
        ])));
        assert!(same_origin(&headers(&[
            (ORIGIN, "https://Mac.tail1234.ts.net"),
            (HOST, "mac.tail1234.ts.net"),
        ])));
        for (origin, host) in [
            ("http://evil.example", "localhost:7420"),
            ("http://localhost:7421", "localhost:7420"),
            ("null", "localhost:7420"),
        ] {
            assert!(
                !same_origin(&headers(&[(ORIGIN, origin), (HOST, host)])),
                "{origin}"
            );
        }
        assert!(!same_origin(&headers(&[(ORIGIN, "http://localhost:7420")])));
    }

    #[test]
    fn the_cookie_is_locked_down() {
        assert_eq!(
            cookie("abc", Duration::from_secs(60), true),
            "kkt_session=abc; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age=60"
        );
        assert_eq!(
            cookie("abc", Duration::from_secs(60), false),
            "kkt_session=abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=60"
        );
    }

    #[test]
    fn only_loopback_hosts_drop_secure() {
        assert!(!secure(&headers(&[(HOST, "localhost:7420")])));
        assert!(!secure(&headers(&[(HOST, "127.0.0.1:7420")])));
        assert!(secure(&headers(&[(HOST, "mac.tail1234.ts.net")])));
        assert!(secure(&headers(&[(HOST, "host.docker.internal:7420")])));
        assert!(secure(&HeaderMap::new()));
    }
}
