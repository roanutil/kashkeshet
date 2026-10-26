use super::*;
use crate::config::MAX_CHANNEL_ID_ATTEMPTS;

impl Rules {
    /// Channels `me` can see, in name order. Open channels are visible to
    /// everyone; closed ones only to their members.
    pub async fn list_channels(&self, me: &Agent, q: ChannelQuery) -> Result<Page<ChannelInfo>> {
        let me = me.name.clone();
        let preview = self.limits().member_preview;
        let after = q.cursor.as_deref().map(decode_channel_cursor).transpose()?;
        self.db()
            .read(move |store| {
                if let Some(other) = &q.with_agent
                    && !agent_exists(store, other)?
                {
                    return Err(RuleError::agent_not_found(other));
                }
                let mut ids = store.channel_ids(&ChannelFilter {
                    visible_to: me.clone(),
                    member_only: q.member_only,
                    shared_with: q.with_agent.clone(),
                    text: q.query.clone(),
                    after,
                    limit: q.limit + 1,
                })?;
                let more = ids.len() > q.limit;
                ids.truncate(q.limit);
                let items = ids
                    .iter()
                    .map(|id| channel_info(store, id, Some(&me), q.after_id, preview))
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
            .read(move |store| {
                visible_channel(store, &me, &channel_id)?;
                members_page(store, &channel_id, limit, cursor.as_deref())
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
        let created = self
            .db()
            .write(move |store| {
                let mut members = vec![me.clone()];
                for m in &new.members {
                    let m = m.trim();
                    if !agent_exists(store, m)? {
                        return Err(RuleError::agent_not_found(m));
                    }
                    if !members.iter().any(|x| x == m) {
                        members.push(m.to_string());
                    }
                }
                let id = free_channel_id(store, entropy.as_ref())?;
                store.insert_channel(&NewChannelRecord {
                    id: id.clone(),
                    name,
                    purpose,
                    open: new.open,
                    created_by: me.clone(),
                    created_at: now.clone(),
                })?;
                for m in &members {
                    store.add_member(&id, m, &now)?;
                }
                channel_info(store, &id, Some(&me), None, limits.member_preview)
            })
            .await?;
        self.events().publish_channel(NewChannelEvent {
            id: created.id.clone(),
            name: created.name.clone(),
            open: created.open,
            created_by: created.created_by.clone(),
            created_at: created.created_at.clone(),
            member_count: created.member_count,
        });
        Ok(created)
    }

    /// Joins an open channel. Joining a channel you are already in does nothing.
    pub async fn join_channel(&self, me: &Agent, channel_id: String) -> Result<ChannelInfo> {
        let me = me.name.clone();
        let now = self.now_text();
        let preview = self.limits().member_preview;
        let who = me.clone();
        let (info, joined) = self
            .db()
            .write(move |store| {
                let (channel, member) = visible_channel(store, &me, &channel_id)?;
                if !member {
                    // `visible_channel` only lets a non-member through for an open channel.
                    store.add_member(&channel.id, &me, &now)?;
                }
                let info = channel_info(store, &channel.id, Some(&me), None, preview)?;
                Ok::<_, RuleError>((info, !member))
            })
            .await?;
        if joined {
            self.events().publish_joined(Joined {
                agent: who,
                channel_id: info.id.clone(),
                channel_name: info.name.clone(),
                member_count: info.member_count,
            });
        }
        Ok(info)
    }

    pub async fn leave_channel(&self, me: &Agent, channel_id: String) -> Result<()> {
        let me = me.name.clone();
        let who = me.clone();
        let (channel, member_count) = self
            .db()
            .write(move |store| {
                let channel = member_channel(store, &me, &channel_id)?;
                store.remove_member(&channel.id, &me)?;
                let count = store.member_count(&channel.id)?;
                Ok::<_, RuleError>((channel, count))
            })
            .await?;
        self.events().publish_left(Left {
            agent: who,
            channel_id: channel.id,
            channel_name: channel.name,
            member_count,
        });
        Ok(())
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
        let (page, channel, added, member_count) = self
            .db()
            .write(move |store| {
                let channel = member_channel(store, &me, &channel_id)?;
                if agents.is_empty() {
                    return Err(RuleError::invalid("`agents` must name at least one agent"));
                }
                let mut added: Vec<String> = Vec::new();
                for a in &agents {
                    let a = a.trim();
                    if !agent_exists(store, a)? {
                        return Err(RuleError::agent_not_found(a));
                    }
                    if !store.is_member(&channel.id, a)? && !added.iter().any(|x| x == a) {
                        added.push(a.to_string());
                    }
                    store.add_member(&channel.id, a, &now)?;
                }
                let page = members_page(store, &channel.id, limit, None)?;
                let count = store.member_count(&channel.id)?;
                Ok::<_, RuleError>((page, channel, added, count))
            })
            .await?;
        // Being added is joining, as far as anyone watching is concerned.
        for agent in added {
            self.events().publish_joined(Joined {
                agent,
                channel_id: channel.id.clone(),
                channel_name: channel.name.clone(),
                member_count,
            });
        }
        Ok(page)
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
            .write(move |store| {
                let channel = member_channel(store, &me, &channel_id)?;
                store.set_purpose(&channel.id, purpose.as_deref())?;
                channel_info(store, &channel.id, Some(&me), None, limits.member_preview)
            })
            .await
    }
}

fn members_page(
    store: &dyn Reads,
    channel_id: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<Members> {
    let total = store.member_count(channel_id)?;
    let mut items: Vec<AgentInfo> = store
        .members_by_name(channel_id, cursor, limit + 1)?
        .into_iter()
        .map(AgentInfo::from)
        .collect();
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|a| a.name.clone())
    } else {
        None
    };
    Ok(Members {
        channel_id: channel_id.to_string(),
        total,
        page: Page { items, next_cursor },
    })
}

fn free_channel_id(store: &dyn Reads, entropy: &dyn Entropy) -> Result<String> {
    for _ in 0..MAX_CHANNEL_ID_ATTEMPTS {
        let id = names::channel_id(entropy);
        if store.channel(&id)?.is_none() {
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
    pub page: Page<AgentInfo>,
}
