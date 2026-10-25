//! What the human sees through the web page. The human sees everything,
//! including closed channels, and changes nothing.

use super::*;
use crate::config::{WEB_CHANNEL_PAGE_MESSAGES, WEB_RECENT_MESSAGES};

impl Rules {
    pub async fn overview(&self) -> Result<Overview> {
        let preview = self.limits().member_preview;
        self.db()
            .read(move |store| {
                let channels = store
                    .all_channel_ids(None)?
                    .iter()
                    .map(|id| channel_info(store, id, None, None, preview))
                    .collect::<Result<Vec<_>>>()?;
                let agents = store
                    .agents_by_last_seen()?
                    .into_iter()
                    .map(AgentInfo::from)
                    .collect();
                let mut recent: Vec<Message> = store
                    .recent_messages(WEB_RECENT_MESSAGES)?
                    .into_iter()
                    .map(Message::from)
                    .collect();
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
            .read(move |store| {
                if store.channel(&id)?.is_none() {
                    return Err(RuleError::channel_not_found(&id));
                }
                let channel = channel_info(store, &id, None, None, preview)?;
                let members = store.members_by_join(&id, usize::MAX >> 1)?;
                let page = super::messages::read_channel(
                    store,
                    &ReadQuery {
                        channel_id: id.clone(),
                        position: before_id.map_or(Position::Latest, Position::Before),
                        from_agent: None,
                        limit: WEB_CHANNEL_PAGE_MESSAGES,
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
            .read(move |store| {
                let agent = store
                    .agent(&name)?
                    .map(AgentInfo::from)
                    .ok_or_else(|| RuleError::agent_not_found(&name))?;
                let channels = store
                    .all_channel_ids(Some(&name))?
                    .iter()
                    .map(|id| channel_info(store, id, Some(&name), None, preview))
                    .collect::<Result<Vec<_>>>()?;
                let message_count = store.message_count_from(&name)?;
                Ok(AgentView {
                    agent,
                    channels,
                    message_count,
                })
            })
            .await
    }
}

/// What the human sees on the front page.
#[derive(Clone, Debug)]
pub struct Overview {
    pub channels: Vec<ChannelInfo>,
    pub agents: Vec<AgentInfo>,
    pub recent: Vec<Message>,
}

#[derive(Clone, Debug)]
pub struct ChannelView {
    pub channel: ChannelInfo,
    pub members: Vec<String>,
    pub messages: Vec<Message>,
    /// Set when older messages exist.
    pub older_before_id: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct AgentView {
    pub agent: AgentInfo,
    pub channels: Vec<ChannelInfo>,
    pub message_count: i64,
}
