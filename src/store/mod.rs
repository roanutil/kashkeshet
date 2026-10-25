//! Storage: what the rules can ask of the database and change in it.
//!
//! The contract is two traits. [`Reads`] answers questions; [`Writes`] adds
//! the changes. Neither decides anything about permissions: the rules ask, for
//! example, whether an agent is a member, and decide what that means.
//!
//! [`Db`] is the one implementation, over SQLite. Each call to [`Db::read`] or
//! [`Db::write`] runs one closure, and a write closure is one transaction: its
//! changes all commit together, or not at all.

mod sqlite;

#[cfg(test)]
mod tests;

pub use sqlite::Db;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(String),
    #[error("database task failed: {0}")]
    Task(String),
    #[error("database lock poisoned")]
    Poisoned,
}

pub type StoreResult<T> = Result<T, StoreError>;

/// An agent as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRecord {
    pub name: String,
    pub created_at: String,
    pub last_seen: Option<String>,
    /// The server run that created the agent. `None` for agents from before
    /// runs were recorded.
    pub run_id: Option<i64>,
    /// When the operator revoked the agent's token, if it has.
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewAgentRecord {
    pub name: String,
    pub token_hash: Vec<u8>,
    pub created_at: String,
    pub last_seen: Option<String>,
    pub run_id: i64,
}

/// The facts about a channel that decide who may use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelFacts {
    pub id: String,
    pub name: String,
    pub open: bool,
}

/// A channel's stored fields and counts, as one viewer sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSummary {
    pub id: String,
    pub name: String,
    pub purpose: Option<String>,
    pub open: bool,
    pub created_by: String,
    pub created_at: String,
    pub member_count: i64,
    /// False when there is no viewer.
    pub viewer_is_member: bool,
    /// Messages with IDs above the `after_id` asked for, if one was.
    pub new_messages: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct NewChannelRecord {
    pub id: String,
    pub name: String,
    pub purpose: Option<String>,
    pub open: bool,
    pub created_by: String,
    pub created_at: String,
}

/// Which channels [`Reads::channel_ids`] returns, in name order.
#[derive(Clone, Debug)]
pub struct ChannelFilter {
    /// Channels that are open or that this agent is in.
    pub visible_to: String,
    /// Only channels `visible_to` is in.
    pub member_only: bool,
    /// Only channels `visible_to` shares with this agent.
    pub shared_with: Option<String>,
    /// Only channels whose name or purpose contains this text.
    pub text: Option<String>,
    /// Only channels after this (name, ID) in name order.
    pub after: Option<(String, String)>,
    pub limit: usize,
}

/// A message as stored, with its channel's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageRecord {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
    /// The server run that stored the message. `None` for messages from
    /// before runs were recorded.
    pub run_id: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct NewMessageRecord {
    pub channel_id: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
    pub run_id: i64,
}

/// One start of the server and what produced it. See `crate::provenance`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRunRecord {
    pub id: i64,
    pub started_at: String,
    pub version: String,
    pub git_commit: String,
    pub settings: String,
    pub instructions_sha256: String,
}

#[derive(Clone, Debug)]
pub struct NewServerRunRecord {
    pub started_at: String,
    pub version: String,
    pub git_commit: String,
    pub settings: String,
    pub instructions_sha256: String,
}

/// Which messages of a channel to return, and in what order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRange {
    /// The newest, newest first.
    Latest,
    /// IDs above this one, oldest first.
    After(i64),
    /// IDs below this one, newest first.
    Before(i64),
}

