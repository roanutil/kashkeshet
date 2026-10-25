//! Output for registration and agent lists.

use super::fit::{Row, short_time};
use super::{Format, Formatter};
use crate::rules::{AgentInfo, Page, Registration};
use serde_json::json;

impl Formatter {
    pub fn registration(&self, f: Format, r: &Registration) -> String {
        match f {
            Format::Text => format!(
                "name: {}\ntoken: {}\nPass this token as `token` on every other call. Keep it private: \
                 anyone with it can act as you. If you lose it, call `register` again; you will get a new name.",
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
