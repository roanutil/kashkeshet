use super::*;
use crate::config::MAX_CHANNEL_ID_ATTEMPTS;

impl Rules {
    /// Channels `me` can see, in name order. Open channels are visible to
    /// everyone; closed ones only to their members.
    pub async fn list_channels(&self, me: &Agent, q: ChannelQuery) -> Result<Page<ChannelInfo>> {
        let me = me.name.clone();
        let preview = self.limits().member_preview;
        let (cursor_name, cursor_id) = match q.cursor.as_deref() {
            Some(c) => {
                let (name, id) = decode_channel_cursor(c)?;
                (Some(name), Some(id))
            }
            None => (None, None),
        };
        self.db()
            .read(move |conn| {
                if let Some(other) = &q.with_agent
                    && !agent_exists(conn, other)?
                {
                    return Err(RuleError::agent_not_found(other));
                }
                let mut stmt = conn.prepare_cached(
                    "SELECT c.id FROM channels c
                     WHERE (c.is_open = 1 OR EXISTS (SELECT 1 FROM memberships m
                                                     WHERE m.channel_id = c.id AND m.agent = :me))
                       AND (:member_only = 0 OR EXISTS (SELECT 1 FROM memberships m
                                                        WHERE m.channel_id = c.id AND m.agent = :me))
                       AND (:with IS NULL OR (
                             EXISTS (SELECT 1 FROM memberships m WHERE m.channel_id = c.id AND m.agent = :me)
                         AND EXISTS (SELECT 1 FROM memberships w WHERE w.channel_id = c.id AND w.agent = :with)))
                       AND (:pattern IS NULL
                            OR c.name LIKE :pattern ESCAPE '\\'
                            OR coalesce(c.purpose, '') LIKE :pattern ESCAPE '\\')
                       AND (:cname IS NULL OR (c.name, c.id) > (:cname, :cid))
                     ORDER BY c.name, c.id
                     LIMIT :limit",
                )?;
                let mut ids = stmt
                    .query_map(
                        named_params! {
                            ":me": me,
                            ":member_only": q.member_only,
                            ":with": q.with_agent,
                            ":pattern": q.query.as_deref().map(like_pattern),
                            ":cname": cursor_name,
                            ":cid": cursor_id,
                            ":limit": (q.limit + 1) as i64,
                        },
                        |r| r.get::<_, String>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let more = ids.len() > q.limit;
                ids.truncate(q.limit);
                let items = ids
                    .iter()
                    .map(|id| channel_info(conn, id, Some(&me), q.after_id, preview))
                    .collect::<Result<Vec<_>>>()?;
                let next_cursor = match (more, items.last()) {
                    (true, Some(last)) => Some(encode_channel_cursor(&last.name, &last.id)),
                    _ => None,
                };
                Ok(Page { items, next_cursor })
            })
            .await
    }

    /// Members of a channel, in name order. Anyone may list an open channel's
    /// members; only members may list a closed channel's.
    pub async fn list_members(
        &self,
        me: &Agent,
        channel_id: String,
        limit: usize,
        cursor: Option<String>,
    ) -> Result<Members> {
        let me = me.name.clone();
        self.db()
            .read(move |conn| {
                visible_channel(conn, &me, &channel_id)?;
                members_page(conn, &channel_id, limit, cursor.as_deref())
            })
            .await
    }

    /// Creates a channel. The creator and every listed member join at once.
    pub async fn create_channel(&self, me: &Agent, new: NewChannel) -> Result<ChannelInfo> {
        let limits = self.limits().clone();
        let name = clean_name(&new.name, limits.max_channel_name_chars)?;
        let purpose = clean_purpose(new.purpose.as_deref(), limits.max_purpose_chars)?;
        let me = me.name.clone();
        let now = self.now_text();
        let entropy = self.inner.deps.entropy.clone();
        self.db()
            .write(move |tx| {
                let mut members = vec![me.clone()];
                for m in &new.members {
                    let m = m.trim();
                    if !agent_exists(tx, m)? {
                        return Err(RuleError::agent_not_found(m));
                    }
                    if !members.iter().any(|x| x == m) {
                        members.push(m.to_string());
                    }
                }
                let id = free_channel_id(tx, entropy.as_ref())?;
                tx.execute(
                    "INSERT INTO channels (id, name, purpose, is_open, created_by, created_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![id, name, purpose, new.open, me, now],
                )?;
                for m in &members {
                    add_member(tx, &id, m, &now)?;
                }
                channel_info(tx, &id, Some(&me), None, limits.member_preview)
            })
            .await
    }

    /// Joins an open channel. Joining a channel you are already in does nothing.
    pub async fn join_channel(&self, me: &Agent, channel_id: String) -> Result<ChannelInfo> {
        let me = me.name.clone();
        let now = self.now_text();
        let preview = self.limits().member_preview;
        self.db()
            .write(move |tx| {
                let (row, member) = visible_channel(tx, &me, &channel_id)?;
                if !member {
                    // `visible_channel` only lets a non-member through for an open channel.
                    add_member(tx, &row.id, &me, &now)?;
                }
                channel_info(tx, &row.id, Some(&me), None, preview)
            })
            .await
    }

    pub async fn leave_channel(&self, me: &Agent, channel_id: String) -> Result<()> {
        let me = me.name.clone();
        self.db()
            .write(move |tx| {
                let row = member_channel(tx, &me, &channel_id)?;
                tx.execute(
                    "DELETE FROM memberships WHERE channel_id = ? AND agent = ?",
                    [&row.id, &me],
                )?;
                Ok(())
            })
            .await
    }

    /// Adds agents to a channel `me` is in. They become members at once.
    /// Agents already in the channel are left as they are.
    pub async fn add_to_channel(
        &self,
        me: &Agent,
        channel_id: String,
        agents: Vec<String>,
    ) -> Result<Members> {
        let me = me.name.clone();
        let now = self.now_text();
        let limit = self.limits().default_limit;
        self.db()
            .write(move |tx| {
                let row = member_channel(tx, &me, &channel_id)?;
                if agents.is_empty() {
                    return Err(RuleError::invalid("`agents` must name at least one agent"));
                }
                for a in &agents {
                    let a = a.trim();
                    if !agent_exists(tx, a)? {
                        return Err(RuleError::agent_not_found(a));
                    }
                    add_member(tx, &row.id, a, &now)?;
                }
                members_page(tx, &row.id, limit, None)
            })
            .await
    }

    pub async fn set_purpose(
        &self,
        me: &Agent,
        channel_id: String,
        purpose: String,
    ) -> Result<ChannelInfo> {
        let limits = self.limits().clone();
        let purpose = clean_purpose(Some(&purpose), limits.max_purpose_chars)?;
        let me = me.name.clone();
        self.db()
            .write(move |tx| {
                let row = member_channel(tx, &me, &channel_id)?;
                tx.execute(
                    "UPDATE channels SET purpose = ? WHERE id = ?",
                    rusqlite::params![purpose, row.id],
                )?;
                channel_info(tx, &row.id, Some(&me), None, limits.member_preview)
            })
            .await
    }
}

fn add_member(conn: &Connection, channel_id: &str, agent: &str, now: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO memberships (channel_id, agent, joined_at) VALUES (?, ?, ?)",
        [channel_id, agent, now],
    )?;
    Ok(())
}

