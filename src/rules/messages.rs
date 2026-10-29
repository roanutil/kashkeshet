use super::*;
use crate::events::{NewMessage, Notice};
use std::sync::atomic::{AtomicBool, Ordering};

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
        if auth::contains_secret(&body) {
            return Err(RuleError::new(
                Code::ContainsSecret,
                "message contains something shaped like a token or recovery code. \
                 Tokens and recovery codes must never be shared; remove it and send again.",
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
            .write(move |store| {
                let me = sender;
                let channel = member_channel(store, &me, &channel_id)?;
                if !rules.inner.deps.sends.allow(&me, now) {
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
                let run_id = rules.inner.deps.run_id;
                let id = store.insert_message(&NewMessageRecord {
                    channel_id: channel.id.clone(),
                    agent: me.clone(),
                    body: body.clone(),
                    created_at: created_at.clone(),
                    run_id,
                })?;
                Ok(Message {
                    id,
                    channel_id: channel.id,
                    channel_name: channel.name,
                    agent: me,
                    body,
                    created_at,
                    run_id: Some(run_id),
                })
            })
            .await;
        let message = match written {
            Ok(message) => message,
            Err(e) => {
                if charged.load(Ordering::SeqCst) {
                    self.inner.deps.sends.refund(&me);
                }
                return Err(e);
            }
        };
        self.publish_message(&message);
        Ok(message)
    }

    /// Tells waiting agents and the web page about a stored message.
    pub(super) fn publish_message(&self, message: &Message) {
        self.events().publish(NewMessage {
            id: message.id,
            channel_id: message.channel_id.clone(),
            channel_name: message.channel_name.clone(),
            agent: message.agent.clone(),
            body: message.body.clone(),
            created_at: message.created_at.clone(),
            run_id: message.run_id,
        });
    }

    /// Stores a message from the operator in `channel`, saying what the
    /// human changed. Runs inside the change's own write, so the change and
    /// its message are stored together or not at all. The caller publishes
    /// the message once the write is done.
    pub(super) fn post_as_operator(
        &self,
        store: &dyn Writes,
        channel: &ChannelFacts,
        body: String,
    ) -> Result<Message> {
        self.post_notice(store, channel, names::OPERATOR, body)
    }

    /// Stores a message from `author` in `channel`, saying what changed,
    /// inside the change's own write, as [`Rules::post_as_operator`] does.
    pub(super) fn post_notice(
        &self,
        store: &dyn Writes,
        channel: &ChannelFacts,
        author: &str,
        body: String,
    ) -> Result<Message> {
        let created_at = self.now_text();
        let run_id = self.inner.deps.run_id;
        let id = store.insert_message(&NewMessageRecord {
            channel_id: channel.id.clone(),
            agent: author.to_string(),
            body: body.clone(),
            created_at: created_at.clone(),
            run_id,
        })?;
        Ok(Message {
            id,
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            agent: author.to_string(),
            body,
            created_at,
            run_id: Some(run_id),
        })
    }

    /// Messages in a channel `me` is in.
    pub async fn read(&self, me: &Agent, q: ReadQuery) -> Result<MessagePage> {
        let me = me.name.clone();
        self.db()
            .read(move |store| {
                member_channel(store, &me, &q.channel_id)?;
                read_channel(store, &q)
            })
            .await
    }

    /// One message in full.
    pub async fn get_message(&self, me: &Agent, id: i64) -> Result<Message> {
        let me = me.name.clone();
        self.db()
            .read(move |store| {
                let message = store
                    .message(id)?
                    .ok_or_else(|| RuleError::message_not_found(id))?;
                let channel = store.channel(&message.channel_id)?;
                let member = match &channel {
                    Some(c) => store.is_member(&c.id, &me)?,
                    None => false,
                };
                let open = channel.as_ref().is_some_and(|c| c.open);
                match access::decide(
                    channel.is_some(),
                    open,
                    Who::Agent { member },
                    Need::Membership,
                ) {
                    Decision::Allowed => Ok(Message::from(message)),
                    Decision::NotMember => Err(RuleError::not_member(&message.channel_id)),
                    // A message in a hidden channel does not exist, as far as `me` can tell.
                    Decision::NotFound => Err(RuleError::message_not_found(id)),
                }
            })
            .await
    }

    /// Full-text search over the channels `me` is in, newest first.
    pub async fn search(&self, me: &Agent, q: SearchQuery) -> Result<Vec<SearchHit>> {
        let words: Vec<String> = q.query.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Err(RuleError::invalid("search `query` must not be empty"));
        }
        let me = me.name.clone();
        self.db()
            .read(move |store| {
                if let Some(ch) = &q.channel_id {
                    member_channel(store, &me, ch)?;
                }
                let hits = store.search(&SearchFilter {
                    member: me,
                    words,
                    channel_id: q.channel_id,
                    from_agent: q.from_agent,
                    limit: q.limit,
                })?;
                Ok(hits.into_iter().map(SearchHit::from).collect())
            })
            .await
    }

    /// The channels `me` is in that have messages newer than `after_id`,
    /// busiest-most-recently first.
    pub async fn activity(&self, me: &Agent, after_id: i64) -> Result<Vec<ActivityRow>> {
        let me = me.name.clone();
        self.db()
            .read(move |store| {
                let rows = store.activity(&me, after_id)?;
                Ok(rows.into_iter().map(ActivityRow::from).collect())
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
                    .read(move |store| {
                        for id in &to_check {
                            member_channel(store, &who, id)?;
                        }
                        Ok::<_, RuleError>(())
                    })
                    .await?;
                Some(ids.clone())
            }
        };

        let grace = chrono::Duration::from_std(self.limits().waiting_grace)
            .map_err(|e| RuleError::invalid(e.to_string()))?;
        let guard =
            self.inner
                .deps
                .waiting
                .start(&me.name, filter.clone(), self.inner.deps.clock.now());
        let result = async {
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
                    notice = notices.next() => match notice {
                        Some(Notice::Message(m)) if m.id > q.after_id => break,
                        Some(Notice::Message(_older)) => continue,
                        // Missed some notices; look in the database instead.
                        Some(Notice::Missed(_)) => break,
                        // New agents, channels and members are for the web
                        // page; a waiting agent wants messages.
                        Some(
                            Notice::Agent(_) | Notice::Channel(_) | Notice::Joined(_) | Notice::Left(_),
                        ) => continue,
                        // The server is shutting down.
                        None => return Ok(found),
                    },
                }
            }
        }
        }.await;
        if result.is_ok() {
            guard.finish(self.inner.deps.clock.now() + grace);
        }
        result
    }

    async fn newer_messages(
        &self,
        me: &Agent,
        after_id: i64,
        filter: Option<Vec<String>>,
        limit: usize,
    ) -> Result<WaitResult> {
        let me = me.name.clone();
        self.db()
            .read(move |store| {
                let filter = NewerFilter {
                    member: me,
                    after_id,
                    channel_ids: filter,
                };
                let messages = store.newer_messages(&filter, limit)?;
                let total = store.count_newer_messages(&filter)?;
                Ok(WaitResult {
                    more_waiting: total - messages.len() as i64,
                    messages: messages.into_iter().map(Message::from).collect(),
                })
            })
            .await
    }
}

