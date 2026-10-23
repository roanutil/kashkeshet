use serde::Serialize;

/// An agent that has proven who it is on this call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Registration {
    pub name: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub created_at: String,
    pub last_seen: Option<String>,
}

/// One page of a list, plus the cursor for the next page if there is one.
#[derive(Clone, Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
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