fn members_page(
    conn: &Connection,
    channel_id: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<Members> {
    let total: i64 = conn.query_row(
        "SELECT count(*) FROM memberships WHERE channel_id = ?",
        [channel_id],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare_cached(
        "SELECT agent FROM memberships
         WHERE channel_id = :id AND (:cursor IS NULL OR agent > :cursor)
         ORDER BY agent LIMIT :limit",
    )?;
    let mut items = stmt
        .query_map(
            named_params! { ":id": channel_id, ":cursor": cursor, ":limit": (limit + 1) as i64 },
            |r| r.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().cloned()
    } else {
        None
    };
    Ok(Members {
        channel_id: channel_id.to_string(),
        total,
        page: Page { items, next_cursor },
    })
}

fn free_channel_id(conn: &Connection, entropy: &dyn Entropy) -> Result<String> {
    for _ in 0..MAX_CHANNEL_ID_ATTEMPTS {
        let id = names::channel_id(entropy);
        if channel_row(conn, &id)?.is_none() {
            return Ok(id);
        }
    }
    Err(RuleError::new(
        Code::Internal,
        "could not find a free channel ID",
    ))
}

fn clean_name(name: &str, max_chars: usize) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RuleError::invalid("channel name must not be empty"));
    }
    if name.chars().count() > max_chars {
        return Err(RuleError::invalid(format!(
            "channel name must be at most {max_chars} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(RuleError::invalid(
            "channel name must not contain line breaks or control characters",
        ));
    }
    Ok(name.to_string())
}

