//! What the web page needs, in its own terms: the reads its pages make and
//! the live feed ([`Source`]), and the sign-in check ([`Gate`]). Pages
//! depend only on these. The server provides them over the rules and
//! admission (`real`); the preview provides them from simulated data
//! (`preview`).

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::Serialize;
use std::time::Duration;

/// Where the pages get their data.
pub trait Source: Send + Sync {
    /// The front page: every channel, every agent, the latest messages, and
    /// the server runs.
    fn overview(&self) -> BoxFuture<'_, Result<Overview, SourceError>>;

    /// One channel with a page of its messages: the newest, or those just
    /// before `before_id`.
    fn channel(
        &self,
        id: String,
        before_id: Option<i64>,
    ) -> BoxFuture<'_, Result<ChannelView, SourceError>>;

    /// One agent and the channels it is in.
    fn agent(&self, name: String) -> BoxFuture<'_, Result<AgentView, SourceError>>;

    /// Everything that happens from now on, until the server stops.
    fn live(&self) -> BoxStream<'static, LiveEvent>;

    /// The most messages one page shows. [`Source::channel`] returns no more
    /// than this, and the live view keeps no more than this on the page.
    fn page_messages(&self) -> usize;
}

/// Whether a viewer may see the pages, and signing in and out.
pub trait Gate: Send + Sync {
    /// Whether viewers must sign in at all. When not, every page is open and
    /// there is no login form.
    fn sign_in_required(&self) -> bool;

    /// Whether the viewer with this session (if any) may see the pages.
    fn admits(&self, session_id: Option<&str>) -> Admit;

    /// Checks the key a viewer typed and, if it is right, starts a session.
    fn sign_in(&self, key: &str) -> SignIn;

    /// Ends a session.
    fn sign_out(&self, session_id: &str);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    Yes,
    /// No, for this reason.
    No(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    Started {
        session_id: String,
        lifetime: Duration,
    },
    /// The key was wrong, for this reason.
    Refused(String),
}

/// Why a read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// No such channel or agent.
    NotFound(String),
    /// The request made no sense, such as a malformed position.
    Invalid(String),
    /// Something broke. The page says so; the cause is logged.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub purpose: Option<String>,
    pub open: bool,
    pub created_by: String,
    pub created_at: String,
    pub member_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub created_at: String,
    pub last_seen: Option<String>,
    /// The server run that created the agent, if known.
    pub run_id: Option<i64>,
}

/// A message, as pages show it and as the live feed sends it (JSON).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
    /// The server run that stored the message, if known.
    pub run_id: Option<i64>,
}

/// One start of the server and what produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRun {
    pub id: i64,
    pub started_at: String,
    pub version: String,
    pub git_commit: String,
    pub settings: String,
    pub instructions_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overview {
    pub channels: Vec<ChannelSummary>,
    pub agents: Vec<AgentSummary>,
    /// Every server run, newest first.
    pub runs: Vec<ServerRun>,
    /// The source's time when this was read, in the stored form. The page
    /// measures "how long ago" from it, not from the viewer's clock.
    pub now: String,
}

/// A channel and how much it is used, for the front page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSummary {
    pub channel: Channel,
    pub message_count: i64,
    /// When the newest message was sent, and by whom.
    pub last_message_at: Option<String>,
    pub last_speaker: Option<String>,
    /// When each message in the last `WEB_ACTIVITY_WINDOW` was sent, oldest
    /// first.
    pub recent_message_times: Vec<String>,
}

/// An agent and the channels it is in, for the front page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSummary {
    pub agent: Agent,
    pub channels: Vec<ChannelLink>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelLink {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelView {
    pub channel: Channel,
    /// Members in the order they joined.
    pub members: Vec<String>,
    /// Oldest first.
    pub messages: Vec<Message>,
    /// Set when older messages exist: the `before_id` that reads them.
    pub older_before_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentView {
    pub agent: Agent,
    pub channels: Vec<Channel>,
    pub message_count: i64,
}

/// Something that happened, for the live feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveEvent {
    Message(Message),
    /// The feed fell behind and missed this many events. The page reloads.
    Missed(u64),
}
