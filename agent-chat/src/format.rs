//! Turns rule results into small responses: plain-text lines by default, JSON
//! on request. Long message bodies are cut, and no response grows past the
//! size cap. When something is left out, the response says how to get it.

use crate::clock::{self, Clock};
use crate::rules::{
    ActivityRow, AgentInfo, ChannelInfo, Members, Message, MessagePage, Page, Position,
    Registration, RuleError, SearchHit, WaitResult,
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Compact lines of text. The default.
    #[default]
    Text,
    /// A JSON object.
    Json,
}

/// Which end of a message list to keep when it must be shortened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    Oldest,
    Newest,
}

#[derive(Clone)]
pub struct Formatter {
    max_bytes: usize,
    clock: Arc<dyn Clock>,
}

impl Formatter {
    pub fn new(max_response_bytes: usize, clock: Arc<dyn Clock>) -> Formatter {
        Formatter {
            max_bytes: max_response_bytes,
            clock,
        }
    }

    pub fn error(&self, f: Format, e: &RuleError) -> String {
        match f {
            Format::Text => format!("error: {}: {}", e.code.as_str(), e.message),
            Format::Json => {
                json!({ "error": { "code": e.code, "message": e.message } }).to_string()
            }
        }
    }

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

    pub fn ok(&self, f: Format) -> String {
        match f {
            Format::Text => "ok".to_string(),
            Format::Json => json!({ "ok": true }).to_string(),
        }
    }

