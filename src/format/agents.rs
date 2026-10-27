//! Output for registration and agent lists.

use super::fit::{Row, cut_bytes, one_line, short_time};
use super::{Format, Formatter};
use crate::rules::{AgentInfo, OperatorInstructions, Page, Reclaimed, Registration};
use serde_json::{Value, json};

impl Formatter {
    pub fn registration(&self, f: Format, r: &Registration) -> String {
        match f {
            Format::Text => {
                let text = format!(
                    "name: {}\ntoken: {}\nrecovery_code: {}\nPass the token as `token` on authenticated calls. \
                     The recovery code is shown only at registration; `reclaim` exchanges your name and \
                     recovery code for a new token for the same identity, invalidating the old token. \
                     Both the token and recovery code are private credentials.",
                    r.name, r.token, r.recovery_code,
                );
                self.with_operator_instructions(&text, r.operator_instructions.as_ref())
            }
            Format::Json => {
                json!({ "name": r.name, "token": r.token, "recovery_code": r.recovery_code, "operator_instructions": r.operator_instructions })
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

    pub fn whoami(
        &self,
        f: Format,
        name: &str,
        instructions: Option<&OperatorInstructions>,
    ) -> String {
        match f {
            Format::Text => self.with_operator_instructions(&format!("name: {name}"), instructions),
            Format::Json => {
                json!({ "name": name, "operator_instructions": instructions }).to_string()
            }
        }
    }

    /// The operator text is separate from the server's own instructions.
    pub fn with_operator_instructions(
        &self,
        base: &str,
        instructions: Option<&OperatorInstructions>,
    ) -> String {
        match instructions {
            None => base.into(),
            Some(v) => format!(
                "{base}\n\nFrom this server's operator (version {}):\n{}",
                v.version, v.text
            ),
        }
    }

    /// Acknowledges a new description by its length, without repeating it,
    /// so the reply is short whatever the description holds.
    pub fn description(&self, f: Format, a: &AgentInfo) -> String {
        let chars = a.description.as_deref().map_or(0, |d| d.chars().count());
        match f {
            Format::Text => match chars {
                0 => format!("name: {}\ndescription cleared", a.name),
                n => format!("name: {}\ndescription set ({n} characters)", a.name),
            },
            Format::Json => json!({ "name": a.name, "description_chars": chars }).to_string(),
        }
    }

    /// One row per agent, built by `build` from the agent and the description
    /// to show. Descriptions are shown whole unless one row alone would not
    /// fit in a response; then that row's description is cut to fit.
    pub(super) fn described_rows(
        &self,
        f: Format,
        header: Option<&(String, Value)>,
        key: &str,
        agents: &[AgentInfo],
        build: impl Fn(&AgentInfo, Option<&str>) -> Row,
    ) -> Vec<Row> {
        let fits = |row: &Row| self.row_fits(f, header, key, row);
        agents
            .iter()
            .map(|a| {
                // A single agent can wait on more channels than fit in a row.
                // Keep the agent visible and state how many channel IDs were omitted.
                let render = |shown: &AgentInfo, description: Option<&str>| {
                    let mut row = build(shown, description);
                    let omitted = a.waiting_channels.len() - shown.waiting_channels.len();
                    if omitted > 0 {
                        row.text
                            .push_str(&format!(" ({omitted} more waiting channels)"));
                        row.json["waiting_channels_more"] = json!(omitted);
                    }
                    row
                };
                let mut shown = a.clone();
                // Reserve the full description and the row continuation while
                // reducing IDs; if no IDs remain, the description itself can be cut.
                while !fits(&render(&shown, a.description.as_deref()))
                    && !shown.waiting_channels.is_empty()
                {
                    shown.waiting_channels.pop();
                }
                let whole = render(&shown, a.description.as_deref());
                let Some(d) = a.description.as_deref().filter(|_| !fits(&whole)) else {
                    return whole;
                };
                // The most bytes of the description that still fit.
                let (mut lo, mut hi) = (0, d.len());
                while lo < hi {
                    let mid = (lo + hi).div_ceil(2);
                    if fits(&render(&shown, Some(&cut_bytes(d, mid)))) {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                render(&shown, Some(&cut_bytes(d, lo)))
            })
            .collect()
    }

    pub fn agents(&self, f: Format, page: &Page<AgentInfo>) -> String {
        let now = self.clock.now();
        let rows = self.described_rows(f, None, "agents", &page.items, |a, description| Row {
            text: with_description(
                format!(
                    "{}; {}",
                    match &a.last_seen {
                        Some(seen) => format!("{}  last call {}", a.name, short_time(seen, now)),
                        None => a.name.clone(),
                    },
                    listening_text(a)
                ),
                description,
            ),
            json: json!({
                "name": a.name,
                "last_seen": a.last_seen,
                "listening": a.listening,
                "waiting_channels": a.waiting_channels,
                "description": description,
            }),
            resume: format!("cursor={}", a.name),
        });
        let tail = page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        self.list(f, None, "agents", &rows, tail)
    }
}

/// `line`, followed by `description` on the same line if there is one.
pub(super) fn with_description(mut line: String, description: Option<&str>) -> String {
    if let Some(d) = description {
        line.push_str(" — ");
        line.push_str(&one_line(d));
    }
    line
}

pub(super) fn listening_text(a: &AgentInfo) -> String {
    if a.listening {
        format!("listening on {}", a.waiting_channels.join(", "))
    } else {
        "not listening".into()
    }
}
