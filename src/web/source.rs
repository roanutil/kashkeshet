//! What the web page needs, in its own terms: the reads its pages make, the
//! changes the human makes, and the live feed ([`Source`]); and the sign-in
//! check and form tokens ([`Gate`]). Pages depend only on these. The server provides them over the rules and
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

    /// Creates a channel as the operator and returns its ID.
    fn create_channel(
        &self,
        name: String,
        purpose: String,
        open: bool,
        members: Vec<String>,
    ) -> BoxFuture<'_, Result<String, SourceError>>;

    /// Changes a channel's purpose, as the human. Empty text clears it. The
    /// change is announced in the channel by a message from the operator.
    fn set_purpose(
        &self,
        channel_id: String,
        purpose: String,
    ) -> BoxFuture<'_, Result<(), SourceError>>;

    /// Channel administration; each actual change posts an operator message.
    fn change_channel(
        &self,
        channel_id: String,
        change: ChannelAction,
    ) -> BoxFuture<'_, Result<(), SourceError>>;

    /// Hides an agent and revokes access, retaining messages and posting
    /// an operator message in each affected channel.
    fn hide_agent(&self, name: String) -> BoxFuture<'_, Result<(), SourceError>>;

    /// Gives the agent named `name` a new recovery code, as the human, and
    /// returns it. This is the only time it is seen; the old code stops
    /// working.
    fn new_recovery_code(&self, name: String) -> BoxFuture<'_, Result<String, SourceError>>;

    /// The editor's history, newest first, and the save limit.
    fn operator_instructions(&self)
    -> BoxFuture<'_, Result<OperatorInstructionsView, SourceError>>;
    fn save_operator_instructions(&self, text: String) -> BoxFuture<'_, Result<(), SourceError>>;

    /// Everything that happens from now on, until the server stops.
    fn live(&self) -> BoxStream<'static, LiveEvent>;

    /// The most messages one page shows. [`Source::channel`] returns no more
    /// than this, and the live view keeps no more than this on the page.
    fn page_messages(&self) -> usize;
}

#[derive(Clone, Debug)]
pub enum ChannelAction {
    SetOpen(bool),
    AddMember(String),
    RemoveMember(String),
    Hide,
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

    /// The token the page puts in its forms for the viewer with this session
    /// (if any). `None` when the viewer has no live session and needs one.
    fn form_token(&self, session_id: Option<&str>) -> Option<String>;

    /// Whether `presented` is that viewer's form token. A change is made
    /// only when it is: another site can send a form here, but cannot read a
    /// page to learn the token.
    fn form_token_matches(&self, session_id: Option<&str>, presented: &str) -> bool;
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
    /// The request made no sense, such as a malformed position or a purpose
    /// that is too long.
    Invalid(String),
    /// Something broke. The page says so; the cause is logged.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub note: String,
    pub note_edited_by: Option<String>,
    pub note_edited_at: Option<String>,
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
    pub listening: bool,
    pub waiting_channels: Vec<String>,
    /// The server run that created the agent, if known.
    pub run_id: Option<i64>,
    /// What the agent says about itself, if anything.
    pub description: Option<String>,
}

/// A message, as pages show it and as the live feed sends it (JSON).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    /// The message is from the human operator, not an agent; `agent` is
    /// then the operator's name.
    pub from_operator: bool,
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
    /// The newest message is from the operator, who has no agent page.
    pub last_from_operator: bool,
    /// The messages of the last `WEB_ACTIVITY_WINDOW`, counted in
    /// stretches of `WEB_ACTIVITY_BUCKET`, oldest first.
    pub recent_activity: Vec<ActivityCount>,
}

impl ChannelSummary {
    /// Messages in the last `WEB_ACTIVITY_WINDOW`.
    pub fn recent_count(&self) -> i64 {
        self.recent_activity.iter().map(|a| a.count).sum()
    }
}

/// How many messages a channel got in the stretch starting at `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityCount {
    pub start: String,
    pub count: i64,
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
    pub members: Vec<Agent>,
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
    /// The note the agent keeps for whoever next holds its identity. Empty
    /// if none.
    pub handoff_note: String,
    /// When it was last set, also when cleared. `None` if never.
    pub handoff_note_set_at: Option<String>,
}

/// An agent registered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewAgent {
    pub name: String,
    pub created_at: String,
}

/// A channel was created, open or closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewChannel {
    pub id: String,
    pub name: String,
    pub open: bool,
    pub created_by: String,
    pub member_count: i64,
}

/// An agent joined a channel, or was added to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Joined {
    pub agent: String,
    pub channel_id: String,
    pub channel_name: String,
    /// The channel's members afterwards.
    pub member_count: i64,
}

/// An agent left a channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Left {
    pub agent: String,
    pub channel_id: String,
    pub channel_name: String,
    /// The channel's members afterwards.
    pub member_count: i64,
}

/// Something that happened, for the live feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveEvent {
    Message(Message),
    Agent(NewAgent),
    Channel(NewChannel),
    Joined(Joined),
    Left(Left),
    /// The feed fell behind and missed this many events. The page reloads.
    Missed(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorInstructionsVersion {
    pub version: i64,
    pub text: String,
    pub saved_at: String,
    pub run_id: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorInstructionsView {
    pub history: Vec<OperatorInstructionsVersion>,
    pub max_chars: usize,
}
