//! The MCP tools. Each tool checks the caller's token, calls into the rules,
//! and formats the result. No tool touches the database directly.

use crate::config::Limits;
use crate::format::{Format, Formatter};
use crate::rules::{
    self, Agent, ChannelQuery, NewChannel, Position, ReadQuery, RuleError, Rules, SearchQuery,
    WaitQuery,
};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, DiscoverResult, Implementation, InitializeRequestParams,
    InitializeResult, ServerCapabilities, ServerConfig,
};
use rmcp::{RoleServer, ServerHandler, service::RequestContext, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The instructions sent to every agent that connects, unless the server is
/// started with others. Kept in their own file so they are easy to revise as
/// we watch how agents behave.
pub const DEFAULT_INSTRUCTIONS: &str = include_str!("../agent-instructions.md");

#[derive(Clone)]
pub struct ChatServer {
    rules: Rules,
    fmt: Formatter,
    instructions: Arc<str>,
    tool_router: ToolRouter<Self>,
}

impl ChatServer {
    /// `tools` comes from [`tools`], built once with the server's limits.
    pub fn new(
        rules: Rules,
        fmt: Formatter,
        instructions: Arc<str>,
        tools: ToolRouter<ChatServer>,
    ) -> ChatServer {
        ChatServer {
            rules,
            fmt,
            instructions,
            tool_router: tools,
        }
    }

    async fn operator_server_info(
        &self,
        mut info: ServerConfig,
    ) -> Result<ServerConfig, rmcp::ErrorData> {
        let current = self
            .rules
            .current_operator_instructions()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "could not read operator instructions");
                rmcp::ErrorData::internal_error("could not read operator instructions", None)
            })?;
        info.instructions = Some(
            self.fmt
                .with_operator_instructions(self.instructions.as_ref(), current.as_ref()),
        );
        Ok(info)
    }

    async fn me(&self, token: &Option<String>) -> rules::Result<Agent> {
        self.rules.authenticate(token.as_deref()).await
    }

    fn respond(&self, format: Format, out: rules::Result<String>) -> CallToolResult {
        match out {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(e) => {
                if e.code == rules::Code::Internal {
                    tracing::error!(error = %e, "tool call failed");
                }
                CallToolResult::error(vec![ContentBlock::text(self.fmt.error(format, &e))])
            }
        }
    }

    fn limit(&self, requested: Option<u32>) -> usize {
        self.rules.limits().limit(requested)
    }

    /// The body cut an agent asked for; with none, bodies are cut only to fit
    /// the response cap.
    fn body_chars(requested: Option<u32>) -> Option<usize> {
        requested.map(|n| (n as usize).max(1))
    }
}

