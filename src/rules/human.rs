//! What the human sees through the web page. The human sees everything,
//! including closed channels, and changes nothing.

use super::*;

/// Messages shown on the front page.
const RECENT_MESSAGES: usize = 50;
/// Messages shown per channel page.
const CHANNEL_PAGE_MESSAGES: usize = 200;

impl Rules {
    pub async fn overview(&self) -> Result<Overview> {
        let preview = self.limits().member_preview;
        self.db()
            .read(move |conn| {
                let channels = all_channel_ids(conn, None)?
                    .iter()
                    .map(|id| channel_info(conn, id, None, None, preview))
                    .collect::<Result<Vec<_>>>()?;
                let agents = all_agents(conn)?;
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
                     ORDER BY m.id DESC LIMIT ?"
                ))?;
                let mut recent = stmt
                    .query_map([RECENT_MESSAGES as i64], message_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                recent.reverse();
                Ok(Overview {
                    channels,
                    agents,
                    recent,
                })
            })
            .await
    }

    pub async fn channel_view(&self, id: String, before_id: Option<i64>) -> Result<ChannelView> {
        let preview = self.limits().member_preview;
        self.db()
            .read(move |conn| {
                if channel_row(conn, &id)?.is_none() {
                    return Err(RuleError::channel_not_found(&id));
                }
                let channel = channel_info(conn, &id, None, None, preview)?;
                let members = member_names(conn, &id, usize::MAX >> 1)?;
                let page = super::messages::read_channel(
                    conn,
                    &ReadQuery {
                        channel_id: id.clone(),
                        position: before_id.map_or(Position::Latest, Position::Before),
                        from_agent: None,
                        limit: CHANNEL_PAGE_MESSAGES,
                    },
                )?;
                let older_before_id = match page.more {
                    Some(Position::Before(b)) => Some(b),
                    _ => None,
                };
                Ok(ChannelView {
                    channel,
                    members,
                    messages: page.messages,
                    older_before_id,
                })
            })
            .await
    }

    pub async fn agent_view(&self, name: String) -> Result<AgentView> {
        let preview = self.limits().member_preview;
        self.db()
            .read(move |conn| {
                let agent = conn
                    .query_row(
                        "SELECT name, created_at, last_seen FROM agents WHERE name = ?",
                        [&name],
                        |r| {
                            Ok(AgentInfo {
                                name: r.get(0)?,
                                created_at: r.get(1)?,
                                last_seen: r.get(2)?,
                            })
                        },
                    )
                    .optional()?
                    .ok_or_else(|| RuleError::agent_not_found(&name))?;
                let channels = all_channel_ids(conn, Some(&name))?
                    .iter()
                    .map(|id| channel_info(conn, id, Some(&name), None, preview))
                    .collect::<Result<Vec<_>>>()?;
                let message_count = conn.query_row(
                    "SELECT count(*) FROM messages WHERE agent = ?",
                    [&name],
                    |r| r.get(0),
                )?;
                Ok(AgentView {
                    agent,
                    channels,
                    message_count,
                })
            })
            .await
    }
}

/// Every channel ID in name order, or only those `member` is in.
fn all_channel_ids(conn: &Connection, member: Option<&str>) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.id FROM channels c
         WHERE :member IS NULL
            OR EXISTS (SELECT 1 FROM memberships m WHERE m.channel_id = c.id AND m.agent = :member)
         ORDER BY c.name, c.id",
    )?;
    let ids = stmt
        .query_map(named_params! { ":member": member }, |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

fn all_agents(conn: &Connection) -> Result<Vec<AgentInfo>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name, created_at, last_seen FROM agents ORDER BY coalesce(last_seen, created_at) DESC",
    )?;
    let agents = stmt
        .query_map([], |r| {
            Ok(AgentInfo {
                name: r.get(0)?,
                created_at: r.get(1)?,
                last_seen: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(agents)
}
