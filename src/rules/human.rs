//! What the human sees through the web page. The human sees everything,
//! including closed channels. The human's changes are the same rules as
//! agents', made as [`Actor::Human`].

use super::*;
use crate::config::{WEB_ACTIVITY_BUCKET, WEB_ACTIVITY_WINDOW};
use std::collections::{BTreeMap, HashMap};

impl Rules {
    /// The front page: every channel with how much it is used, every agent
    /// with the channels it is in, and the server runs.
    pub async fn overview(&self) -> Result<Overview> {
        let rules = self.clone();
        let preview = self.limits().member_preview;
        let now = self.inner.deps.clock.now();
        let window = chrono::Duration::from_std(WEB_ACTIVITY_WINDOW)
            .map_err(|e| RuleError::new(Code::Internal, e.to_string()))?;
        let since = clock::to_text(now - window);
        let now = clock::to_text(now);
        self.db()
            .read(move |store| {
                let mut used: HashMap<String, ChannelUseRecord> = store
                    .channel_use()?
                    .into_iter()
                    .map(|u| (u.channel_id.clone(), u))
                    .collect();
                // Every message in the window, counted per channel in
                // stretches of `WEB_ACTIVITY_BUCKET`.
                let mut recent: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
                for m in store.message_times_since(&since)? {
                    let start = clock::floor_text(&m.created_at, WEB_ACTIVITY_BUCKET)
                        .unwrap_or(m.created_at);
                    *recent
                        .entry(m.channel_id)
                        .or_default()
                        .entry(start)
                        .or_default() += 1;
                }
                let channels = store
                    .all_channel_ids(None)?
                    .iter()
                    .map(|id| {
                        let info = channel_info(store, id, None, None, preview)?;
                        let use_ = used.remove(id);
                        let recent_activity = recent
                            .remove(id)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(start, count)| ActivityCount { start, count })
                            .collect();
                        Ok(ChannelActivity {
                            message_count: use_.as_ref().map_or(0, |u| u.message_count),
                            last_message_at: use_.as_ref().and_then(|u| u.last_message_at.clone()),
                            last_speaker: use_.and_then(|u| u.last_speaker),
                            recent_activity,
                            info,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut member_of: HashMap<String, Vec<ChannelRef>> = HashMap::new();
                for m in store.memberships()? {
                    member_of.entry(m.agent).or_default().push(ChannelRef {
                        id: m.channel_id,
                        name: m.channel_name,
                    });
                }
                let agents = store
                    .agents_by_last_seen()?
                    .into_iter()
                    .map(|a| {
                        Ok(AgentActivity {
                            channels: member_of.remove(&a.name).unwrap_or_default(),
                            info: rules.observed_agent(store, a, None, None)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let runs = store
                    .server_runs()?
                    .into_iter()
                    .map(ServerRunInfo::from)
                    .collect();
                Ok(Overview {
                    channels,
                    agents,
                    runs,
                    now,
                })
            })
            .await
    }

    pub async fn channel_view(&self, id: String, before_id: Option<i64>) -> Result<ChannelView> {
        let rules = self.clone();
        let preview = self.limits().member_preview;
        let page_messages = self.limits().web_page_messages;
        self.db()
            .read(move |store| {
                if store.channel(&id)?.is_none() {
                    return Err(RuleError::channel_not_found(&id));
                }
                let channel = channel_info(store, &id, None, None, preview)?;
                let members = store
                    .members_by_join(&id, usize::MAX >> 1)?
                    .iter()
                    .map(|name| {
                        store
                            .agent(name)?
                            .ok_or_else(|| RuleError::agent_not_found(name))
                            .and_then(|a| rules.observed_agent(store, a, None, Some(&id)))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let page = super::messages::read_channel(
                    store,
                    &ReadQuery {
                        channel_id: id.clone(),
                        position: before_id.map_or(Position::Latest, Position::Before),
                        from_agent: None,
                        limit: page_messages,
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
        let rules = self.clone();
        let preview = self.limits().member_preview;
        self.db()
            .read(move |store| {
                let agent = store
                    .agent(&name)?
                    .ok_or_else(|| RuleError::agent_not_found(&name))
                    .and_then(|a| rules.observed_agent(store, a, None, None))?;
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
    /// In name order.
    pub channels: Vec<ChannelActivity>,
    /// Most recently seen first.
    pub agents: Vec<AgentActivity>,
    /// Every server run, newest first.
    pub runs: Vec<ServerRunInfo>,
    /// The server's time when this was read, so the page can tell how long
    /// ago things happened without trusting the viewer's clock.
    pub now: String,
}

/// A channel and how much it is used.
#[derive(Clone, Debug)]
pub struct ChannelActivity {
    pub info: ChannelInfo,
    pub message_count: i64,
    pub last_message_at: Option<String>,
    pub last_speaker: Option<String>,
    /// The messages of the last `WEB_ACTIVITY_WINDOW`, counted in
    /// stretches of `WEB_ACTIVITY_BUCKET`, oldest first.
    pub recent_activity: Vec<ActivityCount>,
}

/// How many messages a channel got in the stretch starting at `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityCount {
    pub start: String,
    pub count: i64,
}

/// An agent and the channels it is in.
#[derive(Clone, Debug)]
pub struct AgentActivity {
    pub info: AgentInfo,
    /// In the order it joined them.
    pub channels: Vec<ChannelRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRef {
    pub id: String,
    pub name: String,
}

/// One start of the server and what produced it.
#[derive(Clone, Debug)]
pub struct ServerRunInfo {
    pub id: i64,
    pub started_at: String,
    pub version: String,
    pub git_commit: String,
    pub settings: String,
    pub instructions_sha256: String,
}

impl From<ServerRunRecord> for ServerRunInfo {
    fn from(r: ServerRunRecord) -> ServerRunInfo {
        ServerRunInfo {
            id: r.id,
            started_at: r.started_at,
            version: r.version,
            git_commit: r.git_commit,
            settings: r.settings,
            instructions_sha256: r.instructions_sha256,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChannelView {
    pub channel: ChannelInfo,
    pub members: Vec<AgentInfo>,
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
