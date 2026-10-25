use super::*;
use crate::events::NewMessage;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast::error::RecvError;

impl Rules {
    /// Posts a message to a channel `me` is in. Returns the stored message.
    pub async fn send(&self, me: &Agent, channel_id: String, body: String) -> Result<Message> {
        let max = self.limits().max_body_bytes;
        if body.trim().is_empty() {
            return Err(RuleError::invalid("message body must not be empty"));
        }
        if body.len() > max {
            return Err(RuleError::new(
                Code::BodyTooLong,
                format!("message body is {} bytes; the limit is {max}", body.len()),
            ));
        }
        if auth::contains_token(&body) {
            return Err(RuleError::new(
                Code::ContainsSecret,
                "message contains something shaped like a token (`act_` plus 26 characters). \
                 Tokens must never be shared; remove it and send again.",
            ));
        }
        let me = me.name.clone();
        let now = self.inner.deps.clock.now();
        let rules = self.clone();
        // Set once the rate limiter has counted this send, so a failed write
        // can give it back.
        let charged = Arc::new(AtomicBool::new(false));
        let charged_in_tx = charged.clone();
        let sender = me.clone();
        let written = self
            .db()
            .write(move |tx| {
                let me = sender;
                let row = member_channel(tx, &me, &channel_id)?;
                if !rules.inner.sends.allow(&me, now) {
                    return Err(RuleError::new(
                        Code::RateLimited,
                        format!(
                            "you can send at most {} messages a minute; wait and try again",
                            rules.limits().sends_per_minute
                        ),
                    ));
                }
                charged_in_tx.store(true, Ordering::SeqCst);
                let created_at = clock::to_text(now);
                tx.execute(
                    "INSERT INTO messages (channel_id, agent, body, created_at) VALUES (?, ?, ?, ?)",
                    [&row.id, &me, &body, &created_at],
                )?;
                Ok(Message {
                    id: tx.last_insert_rowid(),
                    channel_id: row.id,
                    channel_name: row.name,
                    agent: me,
                    body,
                    created_at,
                })
            })
            .await;
        let message = match written {
            Ok(message) => message,
            Err(e) => {
                if charged.load(Ordering::SeqCst) {
                    self.inner.sends.refund(&me);
                }
                return Err(e);
            }
        };
        self.events().publish(NewMessage {
            id: message.id,
            channel_id: message.channel_id.clone(),
            channel_name: message.channel_name.clone(),
            agent: message.agent.clone(),
            body: message.body.clone(),
            created_at: message.created_at.clone(),
        });
        Ok(message)
    }

    /// Messages in a channel `me` is in.
    pub async fn read(&self, me: &Agent, q: ReadQuery) -> Result<MessagePage> {
        let me = me.name.clone();
        self.db()
            .read(move |conn| {
                member_channel(conn, &me, &q.channel_id)?;
                read_channel(conn, &q)
            })
            .await
    }

    /// One message in full.
    pub async fn get_message(&self, me: &Agent, id: i64) -> Result<Message> {
        let me = me.name.clone();
        self.db()
            .read(move |conn| {
                let message =
                    load_message(conn, id)?.ok_or_else(|| RuleError::message_not_found(id))?;
                let (row, member) = match visible_channel(conn, &me, &message.channel_id) {
                    Ok(found) => found,
                    // A message in a hidden channel does not exist, as far as `me` can tell.
                    Err(e) if e.code == Code::NotFound => {
                        return Err(RuleError::message_not_found(id));
                    }
                    Err(e) => return Err(e),
                };
                if !member {
                    return Err(RuleError::not_member(&row.id));
                }
                Ok(message)
            })
            .await
    }

