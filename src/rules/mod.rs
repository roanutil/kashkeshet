//! Every permission check and database change.
//!
//! Both the MCP tools and the web page go through this module. Nothing else
//! touches the database.
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

mod agents;
mod auth;
mod channels;
mod error;
mod human;
mod messages;
mod rate;

#[cfg(test)]
mod tests;

pub use agents::{Agent, AgentInfo, Registration};
pub use channels::{ChannelInfo, ChannelQuery, Members, NewChannel, channel_cursor};
pub use error::{Code, RuleError};
pub use human::{AgentView, ChannelView, Overview};
pub use messages::{
    ActivityRow, Message, MessagePage, Position, ReadQuery, SearchHit, SearchQuery, WaitQuery,
    WaitResult,
};
pub use rate::SendLimiter;

use crate::clock::{self, Clock};
use crate::config::Limits;
use crate::db::Db;
use crate::events::Events;
use crate::names::{self, Entropy};
use rusqlite::{Connection, OptionalExtension, named_params};
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

/// A channel's stored fields.
#[derive(Clone, Debug)]
struct ChannelRow {
    id: String,
    name: String,
    is_open: bool,
}

fn channel_row(conn: &Connection, id: &str) -> Result<Option<ChannelRow>> {
    conn.query_row(
        "SELECT id, name, is_open FROM channels WHERE id = ?",
        [id],
        |r| {
            Ok(ChannelRow {
                id: r.get(0)?,
                name: r.get(1)?,
                is_open: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(RuleError::from)
}

fn is_member(conn: &Connection, channel_id: &str, agent: &str) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM memberships WHERE channel_id = ? AND agent = ?",
            [channel_id, agent],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// The channel, if `agent` can see it. A closed channel the agent is not in
/// is reported as `not_found`, exactly as if it did not exist.
fn visible_channel(conn: &Connection, agent: &str, id: &str) -> Result<(ChannelRow, bool)> {
    let row = channel_row(conn, id)?.ok_or_else(|| RuleError::channel_not_found(id))?;
    let member = is_member(conn, id, agent)?;
    if !row.is_open && !member {
        return Err(RuleError::channel_not_found(id));
    }
    Ok((row, member))
}

/// The channel, if `agent` is a member of it.
fn member_channel(conn: &Connection, agent: &str, id: &str) -> Result<ChannelRow> {
    let (row, member) = visible_channel(conn, agent, id)?;
    if !member {
        return Err(RuleError::not_member(id));
    }
    Ok(row)
}

fn agent_exists(conn: &Connection, name: &str) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row("SELECT 1 FROM agents WHERE name = ?", [name], |r| r.get(0))
        .optional()?;
    Ok(found.is_some())
}

/// Full details of one channel, as `viewer` sees it. `viewer` is `None` for the human.
fn channel_info(
    conn: &Connection,
    id: &str,
    viewer: Option<&str>,
    after_id: Option<i64>,
    preview: usize,
) -> Result<ChannelInfo> {
    let mut info = conn.query_row(
        "SELECT c.id, c.name, c.purpose, c.is_open, c.created_by, c.created_at,
                (SELECT count(*) FROM memberships m WHERE m.channel_id = c.id),
                EXISTS (SELECT 1 FROM memberships m WHERE m.channel_id = c.id AND m.agent = :viewer),
                CASE WHEN :after IS NULL THEN NULL
                     ELSE (SELECT count(*) FROM messages g WHERE g.channel_id = c.id AND g.id > :after)
                END
         FROM channels c WHERE c.id = :id",
        named_params! { ":id": id, ":viewer": viewer, ":after": after_id },
        |r| {
            Ok(ChannelInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                purpose: r.get(2)?,
                open: r.get(3)?,
                created_by: r.get(4)?,
                created_at: r.get(5)?,
                member_count: r.get(6)?,
                members_preview: Vec::new(),
                is_member: r.get(7)?,
                new_messages: r.get(8)?,
            })
        },
    )?;
    info.members_preview = member_names(conn, id, preview)?;
    Ok(info)
}

/// Up to `limit` member names, in the order they joined.
fn member_names(conn: &Connection, channel_id: &str, limit: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT agent FROM memberships WHERE channel_id = ? ORDER BY joined_at, agent LIMIT ?",
    )?;
    let names = stmt
        .query_map(rusqlite::params![channel_id, limit as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(names)
}

const MESSAGE_COLUMNS: &str = "m.id, m.channel_id, c.name, m.agent, m.body, m.created_at";

fn message_from_row(r: &rusqlite::Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        channel_name: r.get(2)?,
        agent: r.get(3)?,
        body: r.get(4)?,
        created_at: r.get(5)?,
    })
}

/// Escapes `%`, `_` and `\` so text matches literally in a `LIKE ... ESCAPE '\'` pattern.
fn like_pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('%');
    for ch in text.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}
