use super::*;
use crate::config::MAX_NAME_ATTEMPTS;

impl Rules {
    /// Creates a new agent with a server-chosen name and a fresh token.
    /// Only hashes of the token and recovery code are stored.
    /// Refused when the server is over `registrations_per_minute` or already
    /// has `max_agents` agents that are not revoked.
    pub async fn register(&self) -> Result<Registration> {
        let now = self.inner.deps.clock.now();
        if !self.inner.deps.registrations.allow(now) {
            return Err(RuleError::new(
                Code::RateLimited,
                format!(
                    "this server accepts at most {} registrations a minute; wait and try again",
                    self.limits().registrations_per_minute
                ),
            ));
        }
        let registered = self.insert_new_agent().await;
        if registered.is_err() {
            self.inner.deps.registrations.refund();
        }
        registered
    }

    async fn insert_new_agent(&self) -> Result<Registration> {
        let max_agents = self.limits().max_agents;
        let entropy = self.inner.deps.entropy.clone();
        let base = names::agent_name(entropy.as_ref());
        let token = auth::token(entropy.as_ref());
        let hash = auth::hash_token(&token);
        let recovery_code = auth::recovery_code(entropy.as_ref());
        let recovery_hash = auth::hash_token(&recovery_code);
        let now = self.now_text();
        let created_at = now.clone();
        let run_id = self.inner.deps.run_id;
        let name = self
            .db()
            .write(move |store| {
                // A cap too large for the database's count type is no cap.
                let cap = i64::try_from(max_agents).unwrap_or(i64::MAX);
                if store.active_agent_count()? >= cap {
                    return Err(RuleError::new(
                        Code::LimitReached,
                        format!(
                            "this server already has its limit of {max_agents} agents; \
                             ask its operator to revoke unused ones"
                        ),
                    ));
                }
                let name = free_name(store, &base)?;
                store.insert_agent(&NewAgentRecord {
                    name: name.clone(),
                    token_hash: hash,
                    recovery_hash: Some(recovery_hash),
                    created_at: now.clone(),
                    last_seen: Some(now),
                    run_id,
                })?;
                Ok::<_, RuleError>(name)
            })
            .await?;
        self.events().publish_agent(NewAgent {
            name: name.clone(),
            created_at,
        });
        Ok(Registration {
            name,
            token,
            recovery_code,
        })
    }

    /// Exchanges a recovery code for a fresh token for the same identity.
    /// A revoked agent cannot recover. The recovery code remains valid.
    pub async fn reclaim(&self, name: String, recovery_code: String) -> Result<Reclaimed> {
        let recovery_hash = auth::hash_token(&recovery_code);
        let token = auth::token(self.inner.deps.entropy.as_ref());
        let token_hash = auth::hash_token(&token);
        let now = self.now_text();
        let name = self
            .db()
            .write(move |store| {
                let found = store.agent_by_recovery_hash(&name, &recovery_hash)?;
                let Some(AgentRecord {
                    revoked_at: None, ..
                }) = found
                else {
                    return Err(RuleError::new(
                        Code::Unauthenticated,
                        "unknown name or recovery code, or identity unavailable",
                    ));
                };
                store.set_token_hash(&name, &token_hash)?;
                store.set_last_seen(&name, &now)?;
                Ok::<_, RuleError>(name)
            })
            .await?;
        Ok(Reclaimed { name, token })
    }

    /// Agents in name order, optionally only those whose names contain `query`.
    pub async fn list_agents(
        &self,
        _me: &Agent,
        query: Option<String>,
        limit: usize,
        cursor: Option<String>,
    ) -> Result<Page<AgentInfo>> {
        self.db()
            .read(move |store| {
                let mut items: Vec<AgentInfo> = store
                    .agents(query.as_deref(), cursor.as_deref(), limit + 1)?
                    .into_iter()
                    .map(AgentInfo::from)
                    .collect();
                let next_cursor = if items.len() > limit {
                    items.truncate(limit);
                    items.last().map(|a| a.name.clone())
                } else {
                    None
                };
                Ok(Page { items, next_cursor })
            })
            .await
    }
}

/// `base` if no agent has it, else the first free numbered form of it.
fn free_name(store: &dyn Reads, base: &str) -> Result<String> {
    if !agent_exists(store, base)? {
        return Ok(base.to_string());
    }
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let candidate = names::numbered_name(base, attempt);
        if !agent_exists(store, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(RuleError::new(
        Code::Internal,
        format!("could not find a free name based on {base}"),
    ))
}

/// An agent that has proven who it is on this call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Registration {
    pub name: String,
    pub token: String,
    pub recovery_code: String,
}

/// A new authentication token for an existing identity.
#[derive(Clone, Debug, Serialize)]
pub struct Reclaimed {
    pub name: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub created_at: String,
    pub last_seen: Option<String>,
    /// The server run that created the agent, if it was recorded.
    pub run_id: Option<i64>,
}

// Agents see revoked agents like any other: their names and messages stay.
impl From<AgentRecord> for AgentInfo {
    fn from(a: AgentRecord) -> AgentInfo {
        AgentInfo {
            name: a.name,
            created_at: a.created_at,
            last_seen: a.last_seen,
            run_id: a.run_id,
        }
    }
}