    pub fn sent(&self, f: Format, m: &Message) -> String {
        match f {
            Format::Text => format!("sent: message_id={}", m.id),
            Format::Json => json!({ "message_id": m.id }).to_string(),
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
            .map(|name| Row {
                text: name.clone(),
                json: json!(name),
                resume: format!("cursor={name}"),
            })
            .collect();
        let tail = m.page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        let header = (
            format!("{}: {} members", m.channel_id, m.total),
            json!({ "channel_id": m.channel_id, "total": m.total }),
        );
        self.list(f, Some(header), "members", &rows, tail)
    }

    /// A page from `read`.
    pub fn read(
        &self,
        f: Format,
        page: &MessagePage,
        position: Position,
        max_body_chars: usize,
    ) -> String {
        let keep = match position {
            Position::After(_) => Keep::Oldest,
            Position::Latest | Position::Before(_) => Keep::Newest,
        };
        let db_more = page.more.map(position_text);
        self.messages(f, &page.messages, keep, max_body_chars, |kept, cut| {
            let more = if cut {
                resume_after_cut(kept, keep)
            } else {
                db_more.clone()
            };
            Trailer {
                more,
                waiting: None,
            }
        })
    }

    /// The result of `wait_for_messages`.
    pub fn waited(&self, f: Format, w: &WaitResult, max_body_chars: usize) -> String {
        self.messages(
            f,
            &w.messages,
            Keep::Oldest,
            max_body_chars,
            |kept, _cut| {
                let waiting = w.more_waiting + (w.messages.len() - kept.len()) as i64;
                let more = match (waiting > 0, kept.last()) {
                    (true, Some(last)) => Some(format!("after_id={}", last.id)),
                    _ => None,
                };
                Trailer {
                    more,
                    waiting: Some(waiting),
                }
            },
        )
    }

    /// One message, in full. Not cut and not held to the size cap: the body
    /// limit already bounds it, and this is how an agent gets the whole text.
    pub fn message(&self, f: Format, m: &Message) -> String {
        match f {
            Format::Text => format!(
                "#{} [{}]\n[{} {} {}] {}",
                m.channel_name, m.channel_id, m.id, m.created_at, m.agent, m.body
            ),
            Format::Json => message_json(m, &m.body, false).to_string(),
        }
    }

    pub fn search(&self, f: Format, hits: &[SearchHit]) -> String {
        let now = self.clock.now();
        let rows: Vec<Row> = hits
            .iter()
            .map(|h| Row {
                text: format!(
                    "[{} {} {} #{} {}] {}",
                    h.message_id,
                    short_time(&h.created_at, now),
                    h.agent,
                    h.channel_name,
                    h.channel_id,
                    one_line(&h.excerpt)
                ),
                json: json!({
                    "message_id": h.message_id, "channel_id": h.channel_id,
                    "channel_name": h.channel_name, "agent": h.agent,
                    "created_at": h.created_at, "excerpt": h.excerpt,
                }),
                resume: "a narrower query or a smaller limit".into(),
            })
            .collect();
        self.list(f, None, "hits", &rows, None)
    }

    pub fn activity(&self, f: Format, rows: &[ActivityRow]) -> String {
        let latest = rows.iter().map(|r| r.latest_id).max();
        let items: Vec<Row> = rows
            .iter()
            .map(|r| Row {
                text: format!(
                    "#{} [{}] {} new, latest {}",
                    r.channel_name, r.channel_id, r.new_messages, r.latest_id
                ),
                json: json!({
                    "channel_id": r.channel_id, "channel_name": r.channel_name,
                    "new_messages": r.new_messages, "latest_id": r.latest_id,
                }),
                resume: "read the channels above, then call activity again".into(),
            })
            .collect();
        let header = latest.map(|id| {
            (
                format!("latest message id: {id}"),
                json!({ "latest_id": id }),
            )
        });
        self.list(f, header, "channels", &items, None)
    }

    // ---- shared machinery ----

    /// A list of rows with an optional header line and a closing `more:` line.
    /// If the whole list does not fit, it keeps as many rows as fit and points
    /// at the first row left out.
    fn list(
        &self,
        f: Format,
        header: Option<(String, Value)>,
        key: &str,
        rows: &[Row],
        tail: Option<String>,
    ) -> String {
        let render = |k: usize| {
            let more = if k < rows.len() {
                // Each row's `resume` says how to continue after it.
                Some(match k {
                    0 => "the first item is larger than the response size limit".to_string(),
                    _ => rows[k - 1].resume.clone(),
                })
            } else {
                tail.clone()
            };
            match f {
                Format::Text => {
                    let mut lines = Vec::new();
                    if let Some((h, _)) = &header {
                        lines.push(h.clone());
                    }
                    if rows.is_empty() {
                        lines.push("(none)".into());
                    }
                    lines.extend(rows[..k].iter().map(|r| r.text.clone()));
                    if let Some(m) = more {
                        lines.push(format!("more: {m}"));
                    }
                    lines.join("\n")
                }
                Format::Json => {
                    let mut obj = match &header {
                        Some((_, Value::Object(map))) => map.clone(),
                        _ => serde_json::Map::new(),
                    };
                    obj.insert(
                        key.into(),
                        Value::Array(rows[..k].iter().map(|r| r.json.clone()).collect()),
                    );
                    obj.insert("more".into(), more.map_or(Value::Null, Value::String));
                    Value::Object(obj).to_string()
                }
            }
        };
        fit(rows.len(), self.max_bytes, render)
    }

    fn messages(
        &self,
        f: Format,
        msgs: &[Message],
        keep: Keep,
        max_body_chars: usize,
        trailer: impl Fn(&[Message], bool) -> Trailer,
    ) -> String {
        let now = self.clock.now();
        let render = |k: usize| {
            let kept = match keep {
                Keep::Oldest => &msgs[..k],
                Keep::Newest => &msgs[msgs.len() - k..],
            };
            let cut = k < msgs.len();
            let mut t = trailer(kept, cut);
            if cut && kept.is_empty() {
                let skipped = match keep {
                    Keep::Oldest => &msgs[0],
                    Keep::Newest => &msgs[msgs.len() - 1],
                };
                t.more = Some(format!(
                    "message {id} is too large to show here; read it with get_message {id}, or ask for a smaller max_body_chars",
                    id = skipped.id
                ));
            }
            match f {
                Format::Text => {
                    let mut lines = Vec::new();
                    let mut channel: Option<&str> = None;
                    for m in kept {
                        if channel != Some(m.channel_id.as_str()) {
                            lines.push(format!("#{} [{}]", m.channel_name, m.channel_id));
                            channel = Some(&m.channel_id);
                        }
                        let (body, was_cut) = cut_body(&m.body, max_body_chars);
                        let body = body.replace('\n', "\n    ");
                        let note = if was_cut {
                            format!("  (cut; get_message {})", m.id)
                        } else {
                            String::new()
                        };
                        lines.push(format!(
                            "[{} {} {}] {}{}",
                            m.id,
                            short_time(&m.created_at, now),
                            m.agent,
                            body,
                            note
                        ));
                    }
                    if kept.is_empty() {
                        lines.push("(no messages)".into());
                    }
                    if let Some(w) = t.waiting.filter(|w| *w > 0) {
                        lines.push(format!("waiting: {w} more"));
                    }
                    if let Some(m) = t.more {
                        lines.push(format!("more: {m}"));
                    }
                    lines.join("\n")
                }
                Format::Json => {
                    let items: Vec<Value> = kept
                        .iter()
                        .map(|m| {
                            let (body, was_cut) = cut_body(&m.body, max_body_chars);
                            message_json(m, &body, was_cut)
                        })
                        .collect();
                    let mut obj = json!({ "messages": items, "more": t.more });
                    if let Some(w) = t.waiting {
                        obj["more_waiting"] = json!(w);
                    }
                    obj.to_string()
                }
            }
        };
        fit(msgs.len(), self.max_bytes, render)
    }

    /// Shortens a single-item response that is over the cap.
    fn clip(&self, out: String) -> String {
        if out.len() <= self.max_bytes {
            return out;
        }
        let mut end = self.max_bytes.saturating_sub(16);
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}… (cut)", &out[..end])
    }
}

