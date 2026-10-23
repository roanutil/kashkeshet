use super::*;

/// How many numbered names to try before giving up on a base name.
const MAX_NAME_ATTEMPTS: u32 = 10_000;

impl Rules {
    /// Creates a new agent with a server-chosen name and a fresh token.
    /// Only the token's hash is stored.
    pub async fn register(&self) -> Result<Registration> {
        let entropy = self.inner.deps.entropy.clone();
        let base = names::agent_name(entropy.as_ref());
        let token = names::token(entropy.as_ref());
        let hash = names::hash_token(&token);
        let now = self.now_text();
        let name = self
            .db()
            .write(move |tx| {
                let name = free_name(tx, &base)?;
                tx.execute(
                    "INSERT INTO agents (name, token_hash, created_at, last_seen) VALUES (?, ?, ?, ?)",
                    rusqlite::params![name, hash, now, now],
                )?;
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
        let pattern = query.as_deref().map(like_pattern);
        self.db()
            .read(move |conn| {
                let mut stmt = conn.prepare_cached(
                    "SELECT name, created_at, last_seen FROM agents
                     WHERE (:pattern IS NULL OR name LIKE :pattern ESCAPE '\\')
                       AND (:cursor IS NULL OR name > :cursor)
                     ORDER BY name LIMIT :limit",
                )?;
                let mut items = stmt
                    .query_map(
                        named_params! {
                            ":pattern": pattern,
                            ":cursor": cursor,
                            ":limit": (limit + 1) as i64,
                        },
                        |r| {
                            Ok(AgentInfo {
                                name: r.get(0)?,
                                created_at: r.get(1)?,
                                last_seen: r.get(2)?,
                            })
                        },
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
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
fn free_name(conn: &Connection, base: &str) -> Result<String> {
    if !agent_exists(conn, base)? {
        return Ok(base.to_string());
    }
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let candidate = names::numbered_name(base, attempt);
        if !agent_exists(conn, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(RuleError::new(
        Code::Internal,
        format!("could not find a free name based on {base}"),
    ))
}
