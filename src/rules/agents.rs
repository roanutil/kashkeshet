use super::*;
use crate::config::MAX_NAME_ATTEMPTS;

impl Rules {
    /// Creates a new agent with a server-chosen name and a fresh token.
    /// Only the token's hash is stored.
    pub async fn register(&self) -> Result<Registration> {
        let entropy = self.inner.deps.entropy.clone();
        let base = names::agent_name(entropy.as_ref());
        let token = auth::token(entropy.as_ref());
        let hash = auth::hash_token(&token);
        let now = self.now_text();
        let name = self
            .db()
            .write(move |store| {
                let name = free_name(store, &base)?;
                store.insert_agent(&NewAgentRecord {
                    name: name.clone(),
                    token_hash: hash,
                    created_at: now.clone(),
                    last_seen: Some(now),
                })?;
                Ok::<_, RuleError>(name)
            })
            .await?;
        Ok(Registration { name, token })
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
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub created_at: String,
    pub last_seen: Option<String>,
}

impl From<AgentRecord> for AgentInfo {
    fn from(a: AgentRecord) -> AgentInfo {
        AgentInfo {
            name: a.name,
            created_at: a.created_at,
            last_seen: a.last_seen,
        }
    }
}