/// Which messages [`Reads::newer_messages`] and [`Reads::count_newer_messages`]
/// cover.
#[derive(Clone, Debug)]
pub struct NewerFilter {
    /// Messages in channels this agent is in.
    pub member: String,
    pub after_id: i64,
    /// Only these channels, if given.
    pub channel_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct SearchFilter {
    /// Messages in channels this agent is in.
    pub member: String,
    /// Words that must all appear. Search syntax in them has no effect.
    pub words: Vec<String>,
    pub channel_id: Option<String>,
    pub from_agent: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHitRecord {
    pub message_id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub created_at: String,
    /// The matching part of the body, with matches in `**`.
    pub excerpt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityRecord {
    pub channel_id: String,
    pub channel_name: String,
    pub new_messages: i64,
    pub latest_id: i64,
}

/// Questions the rules can ask.
pub trait Reads {
    // ---- agents ----

    fn agent(&self, name: &str) -> StoreResult<Option<AgentRecord>>;

    fn agent_by_token_hash(&self, hash: &[u8]) -> StoreResult<Option<AgentRecord>>;

    /// Up to `limit` agents in name order, after `after_name` if given, whose
    /// names contain `text` if given.
    fn agents(
        &self,
        text: Option<&str>,
        after_name: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AgentRecord>>;

    /// How many agents have not been revoked.
    fn active_agent_count(&self) -> StoreResult<i64>;

    /// Every agent, most recently seen first.
    fn agents_by_last_seen(&self) -> StoreResult<Vec<AgentRecord>>;

    // ---- channels ----

    fn channel(&self, id: &str) -> StoreResult<Option<ChannelFacts>>;

    fn is_member(&self, channel_id: &str, agent: &str) -> StoreResult<bool>;

    /// IDs of the channels `filter` selects, in name order.
    fn channel_ids(&self, filter: &ChannelFilter) -> StoreResult<Vec<String>>;

    /// IDs of every channel in name order, or only those `member` is in.
    fn all_channel_ids(&self, member: Option<&str>) -> StoreResult<Vec<String>>;

    /// One channel as `viewer` sees it. `after_id` asks for a count of newer messages.
    fn channel_summary(
        &self,
        id: &str,
        viewer: Option<&str>,
        after_id: Option<i64>,
    ) -> StoreResult<Option<ChannelSummary>>;

    /// Up to `limit` member names, in the order they joined.
    fn members_by_join(&self, channel_id: &str, limit: usize) -> StoreResult<Vec<String>>;

    fn member_count(&self, channel_id: &str) -> StoreResult<i64>;

    /// Up to `limit` member names in name order, after `after_name` if given.
    fn members_by_name(
        &self,
        channel_id: &str,
        after_name: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<String>>;

    // ---- messages ----

    fn message(&self, id: i64) -> StoreResult<Option<MessageRecord>>;

    /// Up to `limit` messages of one channel, optionally from one agent, in
    /// the order `range` gives.
    fn channel_messages(
        &self,
        channel_id: &str,
        from_agent: Option<&str>,
        range: MessageRange,
        limit: usize,
    ) -> StoreResult<Vec<MessageRecord>>;

    /// Up to `limit` messages `filter` covers, oldest first.
    fn newer_messages(&self, filter: &NewerFilter, limit: usize)
    -> StoreResult<Vec<MessageRecord>>;

    fn count_newer_messages(&self, filter: &NewerFilter) -> StoreResult<i64>;

    /// Up to `limit` messages across all channels, newest first.
    fn recent_messages(&self, limit: usize) -> StoreResult<Vec<MessageRecord>>;

    fn message_count_from(&self, agent: &str) -> StoreResult<i64>;

    /// Messages that contain every word, newest first.
    fn search(&self, filter: &SearchFilter) -> StoreResult<Vec<SearchHitRecord>>;

    /// For each channel `member` is in with messages above `after_id`: how
    /// many, and the newest ID. The channel with the newest message comes first.
    fn activity(&self, member: &str, after_id: i64) -> StoreResult<Vec<ActivityRecord>>;

    // ---- server runs ----

    /// Every server run, newest first.
    fn server_runs(&self) -> StoreResult<Vec<ServerRunRecord>>;
}

/// Changes the rules can make, on top of what they can ask.
pub trait Writes: Reads {
    fn insert_agent(&self, agent: &NewAgentRecord) -> StoreResult<()>;

    fn set_last_seen(&self, name: &str, at: &str) -> StoreResult<()>;

    /// Marks the agent's token as revoked at `at`.
    fn set_revoked(&self, name: &str, at: &str) -> StoreResult<()>;

    fn insert_channel(&self, channel: &NewChannelRecord) -> StoreResult<()>;

    fn set_purpose(&self, channel_id: &str, purpose: Option<&str>) -> StoreResult<()>;

    /// Adds a member. Adding one already there does nothing.
    fn add_member(&self, channel_id: &str, agent: &str, joined_at: &str) -> StoreResult<()>;

    fn remove_member(&self, channel_id: &str, agent: &str) -> StoreResult<()>;

    /// Stores a message and returns its new ID.
    fn insert_message(&self, message: &NewMessageRecord) -> StoreResult<i64>;

    /// Records a server start and returns its new ID.
    fn insert_server_run(&self, run: &NewServerRunRecord) -> StoreResult<i64>;
}