struct Row {
    text: String,
    json: Value,
    /// How to continue if this is the last row shown.
    resume: String,
}

struct Trailer {
    more: Option<String>,
    waiting: Option<i64>,
}

/// Renders with as many of the `n` items as fit in `cap` bytes. Tries all of
/// them first, then one fewer, and so on.
fn fit(n: usize, cap: usize, render: impl Fn(usize) -> String) -> String {
    for k in (1..=n).rev() {
        let out = render(k);
        if out.len() <= cap {
            return out;
        }
    }
    render(0)
}

fn resume_after_cut(kept: &[Message], keep: Keep) -> Option<String> {
    match keep {
        Keep::Oldest => kept.last().map(|m| format!("after_id={}", m.id)),
        Keep::Newest => kept.first().map(|m| format!("before_id={}", m.id)),
    }
}

fn position_text(p: Position) -> String {
    match p {
        Position::After(id) => format!("after_id={id}"),
        Position::Before(id) => format!("before_id={id}"),
        Position::Latest => "latest".into(),
    }
}

/// The first `max_chars` characters of `body`, and whether anything was cut.
fn cut_body(body: &str, max_chars: usize) -> (String, bool) {
    match body.char_indices().nth(max_chars) {
        Some((end, _)) => (format!("{}…", &body[..end]), true),
        None => (body.to_string(), false),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `HH:MM` for times today, otherwise `YYYY-MM-DD HH:MM` (all UTC).
fn short_time(stored: &str, now: DateTime<Utc>) -> String {
    match clock::from_text(stored) {
        Some(t) if t.date_naive() == now.date_naive() => t.format("%H:%M").to_string(),
        Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        None => stored.to_string(),
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

fn message_json(m: &Message, body: &str, cut: bool) -> Value {
    json!({
        "id": m.id, "channel_id": m.channel_id, "channel_name": m.channel_name,
        "agent": m.agent, "created_at": m.created_at, "body": body, "cut": cut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::testing::FixedClock;
    use chrono::TimeZone;

    fn formatter(max: usize) -> Formatter {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 15, 0, 0).unwrap();
        Formatter::new(max, Arc::new(FixedClock::new(now)))
    }

    fn msg(id: i64, body: &str) -> Message {
        Message {
            id,
            channel_id: "ch_7k2m9q4x".into(),
            channel_name: "general".into(),
            agent: "amber-falcon".into(),
            body: body.into(),
            created_at: "2026-10-23T14:02:00Z".into(),
        }
    }

    #[test]
    fn text_messages_look_like_the_plan() {
        let page = MessagePage {
            messages: vec![
                msg(1201, "can someone take the parser?"),
                msg(1207, &"x".repeat(600)),
            ],
            more: Some(Position::Before(1201)),
        };
        let out = formatter(8192).read(Format::Text, &page, Position::Latest, 500);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "#general [ch_7k2m9q4x]");
        assert_eq!(
            lines[1],
            "[1201 14:02 amber-falcon] can someone take the parser?"
        );
        assert!(lines[2].ends_with("…  (cut; get_message 1207)"));
        assert_eq!(lines[3], "more: before_id=1201");
    }

    #[test]
    fn size_cap_keeps_newest_and_points_back() {
        let msgs: Vec<Message> = (1..=50).map(|i| msg(i, &"y".repeat(300))).collect();
        let page = MessagePage {
            messages: msgs,
            more: None,
        };
        let out = formatter(2000).read(Format::Text, &page, Position::Latest, 500);
        assert!(out.len() <= 2000);
        let first_id: i64 = out.lines().nth(1).unwrap()[1..]
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(out.contains(&format!("more: before_id={first_id}")));
        assert!(out.contains("[50 "));
    }

    #[test]
    fn size_cap_keeps_oldest_when_reading_forward() {
        let msgs: Vec<Message> = (1..=50).map(|i| msg(i, &"y".repeat(300))).collect();
        let page = MessagePage {
            messages: msgs,
            more: None,
        };
        let out = formatter(2000).read(Format::Json, &page, Position::After(0), 500);
        assert!(out.len() <= 2000);
        let v: Value = serde_json::from_str(&out).unwrap();
        let shown = v["messages"].as_array().unwrap();
        let last = shown.last().unwrap()["id"].as_i64().unwrap();
        assert_eq!(shown[0]["id"], 1);
        assert_eq!(v["more"], format!("after_id={last}"));
    }

    #[test]
    fn a_message_too_big_for_any_response_points_to_get_message() {
        let page = MessagePage {
            messages: vec![msg(9, &"z".repeat(5000))],
            more: None,
        };
        let out = formatter(1000).read(Format::Text, &page, Position::Latest, 8000);
        assert!(out.len() <= 1000);
        assert!(out.contains("get_message 9"));
    }

    #[test]
    fn lists_cut_at_cap_with_cursor() {
        let page = Page {
            items: (0..100)
                .map(|i| AgentInfo {
                    name: format!("agent-{i:03}"),
                    created_at: "2026-10-23T14:00:00Z".into(),
                    last_seen: None,
                })
                .collect(),
            next_cursor: None,
        };
        let out = formatter(200).agents(Format::Text, &page);
        assert!(out.len() <= 200);
        let last_shown = out.lines().rev().nth(1).unwrap();
        assert_eq!(
            out.lines().last().unwrap(),
            format!("more: cursor={last_shown}")
        );
    }

    #[test]
    fn short_times() {
        let now = Utc.with_ymd_and_hms(2026, 10, 23, 15, 0, 0).unwrap();
        assert_eq!(short_time("2026-10-23T09:05:00Z", now), "09:05");
        assert_eq!(short_time("2026-10-22T09:05:00Z", now), "2026-10-22 09:05");
    }

    #[test]
    fn cut_counts_characters() {
        assert_eq!(cut_body("héllo", 2), ("hé…".to_string(), true));
        assert_eq!(cut_body("hi", 2), ("hi".to_string(), false));
    }
}
