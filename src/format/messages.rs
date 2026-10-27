//! Output for messages, search hits, and activity.

use super::fit::{Keep, Row, Trailer, one_line, short_time};
use super::{Format, Formatter};
use crate::rules::{ActivityRow, Message, MessagePage, Position, SearchHit, WaitResult};
use serde_json::{Value, json};

impl Formatter {
    pub fn sent(&self, f: Format, m: &Message) -> String {
        match f {
            Format::Text => format!("sent: message_id={}", m.id),
            Format::Json => json!({ "message_id": m.id }).to_string(),
        }
    }

    /// A page from `read`.
    pub fn read(
        &self,
        f: Format,
        page: &MessagePage,
        position: Position,
        max_body_chars: Option<usize>,
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
    pub fn waited(&self, f: Format, w: &WaitResult, max_body_chars: Option<usize>) -> String {
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

pub(super) fn message_json(m: &Message, body: &str, cut: bool) -> Value {
    json!({
        "id": m.id, "channel_id": m.channel_id, "channel_name": m.channel_name,
        "agent": m.agent, "created_at": m.created_at, "body": body, "cut": cut,
    })
}
