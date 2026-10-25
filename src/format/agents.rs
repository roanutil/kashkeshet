//! Output for registration and agent lists.

use super::fit::{Row, short_time};
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

    pub fn agents(&self, f: Format, page: &Page<AgentInfo>) -> String {
        let now = self.clock.now();
        let rows: Vec<Row> = page
            .items
            .iter()
            .map(|a| Row {
                text: match &a.last_seen {
                    Some(seen) => format!("{}  last seen {}", a.name, short_time(seen, now)),
                    None => a.name.clone(),
                },
                json: json!({ "name": a.name, "last_seen": a.last_seen }),
                resume: format!("cursor={}", a.name),
            })
            .collect();
        let tail = page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        self.list(f, None, "agents", &rows, tail)
    }
}
