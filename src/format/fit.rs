//! The size-cap machinery every response shares: lists and message lists
//! that keep as many items as fit, cutting long bodies, and clipping.

use super::messages::message_json;
use super::{Format, Formatter};
use crate::clock;
use crate::rules::Message;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

/// Which end of a message list to keep when it must be shortened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keep {
    Oldest,
    Newest,
}

impl Formatter {
    /// A list of rows with an optional header line and a closing `more:` line.
    /// If the whole list does not fit, it keeps as many rows as fit and points
    /// at the first row left out.
    pub(super) fn list(
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
            self.list_page(f, header.as_ref(), key, &rows[..k], more)
        };
        fit(rows.len(), self.max_bytes, render)
    }

    /// Whether `row` alone, with a `more:` line after it, fits in a response.
    pub(super) fn row_fits(
        &self,
        f: Format,
        header: Option<&(String, Value)>,
        key: &str,
        row: &Row,
    ) -> bool {
        let page = self.list_page(
            f,
            header,
            key,
            std::slice::from_ref(row),
            Some(row.resume.clone()),
        );
        page.len() <= self.max_bytes
    }

    /// Renders all of `rows`, whatever their size.
    fn list_page(
        &self,
        f: Format,
        header: Option<&(String, Value)>,
        key: &str,
        rows: &[Row],
        more: Option<String>,
    ) -> String {
        match f {
            Format::Text => {
                let mut lines = Vec::new();
                if let Some((h, _)) = header {
                    lines.push(h.clone());
                }
                if rows.is_empty() {
                    lines.push("(none)".into());
                }
                lines.extend(rows.iter().map(|r| r.text.clone()));
                if let Some(m) = more {
                    lines.push(format!("more: {m}"));
                }
                lines.join("\n")
            }
            Format::Json => {
                let mut obj = match header {
                    Some((_, Value::Object(map))) => map.clone(),
                    _ => serde_json::Map::new(),
                };
                obj.insert(
                    key.into(),
                    Value::Array(rows.iter().map(|r| r.json.clone()).collect()),
                );
                obj.insert("more".into(), more.map_or(Value::Null, Value::String));
                Value::Object(obj).to_string()
            }
        }
    }

    pub(super) fn messages(
        &self,
        f: Format,
        msgs: &[Message],
        keep: Keep,
        max_body_chars: Option<usize>,
        trailer: impl Fn(&[Message], bool) -> Trailer,
    ) -> String {
        let now = self.clock.now();
        let render = |k: usize, max_body_chars: Option<usize>| {
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
        let out = fit(msgs.len(), self.max_bytes, |k| render(k, max_body_chars));
        if msgs.is_empty() || render(1, max_body_chars).len() <= self.max_bytes {
            return out;
        }
        // Even one message does not fit whole: show the first one in line,
        // with as much of its body as fits, and the usual cut marker.
        let longest = msgs
            .iter()
            .map(|m| m.body.chars().count())
            .max()
            .unwrap_or(0);
        let start = max_body_chars.map_or(longest, |n| n.min(longest));
        let (mut low, mut high) = (0, start);
        while low < high {
            let mid = (low + high).div_ceil(2);
            if render(1, Some(mid)).len() <= self.max_bytes {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        let shortened = render(1, Some(low));
        if shortened.len() <= self.max_bytes {
            shortened
        } else {
            out
        }
    }

    /// Shortens a single-item response that is over the cap.
    pub(super) fn clip(&self, out: String) -> String {
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

pub(super) struct Row {
    pub(super) text: String,
    pub(super) json: Value,
    /// How to continue if this is the last row shown.
    pub(super) resume: String,
}

pub(super) struct Trailer {
    pub(super) more: Option<String>,
    pub(super) waiting: Option<i64>,
}

/// Renders with as many of the `n` items as fit in `cap` bytes. Tries all of
/// them first, then one fewer, and so on.
pub(super) fn fit(n: usize, cap: usize, render: impl Fn(usize) -> String) -> String {
    for k in (1..=n).rev() {
        let out = render(k);
        if out.len() <= cap {
            return out;
        }
    }
    render(0)
}

/// The first `max_chars` characters of `body`, and whether anything was cut.
/// With no `max_chars`, the body is whole.
pub(super) fn cut_body(body: &str, max_chars: Option<usize>) -> (String, bool) {
    let cut_at = max_chars.and_then(|n| body.char_indices().nth(n));
    match cut_at {
        Some((end, _)) => (format!("{}…", &body[..end]), true),
        None => (body.to_string(), false),
    }
}

/// `text` cut at a character boundary to at most `max_bytes` bytes, ending
/// in `…` if anything was cut.
pub(super) fn cut_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `HH:MM` for times today, otherwise `YYYY-MM-DD HH:MM` (all UTC).
pub(super) fn short_time(stored: &str, now: DateTime<Utc>) -> String {
    match clock::from_text(stored) {
        Some(t) if t.date_naive() == now.date_naive() => t.format("%H:%M").to_string(),
        Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        None => stored.to_string(),
    }
}
