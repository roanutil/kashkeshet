//! Output for channels and their members.

use super::agents::with_description;
use super::fit::{Row, cut_body, one_line};
use super::{Format, Formatter};
use crate::rules::{ChannelInfo, Members, Page};
use serde_json::{Value, json};

impl Formatter {
    pub fn channels(&self, f: Format, page: &Page<ChannelInfo>, purpose_chars: usize) -> String {
        let rows: Vec<Row> = page
            .items
            .iter()
            .map(|c| Row {
                text: channel_line(c, purpose_chars),
                json: channel_json(c, Some(purpose_chars)),
                resume: format!("cursor={}", crate::rules::channel_cursor(c)),
            })
            .collect();
        let tail = page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        self.list(f, None, "channels", &rows, tail)
    }

    /// One channel in full.
    pub fn channel(&self, f: Format, c: &ChannelInfo) -> String {
        let out = match f {
            Format::Text => {
                let mut lines = vec![format!(
                    "#{} [{}] {}, created by {}",
                    c.name,
                    c.id,
                    if c.open { "open" } else { "closed" },
                    c.created_by
                )];
                if let Some(p) = &c.purpose {
                    lines.push(format!("purpose: {}", one_line(p)));
                }
                lines.push(format!(
                    "members ({}): {}{}",
                    c.member_count,
                    c.members_preview.join(", "),
                    if (c.members_preview.len() as i64) < c.member_count {
                        ", …"
                    } else {
                        ""
                    }
                ));
                lines.push(
                    if c.is_member {
                        "you are a member"
                    } else {
                        "you are not a member"
                    }
                    .into(),
                );
                lines.join("\n")
            }
            Format::Json => channel_json(c, None).to_string(),
        };
        self.clip(out)
    }

    pub fn members(&self, f: Format, m: &Members) -> String {
        let rows: Vec<Row> = m
            .page
            .items
            .iter()
            .map(|a| Row {
                text: with_description(a.name.clone(), a),
                json: json!({ "name": a.name, "description": a.description }),
                resume: format!("cursor={}", a.name),
            })
            .collect();
        let tail = m.page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        let header = (
            format!("{}: {} members", m.channel_id, m.total),
            json!({ "channel_id": m.channel_id, "total": m.total }),
        );
        self.list(f, Some(header), "members", &rows, tail)
    }
}

fn channel_line(c: &ChannelInfo, purpose_chars: usize) -> String {
    let mut parts = vec![
        format!("[{}] #{}", c.id, c.name),
        (if c.open { "open" } else { "closed" }).to_string(),
    ];
    let more = if (c.members_preview.len() as i64) < c.member_count {
        ", …"
    } else {
        ""
    };
    parts.push(format!(
        "{} members: {}{}",
        c.member_count,
        c.members_preview.join(", "),
        more
    ));
    if c.is_member {
        parts.push("you're a member".into());
    }
    if let Some(n) = c.new_messages {
        parts.push(format!("{n} new"));
    }
    let mut line = parts.join(" · ");
    if let Some(p) = &c.purpose {
        line.push_str(" — ");
        line.push_str(&cut_body(&one_line(p), purpose_chars).0);
    }
    line
}

fn channel_json(c: &ChannelInfo, purpose_chars: Option<usize>) -> Value {
    let purpose = match (c.purpose.as_deref(), purpose_chars) {
        (Some(p), Some(max)) => Some(cut_body(p, max).0),
        (p, _) => p.map(str::to_string),
    };
    json!({
        "id": c.id, "name": c.name, "purpose": purpose, "open": c.open,
        "created_by": c.created_by, "member_count": c.member_count,
        "members_preview": c.members_preview, "is_member": c.is_member,
        "new_messages": c.new_messages,
    })
}
