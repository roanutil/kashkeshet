//! Every permission check and database change.
//!
//! Both the MCP tools and the web page go through this module. Nothing else
//! touches the database, and this module reaches it only through the
//! [`crate::store`] contract.
//!
//! The rules:
//!
//! | | Open channel | Closed channel |
//! |---|---|---|
//! | Appears in `list_channels` | To everyone | To members only |
//! | Join | Anyone | Only by being added |
//! | Add others | Any member | Any member |
//! | Read, send, set purpose | Members | Members |
//! | Leave | Any member | Any member |
//!
//! To anyone outside a closed channel, it does not exist: every call answers
//! `not_found`. Outside an open channel, calls that need membership answer
//! `not_member`.

mod access;
mod agents;
mod auth;
mod channels;
mod error;
mod human;
mod messages;
mod operator;
mod rate;

#[cfg(test)]
mod tests;

pub use agents::{Agent, AgentInfo, Registration};
pub use channels::{ChannelInfo, ChannelQuery, Members, NewChannel, channel_cursor};
pub use error::{Code, RuleError};
pub use human::ServerRunInfo;
pub use messages::{
    ActivityRow, Message, MessagePage, Position, ReadQuery, SearchHit, SearchQuery, WaitQuery,
    WaitResult,
};
pub use operator::{AgentStatus, Operator, Revoked};
pub use rate::{RegisterLimiter, SendLimiter};

use crate::clock::{self, Clock};
use crate::config::Limits;
use crate::events::Events;
use crate::names::{self, Entropy};
use crate::store::{
    ActivityRecord, AgentRecord, ChannelFacts, ChannelFilter, Db, MessageRange, MessageRecord,
    NewAgentRecord, NewChannelRecord, NewMessageRecord, NewerFilter, Reads, SearchFilter,
    SearchHitRecord, ServerRunRecord,
};
use access::{Decision, Need};
use serde::Serialize;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, RuleError>;

/// One page of a list, plus the cursor for the next page if there is one.
#[derive(Clone, Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Everything the rules depend on, supplied at startup.
pub struct Deps {
    pub db: Db,
    pub events: Events,
    pub clock: Arc<dyn Clock>,
    pub entropy: Arc<dyn Entropy>,
    /// Counts each agent's sends against `limits.sends_per_minute`.
    pub sends: SendLimiter,
    /// Counts registrations against `limits.registrations_per_minute`.
    pub registrations: RegisterLimiter,
    /// This server run, from `provenance::record`. New agents and messages
    /// are stamped with it.
    pub run_id: i64,
    pub limits: Limits,
}

#[derive(Clone)]
pub struct Rules {
    inner: Arc<Inner>,
}

struct Inner {
    deps: Deps,
}

impl Rules {
    pub fn new(deps: Deps) -> Rules {
        Rules {
            inner: Arc::new(Inner { deps }),
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.inner.deps.limits
    }

    pub fn events(&self) -> &Events {
        &self.inner.deps.events
    }

    fn db(&self) -> &Db {
        &self.inner.deps.db
    }

    fn now_text(&self) -> String {
        clock::to_text(self.inner.deps.clock.now())
    }
}

/// The channel, if `agent` can see it, and whether `agent` is in it.
fn visible_channel(store: &dyn Reads, agent: &str, id: &str) -> Result<(ChannelFacts, bool)> {
    channel_for(store, agent, id, Need::See)
}

/// The channel, if `agent` is a member of it.
fn member_channel(store: &dyn Reads, agent: &str, id: &str) -> Result<ChannelFacts> {
    channel_for(store, agent, id, Need::Membership).map(|(facts, _)| facts)
}

fn channel_for(
    store: &dyn Reads,
    agent: &str,
    id: &str,
    need: Need,
) -> Result<(ChannelFacts, bool)> {
    let facts = store.channel(id)?;
    let member = match &facts {
        Some(_) => store.is_member(id, agent)?,
        None => false,
    };
    let open = facts.as_ref().is_some_and(|f| f.open);
    match (access::decide(facts.is_some(), open, member, need), facts) {
        (Decision::Allowed, Some(facts)) => Ok((facts, member)),
        (Decision::NotMember, _) => Err(RuleError::not_member(id)),
        _ => Err(RuleError::channel_not_found(id)),
    }
}

fn agent_exists(store: &dyn Reads, name: &str) -> Result<bool> {
    Ok(store.agent(name)?.is_some())
}

/// Full details of one channel, as `viewer` sees it. `viewer` is `None` for the human.
fn channel_info(
    store: &dyn Reads,
    id: &str,
    viewer: Option<&str>,
    after_id: Option<i64>,
    preview: usize,
) -> Result<ChannelInfo> {
    let summary = store
        .channel_summary(id, viewer, after_id)?
        .ok_or_else(|| RuleError::channel_not_found(id))?;
    Ok(ChannelInfo {
        id: summary.id,
        name: summary.name,
        purpose: summary.purpose,
        open: summary.open,
        created_by: summary.created_by,
        created_at: summary.created_at,
        member_count: summary.member_count,
        members_preview: store.members_by_join(id, preview)?,
        is_member: summary.viewer_is_member,
        new_messages: summary.new_messages,
    })
}