    /// Full-text search over the channels `me` is in, newest first.
    pub async fn search(&self, me: &Agent, q: SearchQuery) -> Result<Vec<SearchHit>> {
        let fts = fts_query(&q.query)?;
        let me = me.name.clone();
        self.db()
            .read(move |conn| {
                if let Some(ch) = &q.channel_id {
                    member_channel(conn, &me, ch)?;
                }
                let mut stmt = conn.prepare_cached(
                    "SELECT m.id, m.channel_id, c.name, m.agent, m.created_at,
                            snippet(messages_fts, 0, '**', '**', '…', 12)
                     FROM messages_fts
                     JOIN messages m ON m.id = messages_fts.rowid
                     JOIN channels c ON c.id = m.channel_id
                     WHERE messages_fts MATCH :q
                       AND EXISTS (SELECT 1 FROM memberships x
                                   WHERE x.channel_id = m.channel_id AND x.agent = :me)
                       AND (:ch IS NULL OR m.channel_id = :ch)
                       AND (:from IS NULL OR m.agent = :from)
                     ORDER BY m.id DESC
                     LIMIT :limit",
                )?;
                let hits = stmt
                    .query_map(
                        named_params! {
                            ":q": fts,
                            ":me": me,
                            ":ch": q.channel_id,
                            ":from": q.from_agent,
                            ":limit": q.limit as i64,
                        },
                        |r| {
                            Ok(SearchHit {
                                message_id: r.get(0)?,
                                channel_id: r.get(1)?,
                                channel_name: r.get(2)?,
                                agent: r.get(3)?,
                                created_at: r.get(4)?,
                                excerpt: r.get(5)?,
                            })
                        },
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(hits)
            })
            .await
    }

    /// The channels `me` is in that have messages newer than `after_id`,
    /// busiest-most-recently first.
    pub async fn activity(&self, me: &Agent, after_id: i64) -> Result<Vec<ActivityRow>> {
        let me = me.name.clone();
        self.db()
            .read(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT m.channel_id, c.name, count(*), max(m.id)
                     FROM messages m JOIN channels c ON c.id = m.channel_id
                     WHERE m.id > :after
                       AND m.channel_id IN (SELECT channel_id FROM memberships WHERE agent = :me)
                     GROUP BY m.channel_id
                     ORDER BY max(m.id) DESC",
                )?;
                let rows = stmt
                    .query_map(named_params! { ":after": after_id, ":me": me }, |r| {
                        Ok(ActivityRow {
                            channel_id: r.get(0)?,
                            channel_name: r.get(1)?,
                            new_messages: r.get(2)?,
                            latest_id: r.get(3)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
    }

    /// Returns messages newer than `after_id` in `me`'s channels (or the listed
    /// ones). If there are none, waits until one arrives or the time runs out.
    pub async fn wait_for_messages(&self, me: &Agent, q: WaitQuery) -> Result<WaitResult> {
        let filter = match &q.channel_ids {
            None => None,
            Some(ids) if ids.is_empty() => {
                return Err(RuleError::invalid(
                    "`channel_ids` must list at least one channel, or be left out",
                ));
            }
            Some(ids) => {
                let who = me.name.clone();
                let to_check = ids.clone();
                self.db()
                    .read(move |conn| {
                        for id in &to_check {
                            member_channel(conn, &who, id)?;
                        }
                        Ok::<_, RuleError>(())
                    })
                    .await?;
                Some(
                    serde_json::to_string(ids)
                        .map_err(|e| RuleError::new(Code::Internal, e.to_string()))?,
                )
            }
        };

        // Subscribe before the first look, so a message sent in between is not missed.
        let mut notices = self.events().subscribe();
        let clock = &self.inner.deps.clock;
        let timeout = chrono::Duration::from_std(q.timeout)
            .map_err(|e| RuleError::invalid(format!("`timeout_s` is out of range: {e}")))?;
        let mut deadline = clock.sleep_until(clock.now() + timeout);
        loop {
            let found = self
                .newer_messages(me, q.after_id, filter.clone(), q.limit)
                .await?;
            if !found.messages.is_empty() {
                return Ok(found);
            }
            loop {
                tokio::select! {
                    () = &mut deadline => return Ok(found),
                    notice = notices.recv() => match notice {
                        Ok(notice) if notice.id > q.after_id => break,
                        Ok(_older) => continue,
                        // Missed some notices; look in the database instead.
                        Err(RecvError::Lagged(_)) => break,
                        // The server is shutting down.
                        Err(RecvError::Closed) => return Ok(found),
                    },
                }
            }
        }
    }

    async fn newer_messages(
        &self,
        me: &Agent,
        after_id: i64,
        filter: Option<String>,
        limit: usize,
    ) -> Result<WaitResult> {
        let me = me.name.clone();
        self.db()
            .read(move |conn| {
                const SCOPE: &str = "m.id > :after
                    AND m.channel_id IN (SELECT channel_id FROM memberships WHERE agent = :me)
                    AND (:filter IS NULL OR m.channel_id IN (SELECT value FROM json_each(:filter)))";
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
                     WHERE {SCOPE} ORDER BY m.id LIMIT :limit"
                ))?;
                let messages = stmt
                    .query_map(
                        named_params! {
                            ":after": after_id, ":me": me, ":filter": filter, ":limit": limit as i64,
                        },
                        message_from_row,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let total: i64 = conn.query_row(
                    &format!("SELECT count(*) FROM messages m WHERE {SCOPE}"),
                    named_params! { ":after": after_id, ":me": me, ":filter": filter },
                    |r| r.get(0),
                )?;
                Ok(WaitResult {
                    more_waiting: total - messages.len() as i64,
                    messages,
                })
            })
            .await
    }
}

/// Reads one page of a channel. The caller has already checked access.
pub(super) fn read_channel(conn: &Connection, q: &ReadQuery) -> Result<MessagePage> {
    let fetch = (q.limit + 1) as i64;
    let (sql, bound, ascending) = match q.position {
        Position::After(id) => ("m.id > :bound ORDER BY m.id ASC", id, true),
        Position::Before(id) => ("m.id < :bound ORDER BY m.id DESC", id, false),
        Position::Latest => ("m.id <= :bound ORDER BY m.id DESC", i64::MAX, false),
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
         WHERE m.channel_id = :ch AND (:from IS NULL OR m.agent = :from) AND {sql}
         LIMIT :limit"
    ))?;
    let mut messages = stmt
        .query_map(
            named_params! { ":ch": q.channel_id, ":from": q.from_agent, ":bound": bound, ":limit": fetch },
            message_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = messages.len() > q.limit;
    messages.truncate(q.limit);
    if !ascending {
        messages.reverse();
    }
    let more = match (has_more, ascending) {
        (false, _) => None,
        (true, true) => messages.last().map(|m| Position::After(m.id)),
        (true, false) => messages.first().map(|m| Position::Before(m.id)),
    };
    Ok(MessagePage { messages, more })
}

fn load_message(conn: &Connection, id: i64) -> Result<Option<Message>> {
    conn.query_row(
        &format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE m.id = ?"
        ),
        [id],
        message_from_row,
    )
    .optional()
    .map_err(RuleError::from)
}

/// Turns free text into an FTS5 query that matches messages containing every
/// word. Each word is quoted, so FTS5 syntax in the text has no effect.
fn fts_query(text: &str) -> Result<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    if words.is_empty() {
        return Err(RuleError::invalid("search `query` must not be empty"));
    }
    Ok(words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_quotes_words() {
        assert_eq!(fts_query("parser bug").unwrap(), "\"parser\" \"bug\"");
        assert_eq!(fts_query("a\"b OR").unwrap(), "\"a\"\"b\" \"OR\"");
        assert!(fts_query("   ").is_err());
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Message {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
}

/// Where to read from in a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    /// The newest messages.
    Latest,
    /// Messages with IDs above this one, oldest first.
    After(i64),
    /// Messages with IDs below this one, the nearest first.
    Before(i64),
}

#[derive(Clone, Debug)]
pub struct ReadQuery {
    pub channel_id: String,
    pub position: Position,
    pub from_agent: Option<String>,
    pub limit: usize,
}

/// Messages in ID order, plus where to continue if more exist in the
/// direction being read.
#[derive(Clone, Debug, Serialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    #[serde(skip)]
    pub more: Option<Position>,
}

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub query: String,
    pub channel_id: Option<String>,
    pub from_agent: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub message_id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub created_at: String,
    pub excerpt: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivityRow {
    pub channel_id: String,
    pub channel_name: String,
    pub new_messages: i64,
    pub latest_id: i64,
}

#[derive(Clone, Debug)]
pub struct WaitQuery {
    pub after_id: i64,
    pub channel_ids: Option<Vec<String>>,
    pub timeout: std::time::Duration,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct WaitResult {
    pub messages: Vec<Message>,
    /// Newer messages beyond the ones returned.
    pub more_waiting: i64,
}
