//! What the server's operator can do from the command line, against the
//! database file directly, while a server may be running on it: list agents
//! and revoke an agent's token.
//!
//! A running server looks a token up in the database on every call and
//! keeps no copy, so a revocation takes effect on the agent's next call.

use super::*;

/// The operator's view of the agents. Needs only the database and a clock,
/// not a running server.
pub struct Operator {
    db: Db,
    clock: Arc<dyn Clock>,
}

/// One agent as the operator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStatus {
    pub name: String,
    pub created_at: String,
    pub last_seen: Option<String>,
    /// When its token was revoked, if it has been.
    pub revoked_at: Option<String>,
}

/// The result of a revocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revoked {
    /// Revoked now, at this time.
    Now { at: String },
    /// Already revoked earlier, at this time; nothing changed.
    Already { at: String },
}

impl Operator {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Operator {
        Operator { db, clock }
    }

    /// Every agent, most recently seen first, revoked ones included.
    pub async fn agents(&self) -> Result<Vec<AgentStatus>> {
        self.db
            .read(|store| {
                Ok(store
                    .agents_by_last_seen()?
                    .into_iter()
                    .map(|a| AgentStatus {
                        name: a.name,
                        created_at: a.created_at,
                        last_seen: a.last_seen,
                        revoked_at: a.revoked_at,
                    })
                    .collect())
            })
            .await
    }

    /// Revokes the token of the agent named `name`, so every later call with
    /// it is refused as `unauthenticated`. The agent's name, memberships and
    /// messages are kept. `not_found` if there is no such agent.
    pub async fn revoke(&self, name: String) -> Result<Revoked> {
        let now = clock::to_text(self.clock.now());
        self.db
            .write(move |store| {
                let agent = store
                    .agent(&name)?
                    .ok_or_else(|| RuleError::agent_not_found(&name))?;
                match agent.revoked_at {
                    Some(at) => Ok(Revoked::Already { at }),
                    None => {
                        store.set_revoked(&name, &now)?;
                        Ok(Revoked::Now { at: now })
                    }
                }
            })
            .await
    }
}