/// A trimmed purpose, or `None` if it is empty.
fn clean_purpose(purpose: Option<&str>, max_chars: usize) -> Result<Option<String>> {
    let Some(purpose) = purpose.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    if purpose.chars().count() > max_chars {
        return Err(RuleError::invalid(format!(
            "purpose must be at most {max_chars} characters"
        )));
    }
    Ok(Some(purpose.to_string()))
}

/// The `list_channels` cursor that continues after `c`.
pub fn channel_cursor(c: &ChannelInfo) -> String {
    encode_channel_cursor(&c.name, &c.id)
}

/// A list cursor holds the last channel's name (as hex, since names can hold
/// any character) and ID. It carries the position itself, so using it never
/// reveals whether some other channel exists.
fn encode_channel_cursor(name: &str, id: &str) -> String {
    let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{hex}.{id}")
}

fn decode_channel_cursor(cursor: &str) -> Result<(String, String)> {
    let bad = || RuleError::invalid("`cursor` is not one this server gave out");
    let (hex, id) = cursor.split_once('.').ok_or_else(bad)?;
    if hex.len() % 2 != 0 {
        return Err(bad());
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .map_err(|_| bad())?;
    let name = String::from_utf8(bytes).map_err(|_| bad())?;
    Ok((name, id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        let c = encode_channel_cursor("général.x", "ch_abc");
        assert_eq!(
            decode_channel_cursor(&c).unwrap(),
            ("général.x".to_string(), "ch_abc".to_string())
        );
        assert!(decode_channel_cursor("zz.ch_abc").is_err());
        assert!(decode_channel_cursor("nodot").is_err());
    }

    #[test]
    fn names_and_purposes_are_checked() {
        assert_eq!(clean_name("  general ", 64).unwrap(), "general");
        assert!(clean_name("", 64).is_err());
        assert!(clean_name("a\nb", 64).is_err());
        assert!(clean_name(&"x".repeat(65), 64).is_err());
        assert_eq!(clean_purpose(Some("  "), 500).unwrap(), None);
        assert!(clean_purpose(Some(&"x".repeat(501)), 500).is_err());
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub purpose: Option<String>,
    pub open: bool,
    pub created_by: String,
    pub created_at: String,
    pub member_count: i64,
    /// The first few members, in the order they joined.
    pub members_preview: Vec<String>,
    pub is_member: bool,
    /// Messages newer than the `after_id` the agent gave, if it gave one.
    pub new_messages: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelQuery {
    pub query: Option<String>,
    pub member_only: bool,
    pub with_agent: Option<String>,
    pub after_id: Option<i64>,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewChannel {
    pub name: String,
    pub open: bool,
    pub purpose: Option<String>,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Members {
    pub channel_id: String,
    pub total: i64,
    pub page: Page<String>,
}