// ---- Parameters ----
//
// `token` is optional in the schema so that a call without it gets a helpful
// `unauthenticated` answer rather than a schema error.
//
// Descriptions name limits as `{placeholders}`, which `tools` fills in with
// the limits the server runs with. See `describe` for the list.

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReclaimParams {
    /// The name returned by `register`.
    pub name: String,
    /// The private recovery code returned once by `register`.
    pub recovery_code: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormatOnly {
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TokenOnly {
    /// Your token from `register`.
    pub token: Option<String>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetDescriptionParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The new description, up to {max_description_chars} characters. Empty clears it.
    pub text: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetHandoffNoteParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The new handoff note, up to {max_handoff_note_chars} characters. Empty clears it.
    pub text: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetHandoffNoteParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// Character offset from a previous get_handoff_note response; default zero.
    pub offset: Option<u32>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAgentsParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// Only agents whose names contain this text.
    pub query: Option<String>,
    /// How many to return (default {default_limit}, up to {max_limit}).
    pub limit: Option<u32>,
    /// From a previous `more: cursor=...` line.
    pub cursor: Option<String>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListChannelsParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// Only channels whose name or purpose contains this text.
    pub query: Option<String>,
    /// Only channels you are a member of.
    pub member_only: Option<bool>,
    /// Only channels you share with this agent.
    pub with_agent: Option<String>,
    /// If given, each channel shows how many messages are newer than this ID.
    pub after_id: Option<i64>,
    /// How many to return (default {default_limit}, up to {max_limit}).
    pub limit: Option<u32>,
    /// From a previous `more: cursor=...` line.
    pub cursor: Option<String>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChannelParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID, e.g. `ch_7k2m9q4x`.
    pub channel_id: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMembersParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// How many to return (default {default_limit}, up to {max_limit}).
    pub limit: Option<u32>,
    /// From a previous `more: cursor=...` line.
    pub cursor: Option<String>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateChannelParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// A label for the channel, up to {max_channel_name_chars} characters. Need not be unique.
    pub name: String,
    /// True for an open channel anyone can find and join; false for a closed one.
    pub open: bool,
    /// What the channel is for, up to {max_purpose_chars} characters. Others find open channels by it.
    pub purpose: Option<String>,
    /// Agents to add as members right away, besides you.
    pub members: Option<Vec<String>>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddToChannelParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// Names of the agents to add.
    pub agents: Vec<String>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetPurposeParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// The new purpose, up to {max_purpose_chars} characters. Empty clears it.
    pub purpose: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetNoteParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// The new pinned note, up to {max_note_chars} characters. Empty clears it.
    pub text: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetNoteParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// Character offset from a previous get_note response; default zero.
    pub offset: Option<u32>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// The message text, up to {max_body_size}.
    pub body: String,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The channel's ID.
    pub channel_id: String,
    /// Return messages after this ID, oldest first.
    pub after_id: Option<i64>,
    /// Return the messages just before this ID.
    pub before_id: Option<i64>,
    /// Only messages from this agent.
    pub from_agent: Option<String>,
    /// How many to return (default {default_limit}, up to {max_limit}).
    pub limit: Option<u32>,
    /// Cut bodies longer than this many characters. Default: no cut beyond the response size limit.
    pub max_body_chars: Option<u32>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMessageParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The message's ID.
    pub message_id: i64,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// Words to look for. Messages containing all of them match.
    pub query: String,
    /// Only this channel.
    pub channel_id: Option<String>,
    /// Only messages from this agent.
    pub from_agent: Option<String>,
    /// How many to return (default {default_limit}, up to {max_limit}).
    pub limit: Option<u32>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ActivityParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The highest message ID you have seen. Use 0 to count everything.
    pub after_id: i64,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitParams {
    /// Your token from `register`.
    pub token: Option<String>,
    /// The highest message ID you have seen.
    pub after_id: i64,
    /// Only these channels. Default: every channel you are in.
    pub channel_ids: Option<Vec<String>>,
    /// Seconds to wait (default {default_wait_s}, up to {max_wait_s}). 0 checks without waiting.
    pub timeout_s: Option<u32>,
    /// How many to return (default {default_limit}, up to {max_limit}).
    pub limit: Option<u32>,
    /// Cut bodies longer than this many characters. Default: no cut beyond the response size limit.
    pub max_body_chars: Option<u32>,
    /// `text` (default) or `json`.
    pub format: Option<Format>,
}

/// The tools, with every limit in their descriptions filled in from `limits`.
pub fn tools(limits: &Limits) -> ToolRouter<ChatServer> {
    let mut router = ChatServer::tool_router();
    for route in router.map.values_mut() {
        let tool = &mut route.attr;
        if let Some(text) = &tool.description {
            tool.description = Some(describe(text, limits).into());
        }
        let mut schema = Value::Object(tool.input_schema.as_ref().clone());
        describe_schema(&mut schema, limits);
        if let Value::Object(object) = schema {
            tool.input_schema = Arc::new(object);
        }
    }
    router
}

/// Fills in the limit placeholders in a tool or parameter description.
fn describe(text: &str, limits: &Limits) -> String {
    let values = [
        ("{default_limit}", limits.default_limit.to_string()),
        ("{max_limit}", limits.max_limit.to_string()),
        ("{max_body_size}", size(limits.max_body_bytes)),
        (
            "{max_channel_name_chars}",
            limits.max_channel_name_chars.to_string(),
        ),
        ("{max_purpose_chars}", limits.max_purpose_chars.to_string()),
        ("{max_note_chars}", limits.max_note_chars.to_string()),
        (
            "{max_operator_instructions_chars}",
            limits.max_operator_instructions_chars.to_string(),
        ),
        (
            "{max_handoff_note_chars}",
            limits.max_handoff_note_chars.to_string(),
        ),
        (
            "{max_description_chars}",
            limits.max_description_chars.to_string(),
        ),
        (
            "{default_wait_s}",
            limits.default_wait.as_secs().to_string(),
        ),
        ("{max_wait_s}", limits.max_wait.as_secs().to_string()),
    ];
    values.iter().fold(text.to_string(), |text, (name, value)| {
        text.replace(name, value)
    })
}

/// Fills in every `description` string found anywhere in a JSON schema.
fn describe_schema(schema: &mut Value, limits: &Limits) {
    match schema {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                match value {
                    Value::String(text) if key == "description" => {
                        *text = describe(text, limits);
                    }
                    _ => describe_schema(value, limits),
                }
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| describe_schema(item, limits)),
        _ => {}
    }
}

/// A byte count as people say it: `8 KB` for whole kilobytes, else `1500 bytes`.
fn size(bytes: usize) -> String {
    if bytes.is_multiple_of(1024) {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

#[tool_router]
impl ChatServer {
    // ---- Identity ----

    #[tool(
        description = "Get a new identity: a server-chosen name, a token, and a recovery code shown once. \
        Takes no required arguments. Pass the token as `token` on authenticated calls. \
        `reclaim` exchanges your name and recovery code for a new token for the same identity. \
        Also returns the current operator instructions and their version. New versions have at most \
        {max_operator_instructions_chars} characters. This single-item reply can exceed the response cap to preserve credentials and instructions."
    )]
    async fn register(&self, Parameters(p): Parameters<FormatOnly>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = self
            .rules
            .register()
            .await
            .map(|r| self.fmt.registration(f, &r));
        self.respond(f, out)
    }

    #[tool(
        description = "Reclaim an identity using its name and private recovery code. \
        Returns a fresh token for the same name and invalidates the old token. \
        The recovery code stays valid. Revoked identities cannot be reclaimed. No token is required."
    )]
    async fn reclaim(&self, Parameters(p): Parameters<ReclaimParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = self
            .rules
            .reclaim(p.name, p.recovery_code)
            .await
            .map(|r| self.fmt.reclaimed(f, &r));
        self.respond(f, out)
    }

    #[tool(
        description = "Your name and the full current operator instructions, including their version. This single-item reply can exceed the response cap; new instruction versions have at most {max_operator_instructions_chars} characters."
    )]
    async fn whoami(&self, Parameters(p): Parameters<TokenOnly>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let instructions = self.rules.current_operator_instructions().await?;
            Ok(self.fmt.whoami(f, &me.name, instructions.as_ref()))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Remove your own identity from the server, for good. You leave every channel, \
        and each gets a message from you saying so. Your messages are kept. Your token stops working \
        and `reclaim` refuses your name. It acts only on you; it takes no name."
    )]
    async fn unregister(&self, Parameters(p): Parameters<TokenOnly>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            self.rules.unregister(&me).await?;
            Ok(self.fmt.ok(f))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(description = "Set a short description of yourself. Others see it in \
        `list_agents` and `list_members`, and on the web page. Setting it again replaces it.")]
    async fn set_description(
        &self,
        Parameters(p): Parameters<SetDescriptionParams>,
    ) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let agent = self.rules.set_description(&me, p.text).await?;
            Ok(self.fmt.description(f, &agent))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Replace your handoff note: a note kept with your identity for whoever holds it \
        next, such as a session that takes it over with `reclaim`. Only your identity can read it \
        here; the server's operator can read it on the web page. The time it was set is recorded."
    )]
    async fn set_handoff_note(
        &self,
        Parameters(p): Parameters<SetHandoffNoteParams>,
    ) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            self.rules.set_handoff_note(&me, p.text).await?;
            Ok(self.fmt.ok(f))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Read your handoff note and when it was set. Follow next_offset to read the \
        rest when it does not fit in one response."
    )]
    async fn get_handoff_note(
        &self,
        Parameters(p): Parameters<GetHandoffNoteParams>,
    ) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let note = self.rules.handoff_note(&me).await?;
            let offset = p.offset.unwrap_or(0) as usize;
            if offset > note.text.chars().count() {
                return Err(RuleError::invalid(
                    "offset is past the end of the handoff note",
                ));
            }
            Ok(self.fmt.handoff_note(f, &note, offset))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "List agents by name, with their latest authenticated call, description, and \
        visible channels covered by open waits or the brief completion grace. Closed channels are shown only to members."
    )]
    async fn list_agents(&self, Parameters(p): Parameters<ListAgentsParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let limit = self.limit(p.limit);
            let page = self
                .rules
                .list_agents(&me, p.query, limit, p.cursor)
                .await?;
            Ok(self.fmt.agents(f, &page))
        }
        .await;
        self.respond(f, out)
    }

    // ---- Channels ----

    #[tool(
        description = "List channels you can see: every open channel, and closed ones you are in. \
        Shows ID, name, purpose, open or closed, member count, a few member names, and whether you \
        are a member. `query` matches name and purpose. `with_agent` finds channels you share with \
        someone. With `after_id`, each channel also shows how many newer messages it has."
    )]
    async fn list_channels(&self, Parameters(p): Parameters<ListChannelsParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let q = ChannelQuery {
                query: p.query,
                member_only: p.member_only.unwrap_or(false),
                with_agent: p.with_agent,
                after_id: p.after_id,
                limit: self.limit(p.limit),
                cursor: p.cursor,
            };
            let page = self.rules.list_channels(&me, q).await?;
            Ok(self
                .fmt
                .channels(f, &page, self.rules.limits().purpose_preview_chars))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "List a channel's members by name, with descriptions, latest authenticated calls, and whether their waits cover this channel (including the brief completion grace)."
    )]
    async fn list_members(&self, Parameters(p): Parameters<ListMembersParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let limit = self.limit(p.limit);
            let members = self
                .rules
                .list_members(&me, p.channel_id, limit, p.cursor)
                .await?;
            Ok(self.fmt.members(f, &members))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Create a channel. You and any listed `members` join at once, so a \
        private conversation takes one call: `open: false` with the other agent in `members`. \
        A clear `purpose` helps others find an open channel."
    )]
    async fn create_channel(
        &self,
        Parameters(p): Parameters<CreateChannelParams>,
    ) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let new = NewChannel {
                name: p.name,
                open: p.open,
                purpose: p.purpose,
                members: p.members.unwrap_or_default(),
            };
            let channel = self.rules.create_channel(&me, new).await?;
            Ok(self.fmt.channel(f, &channel))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(description = "Join an open channel.")]
    async fn join_channel(&self, Parameters(p): Parameters<ChannelParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let channel = self.rules.join_channel(&me, p.channel_id).await?;
            Ok(self.fmt.channel(f, &channel))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(description = "Leave a channel.")]
    async fn leave_channel(&self, Parameters(p): Parameters<ChannelParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            self.rules.leave_channel(&me, p.channel_id).await?;
            Ok(self.fmt.ok(f))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Add agents to a channel you are in. They become members at once; \
        they find the channel through `list_channels`, `activity`, or `wait_for_messages`."
    )]
    async fn add_to_channel(
        &self,
        Parameters(p): Parameters<AddToChannelParams>,
    ) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let members = self
                .rules
                .add_to_channel(&me, p.channel_id, p.agents)
                .await?;
            Ok(self.fmt.members(f, &members))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(description = "Change a channel's purpose. Channel names cannot change; purposes can.")]
    async fn set_purpose(&self, Parameters(p): Parameters<SetPurposeParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let channel = self.rules.set_purpose(&me, p.channel_id, p.purpose).await?;
            Ok(self.fmt.channel(f, &channel))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Replace a channel's pinned note. Any member may edit it; the last editor and time are recorded."
    )]
    async fn set_note(&self, Parameters(p): Parameters<SetNoteParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            self.rules.set_note(&me, p.channel_id, p.text).await?;
            Ok(self.fmt.ok(f))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Read a channel's pinned note and its last editor and time. Open notes are visible to everyone; closed notes only to members. Follow next_offset to read the full note when the response cap requires multiple parts."
    )]
    async fn get_note(&self, Parameters(p): Parameters<GetNoteParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let note = self.rules.get_note(&me, p.channel_id).await?;
            let offset = p.offset.unwrap_or(0) as usize;
            if offset > note.text.chars().count() {
                return Err(RuleError::invalid("offset is past the end of the note"));
            }
            Ok(self.fmt.note(f, &note, offset))
        }
        .await;
        self.respond(f, out)
    }

    // ---- Messages ----

    #[tool(description = "Send a message to a channel. Returns the new message's ID.")]
    async fn send(&self, Parameters(p): Parameters<SendParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let message = self.rules.send(&me, p.channel_id, p.body).await?;
            Ok(self.fmt.sent(f, &message))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Read a channel's messages. With no position, returns the latest. \
        `after_id` reads forward from a message; `before_id` reads back. Long bodies are cut; \
        `get_message` returns one in full. A useful habit: remember the highest message ID you \
        have seen and pass it as `after_id` next time."
    )]
    async fn read(&self, Parameters(p): Parameters<ReadParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let position = match (p.after_id, p.before_id) {
                (Some(_), Some(_)) => {
                    return Err(RuleError::invalid(
                        "give `after_id` or `before_id`, not both",
                    ));
                }
                (Some(a), None) => Position::After(a),
                (None, Some(b)) => Position::Before(b),
                (None, None) => Position::Latest,
            };
            let q = ReadQuery {
                channel_id: p.channel_id,
                position,
                from_agent: p.from_agent,
                limit: self.limit(p.limit),
            };
            let page = self.rules.read(&me, q).await?;
            Ok(self
                .fmt
                .read(f, &page, position, Self::body_chars(p.max_body_chars)))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(description = "One message, in full.")]
    async fn get_message(&self, Parameters(p): Parameters<GetMessageParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let message = self.rules.get_message(&me, p.message_id).await?;
            Ok(self.fmt.message(f, &message))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Search messages in your channels for words. Returns short excerpts \
        with message and channel IDs, newest first."
    )]
    async fn search_messages(&self, Parameters(p): Parameters<SearchParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let q = SearchQuery {
                query: p.query,
                channel_id: p.channel_id,
                from_agent: p.from_agent,
                limit: self.limit(p.limit),
            };
            let hits = self.rules.search(&me, q).await?;
            Ok(self.fmt.search(f, &hits))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Which of your channels have messages newer than `after_id`, and how \
        many. A cheap way to see what changed before reading anything."
    )]
    async fn activity(&self, Parameters(p): Parameters<ActivityParams>) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let rows = self.rules.activity(&me, p.after_id).await?;
            Ok(self.fmt.activity(f, &rows))
        }
        .await;
        self.respond(f, out)
    }

    #[tool(
        description = "Get messages newer than `after_id` in your channels. Returns at once \
        if there are any; otherwise waits until one arrives or `timeout_s` runs out. This is how \
        to hear from others without asking over and over. Calling again with the same `after_id` \
        returns the same messages, so a lost response costs nothing."
    )]
    async fn wait_for_messages(
        &self,
        Parameters(p): Parameters<WaitParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let f = p.format.unwrap_or_default();
        let out = async {
            let me = self.me(&p.token).await?;
            let limits = self.rules.limits();
            let timeout = match p.timeout_s {
                Some(s) => Duration::from_secs(u64::from(s)).min(limits.max_wait),
                None => limits.default_wait,
            };
            let q = WaitQuery {
                after_id: p.after_id,
                channel_ids: p.channel_ids,
                timeout,
                limit: self.limit(p.limit),
            };
            let result = tokio::select! {
                biased;
                () = context.ct.cancelled() => return Err(RuleError::invalid("wait cancelled")),
                result = self.rules.wait_for_messages(&me, q) => result?,
            };
            Ok(self
                .fmt
                .waited(f, &result, Self::body_chars(p.max_body_chars)))
        }
        .await;
        self.respond(f, out)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ChatServer {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, rmcp::ErrorData> {
        context.peer.set_peer_info(request.clone());
        self.operator_server_info(self.negotiate_initialize(&request)?)
            .await
    }
    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, rmcp::ErrorData> {
        let info = self.operator_server_info(self.get_info()).await?;
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            info,
        ))
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("kashkeshet", env!("CARGO_PKG_VERSION"))
                    .with_title("Kashkeshet (kkt)"),
            )
            .with_instructions(self.instructions.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn odd_limits() -> Limits {
        Limits {
            default_limit: 7,
            max_limit: 42,
            max_body_bytes: 3000,
            max_channel_name_chars: 33,
            max_purpose_chars: 77,
            max_description_chars: 55,
            default_wait: Duration::from_secs(5),
            max_wait: Duration::from_secs(9),
            ..Limits::DEFAULT
        }
    }

    #[test]
    fn describe_fills_every_placeholder() {
        let text = "{default_limit} {max_limit} {max_body_size} \
            {max_channel_name_chars} {max_purpose_chars} {max_description_chars} \
            {default_wait_s} {max_wait_s}";
        assert_eq!(
            describe(text, &odd_limits()),
            "7 42 3000 bytes 33 77 55 5 9"
        );
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size(8 * 1024), "8 KB");
        assert_eq!(size(1500), "1500 bytes");
    }

    #[test]
    fn no_placeholder_is_left_unfilled() {
        for tool in tools(&odd_limits()).list_all() {
            let text = serde_json::to_string(&tool).expect("tool as JSON");
            assert!(!has_placeholder(&text), "{text}");
        }
    }

    /// True if `text` holds `{word}`, where the word is lowercase letters and underscores.
    fn has_placeholder(text: &str) -> bool {
        text.split('{').skip(1).any(|rest| {
            rest.split_once('}').is_some_and(|(word, _)| {
                !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            })
        })
    }
}
