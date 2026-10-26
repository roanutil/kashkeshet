//! Output for registration and agent lists.

use super::fit::{Row, one_line, short_time};
use super::{Format, Formatter};
use crate::rules::{AgentInfo, Page, Reclaimed, Registration};
use serde_json::json;

impl Formatter {
    pub fn registration(&self, f: Format, r: &Registration) -> String {
        match f {
            Format::Text => format!(
                "name: {}\ntoken: {}\nrecovery_code: {}\nPass the token as `token` on authenticated calls. \
                 The recovery code is shown only at registration; `reclaim` exchanges your name and \
                 recovery code for a new token for the same identity, invalidating the old token. \
                 Both the token and recovery code are private credentials.",
                r.name, r.token, r.recovery_code
            ),
            Format::Json => {
                json!({ "name": r.name, "token": r.token, "recovery_code": r.recovery_code })
                    .to_string()
            }
        }
    }

    pub fn reclaimed(&self, f: Format, r: &Reclaimed) -> String {
        match f {
            Format::Text => format!(
                "name: {}\ntoken: {}\nThe old token is invalid. Pass this token as `token` on authenticated calls.",
                r.name, r.token
            ),
            Format::Json => json!({ "name": r.name, "token": r.token }).to_string(),
        }
    }

    pub fn whoami(&self, f: Format, name: &str) -> String {
        match f {
            Format::Text => format!("name: {name}"),
            Format::Json => json!({ "name": name }).to_string(),
        }
    }

    pub fn description(&self, f: Format, a: &AgentInfo) -> String {
        match f {
            Format::Text => match &a.description {
                Some(d) => format!("name: {}\ndescription: {}", a.name, one_line(d)),
                None => format!("name: {}\ndescription cleared", a.name),
            },
            Format::Json => json!({ "name": a.name, "description": a.description }).to_string(),
        }
    }

    pub fn agents(&self, f: Format, page: &Page<AgentInfo>) -> String {
        let now = self.clock.now();
        let rows: Vec<Row> = page
            .items
            .iter()
            .map(|a| Row {
                text: with_description(
                    match &a.last_seen {
                        Some(seen) => format!("{}  last seen {}", a.name, short_time(seen, now)),
                        None => a.name.clone(),
                    },
                    a,
                ),
                json: json!({
                    "name": a.name,
                    "last_seen": a.last_seen,
                    "description": a.description,
                }),
                resume: format!("cursor={}", a.name),
            })
            .collect();
        let tail = page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        self.list(f, None, "agents", &rows, tail)
    }
}

/// `line`, followed by the agent's description on the same line if it has one.
pub(super) fn with_description(mut line: String, a: &AgentInfo) -> String {
    if let Some(d) = &a.description {
        line.push_str(" — ");
        line.push_str(&one_line(d));
    }
    line
}