/// Reads one page of a channel. The caller has already checked access.
pub(super) fn read_channel(store: &dyn Reads, q: &ReadQuery) -> Result<MessagePage> {
    let (range, ascending) = match q.position {
        Position::After(id) => (MessageRange::After(id), true),
        Position::Before(id) => (MessageRange::Before(id), false),
        Position::Latest => (MessageRange::Latest, false),
    };
    let mut messages: Vec<Message> = store
        .channel_messages(&q.channel_id, q.from_agent.as_deref(), range, q.limit + 1)?
        .into_iter()
        .map(Message::from)
        .collect();
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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Message {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
    /// The server run that stored the message, if it was recorded.
    pub run_id: Option<i64>,
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

impl From<MessageRecord> for Message {
    fn from(m: MessageRecord) -> Message {
        Message {
            id: m.id,
            channel_id: m.channel_id,
            channel_name: m.channel_name,
            agent: m.agent,
            body: m.body,
            created_at: m.created_at,
            run_id: m.run_id,
        }
    }
}

impl From<SearchHitRecord> for SearchHit {
    fn from(h: SearchHitRecord) -> SearchHit {
        SearchHit {
            message_id: h.message_id,
            channel_id: h.channel_id,
            channel_name: h.channel_name,
            agent: h.agent,
            created_at: h.created_at,
            excerpt: h.excerpt,
        }
    }
}

impl From<ActivityRecord> for ActivityRow {
    fn from(r: ActivityRecord) -> ActivityRow {
        ActivityRow {
            channel_id: r.channel_id,
            channel_name: r.channel_name,
            new_messages: r.new_messages,
            latest_id: r.latest_id,
        }
    }
}
