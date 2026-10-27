//! The store over SQLite. This is the only file that knows about SQL.
//!
//! One connection writes; several others read. In WAL mode the readers do not
//! block the writer or each other. All calls run on Tokio's blocking pool.

use super::*;
use crate::config::{DB_BUSY_TIMEOUT, DB_READERS};
use rusqlite::{Connection, OpenFlags, OptionalExtension, named_params};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_server_runs.sql"),
    include_str!("../../migrations/0003_revoked_agents.sql"),
    include_str!("../../migrations/0004_identity_recovery.sql"),
    include_str!("../../migrations/0005_messages_by_time.sql"),
    include_str!("../../migrations/0006_agent_descriptions.sql"),
    include_str!("../../migrations/0007_channel_notes.sql"),
    include_str!("../../migrations/0008_operator_messages.sql"),
    include_str!("../../migrations/0010_hidden_entries.sql"),
];

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> StoreError {
        StoreError::Database(e.to_string())
    }
}

#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

struct Inner {
    writer: Mutex<Connection>,
    /// Empty for an in-memory database, which can only have one connection.
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
}

impl Db {
    /// Opens (creating if needed) the database file and brings its tables up to date.
    pub fn open(path: &Path) -> StoreResult<Db> {
        let mut writer = Connection::open(path)?;
        configure(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut writer)?;
        let readers = (0..DB_READERS)
            .map(|_| {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                configure(&conn)?;
                Ok(Mutex::new(conn))
            })
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(Db::from_parts(writer, readers))
    }

    /// A fresh database held in memory, for tests.
    pub fn open_in_memory() -> StoreResult<Db> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&mut conn)?;
        Ok(Db::from_parts(conn, Vec::new()))
    }

    fn from_parts(writer: Connection, readers: Vec<Mutex<Connection>>) -> Db {
        Db {
            inner: Arc::new(Inner {
                writer: Mutex::new(writer),
                readers,
                next_reader: AtomicUsize::new(0),
            }),
        }
    }

    /// Runs `f` on a read connection.
    pub async fn read<T, E, F>(&self, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: FnOnce(&dyn Reads) -> Result<T, E> + Send + 'static,
    {
        let inner = self.inner.clone();
        run_blocking(move || {
            let lock = if inner.readers.is_empty() {
                &inner.writer
            } else {
                let i = inner.next_reader.fetch_add(1, Ordering::Relaxed) % inner.readers.len();
                &inner.readers[i]
            };
            let conn = lock.lock().map_err(|_| StoreError::Poisoned)?;
            f(&Sql(&conn))
        })
        .await
    }

    /// Runs `f` inside a write transaction. The transaction commits only if `f` succeeds.
    pub async fn write<T, E, F>(&self, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: FnOnce(&dyn Writes) -> Result<T, E> + Send + 'static,
    {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut conn = inner.writer.lock().map_err(|_| StoreError::Poisoned)?;
            let tx = conn.transaction().map_err(StoreError::from)?;
            let value = f(&Sql(&tx))?;
            tx.commit().map_err(StoreError::from)?;
            Ok(value)
        })
        .await
    }

    /// Runs raw SQL in its own transaction. Tests use it to break the database
    /// on purpose, for example with a trigger that makes inserts fail.
    #[cfg(test)]
    pub async fn execute_for_tests(&self, sql: &'static str) -> StoreResult<()> {
        let inner = self.inner.clone();
        run_blocking(move || {
            let conn = inner.writer.lock().map_err(|_| StoreError::Poisoned)?;
            conn.execute_batch(sql)?;
            Ok(())
        })
        .await
    }
}

async fn run_blocking<T, E, F>(f: F) -> Result<T, E>
where
    T: Send + 'static,
    E: From<StoreError> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(join_error) => Err(StoreError::Task(join_error.to_string()).into()),
    }
}

fn configure(conn: &Connection) -> StoreResult<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(DB_BUSY_TIMEOUT)?;
    Ok(())
}

/// Applies any migrations newer than the database's `user_version`.
fn migrate(conn: &mut Connection) -> StoreResult<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current.max(0) as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

/// The store's questions and changes, over one connection. Inside
/// [`Db::write`] the connection is a transaction.
struct Sql<'a>(&'a Connection);

const AGENT_COLUMNS: &str = "name, created_at, last_seen, run_id, revoked_at, description";

fn agent_from_row(r: &rusqlite::Row) -> rusqlite::Result<AgentRecord> {
    Ok(AgentRecord {
        name: r.get(0)?,
        created_at: r.get(1)?,
        last_seen: r.get(2)?,
        run_id: r.get(3)?,
        revoked_at: r.get(4)?,
        description: r.get(5)?,
    })
}

const MESSAGE_COLUMNS: &str = "m.id, m.channel_id, c.name, m.agent, m.body, m.created_at, m.run_id";

fn message_from_row(r: &rusqlite::Row) -> rusqlite::Result<MessageRecord> {
    Ok(MessageRecord {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        channel_name: r.get(2)?,
        agent: r.get(3)?,
        body: r.get(4)?,
        created_at: r.get(5)?,
        run_id: r.get(6)?,
    })
}

/// Which messages a `NewerFilter` covers. Uses `:after`, `:me` and `:filter`.
const NEWER_SCOPE: &str = "m.id > :after
    AND m.channel_id IN (SELECT id FROM channels WHERE hidden = 0)
    AND m.channel_id IN (SELECT channel_id FROM memberships WHERE agent = :me)
    AND (:filter IS NULL OR m.channel_id IN (SELECT value FROM json_each(:filter)))";

/// The channel ID list of a `NewerFilter`, as the JSON array `json_each` reads.
fn channel_ids_json(filter: &NewerFilter) -> StoreResult<Option<String>> {
    filter
        .channel_ids
        .as_ref()
        .map(|ids| serde_json::to_string(ids).map_err(|e| StoreError::Database(e.to_string())))
        .transpose()
}

impl Reads for Sql<'_> {
    fn agent_name_taken(&self, name: &str) -> StoreResult<bool> {
        Ok(self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE name = ?)",
            [name],
            |r| r.get(0),
        )?)
    }

    fn agent(&self, name: &str) -> StoreResult<Option<AgentRecord>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {AGENT_COLUMNS} FROM agents WHERE name = ? AND hidden = 0"),
                [name],
                agent_from_row,
            )
            .optional()?)
    }

    fn agent_by_token_hash(&self, hash: &[u8]) -> StoreResult<Option<AgentRecord>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {AGENT_COLUMNS} FROM agents WHERE token_hash = ? AND hidden = 0"),
                [hash],
                agent_from_row,
            )
            .optional()?)
    }

    fn agent_by_recovery_hash(&self, name: &str, hash: &[u8]) -> StoreResult<Option<AgentRecord>> {
        Ok(self
            .0
            .query_row(
                &format!("SELECT {AGENT_COLUMNS} FROM agents WHERE name = ? AND recovery_hash = ? AND hidden = 0"),
                rusqlite::params![name, hash],
                agent_from_row,
            )
            .optional()?)
    }

    fn agents(
        &self,
        text: Option<&str>,
        after_name: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AgentRecord>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents
             WHERE hidden = 0 AND (:pattern IS NULL OR name LIKE :pattern ESCAPE '\\')
               AND (:cursor IS NULL OR name > :cursor)
             ORDER BY name LIMIT :limit"
        ))?;
        let agents = stmt
            .query_map(
                named_params! {
                    ":pattern": text.map(like_pattern),
                    ":cursor": after_name,
                    ":limit": limit as i64,
                },
                agent_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(agents)
    }

    fn active_agent_count(&self) -> StoreResult<i64> {
        Ok(self.0.query_row(
            "SELECT count(*) FROM agents WHERE revoked_at IS NULL AND hidden = 0",
            [],
            |r| r.get(0),
        )?)
    }

    fn agents_by_last_seen(&self) -> StoreResult<Vec<AgentRecord>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents WHERE hidden = 0 ORDER BY coalesce(last_seen, created_at) DESC"
        ))?;
        let agents = stmt
            .query_map([], agent_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(agents)
    }

    fn channel_id_taken(&self, id: &str) -> StoreResult<bool> {
        Ok(self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM channels WHERE id = ?)",
            [id],
            |r| r.get(0),
        )?)
    }

    fn channel(&self, id: &str) -> StoreResult<Option<ChannelFacts>> {
        Ok(self
            .0
            .query_row(
                "SELECT id, name, is_open FROM channels WHERE id = ? AND hidden = 0",
                [id],
                |r| {
                    Ok(ChannelFacts {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        open: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    fn is_member(&self, channel_id: &str, agent: &str) -> StoreResult<bool> {
        let found: Option<i64> = self
            .0
            .query_row(
                "SELECT 1 FROM memberships WHERE channel_id = ? AND agent = ?",
                [channel_id, agent],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    fn channel_ids(&self, filter: &ChannelFilter) -> StoreResult<Vec<String>> {
        let (cursor_name, cursor_id) = match &filter.after {
            Some((name, id)) => (Some(name.as_str()), Some(id.as_str())),
            None => (None, None),
        };
        let mut stmt = self.0.prepare_cached(
            "SELECT c.id FROM channels c
             WHERE c.hidden = 0 AND (c.is_open = 1 OR EXISTS (SELECT 1 FROM memberships m
                                             WHERE m.channel_id = c.id AND m.agent = :me))
               AND (:member_only = 0 OR EXISTS (SELECT 1 FROM memberships m
                                                WHERE m.channel_id = c.id AND m.agent = :me))
               AND (:with IS NULL OR (
                     EXISTS (SELECT 1 FROM memberships m WHERE m.channel_id = c.id AND m.agent = :me)
                 AND EXISTS (SELECT 1 FROM memberships w WHERE w.channel_id = c.id AND w.agent = :with)))
               AND (:pattern IS NULL
                    OR c.name LIKE :pattern ESCAPE '\\'
                    OR coalesce(c.purpose, '') LIKE :pattern ESCAPE '\\')
               AND (:cname IS NULL OR (c.name, c.id) > (:cname, :cid))
             ORDER BY c.name, c.id
             LIMIT :limit",
        )?;
        let ids = stmt
            .query_map(
                named_params! {
                    ":me": filter.visible_to,
                    ":member_only": filter.member_only,
                    ":with": filter.shared_with,
                    ":pattern": filter.text.as_deref().map(like_pattern),
                    ":cname": cursor_name,
                    ":cid": cursor_id,
                    ":limit": filter.limit as i64,
                },
                |r| r.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    fn all_channel_ids(&self, member: Option<&str>) -> StoreResult<Vec<String>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT c.id FROM channels c
             WHERE c.hidden = 0 AND (:member IS NULL
                OR EXISTS (SELECT 1 FROM memberships m WHERE m.channel_id = c.id AND m.agent = :member))
             ORDER BY c.name, c.id",
        )?;
        let ids = stmt
            .query_map(named_params! { ":member": member }, |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }

    fn channel_summary(
        &self,
        id: &str,
        viewer: Option<&str>,
        after_id: Option<i64>,
    ) -> StoreResult<Option<ChannelSummary>> {
        Ok(self
            .0
            .query_row(
                "SELECT c.id, c.name, c.purpose, c.is_open, c.created_by, c.created_at,
                        (SELECT count(*) FROM memberships m WHERE m.channel_id = c.id),
                        EXISTS (SELECT 1 FROM memberships m WHERE m.channel_id = c.id AND m.agent = :viewer),
                        CASE WHEN :after IS NULL THEN NULL
                             ELSE (SELECT count(*) FROM messages g WHERE g.channel_id = c.id AND g.id > :after)
                        END, c.note, c.note_edited_by, c.note_edited_at
                 FROM channels c WHERE c.id = :id AND c.hidden = 0",
                named_params! { ":id": id, ":viewer": viewer, ":after": after_id },
                |r| {
                    Ok(ChannelSummary {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        purpose: r.get(2)?,
                        open: r.get(3)?,
                        created_by: r.get(4)?,
                        created_at: r.get(5)?,
                        member_count: r.get(6)?,
                        viewer_is_member: r.get(7)?,
                        new_messages: r.get(8)?,
                        note: ChannelNoteRecord { text: r.get(9)?, edited_by: r.get(10)?, edited_at: r.get(11)? },
                    })
                },
            )
            .optional()?)
    }

    fn members_by_join(&self, channel_id: &str, limit: usize) -> StoreResult<Vec<String>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT agent FROM memberships WHERE channel_id = ? ORDER BY joined_at, agent LIMIT ?",
        )?;
        let names = stmt
            .query_map(rusqlite::params![channel_id, limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(names)
    }

    fn member_count(&self, channel_id: &str) -> StoreResult<i64> {
        Ok(self.0.query_row(
            "SELECT count(*) FROM memberships WHERE channel_id = ?",
            [channel_id],
            |r| r.get(0),
        )?)
    }

    fn members_by_name(
        &self,
        channel_id: &str,
        after_name: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AgentRecord>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents
             WHERE name IN (SELECT agent FROM memberships WHERE channel_id = :id)
               AND (:cursor IS NULL OR name > :cursor)
             ORDER BY name LIMIT :limit"
        ))?;
        let members = stmt
            .query_map(
                named_params! { ":id": channel_id, ":cursor": after_name, ":limit": limit as i64 },
                agent_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(members)
    }

    fn message(&self, id: i64) -> StoreResult<Option<MessageRecord>> {
        Ok(self
            .0
            .query_row(
                &format!(
                    "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
                     WHERE m.id = ?"
                ),
                [id],
                message_from_row,
            )
            .optional()?)
    }

    fn channel_messages(
        &self,
        channel_id: &str,
        from_agent: Option<&str>,
        range: MessageRange,
        limit: usize,
    ) -> StoreResult<Vec<MessageRecord>> {
        let (sql, bound) = match range {
            MessageRange::After(id) => ("m.id > :bound ORDER BY m.id ASC", id),
            MessageRange::Before(id) => ("m.id < :bound ORDER BY m.id DESC", id),
            MessageRange::Latest => ("m.id <= :bound ORDER BY m.id DESC", i64::MAX),
        };
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE m.channel_id = :ch AND (:from IS NULL OR m.agent = :from) AND {sql}
             LIMIT :limit"
        ))?;
        let messages = stmt
            .query_map(
                named_params! {
                    ":ch": channel_id, ":from": from_agent, ":bound": bound, ":limit": limit as i64,
                },
                message_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(messages)
    }

    fn newer_messages(
        &self,
        filter: &NewerFilter,
        limit: usize,
    ) -> StoreResult<Vec<MessageRecord>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE {NEWER_SCOPE} ORDER BY m.id LIMIT :limit"
        ))?;
        let messages = stmt
            .query_map(
                named_params! {
                    ":after": filter.after_id,
                    ":me": filter.member,
                    ":filter": channel_ids_json(filter)?,
                    ":limit": limit as i64,
                },
                message_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(messages)
    }

    fn count_newer_messages(&self, filter: &NewerFilter) -> StoreResult<i64> {
        Ok(self.0.query_row(
            &format!("SELECT count(*) FROM messages m WHERE {NEWER_SCOPE}"),
            named_params! {
                ":after": filter.after_id,
                ":me": filter.member,
                ":filter": channel_ids_json(filter)?,
            },
            |r| r.get(0),
        )?)
    }

    fn recent_messages(&self, limit: usize) -> StoreResult<Vec<MessageRecord>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE c.hidden = 0 ORDER BY m.id DESC LIMIT ?"
        ))?;
        let messages = stmt
            .query_map([limit as i64], message_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(messages)
    }

    fn message_count_from(&self, agent: &str) -> StoreResult<i64> {
        Ok(self.0.query_row(
            "SELECT count(*) FROM messages WHERE agent = ?",
            [agent],
            |r| r.get(0),
        )?)
    }

    fn search(&self, filter: &SearchFilter) -> StoreResult<Vec<SearchHitRecord>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT m.id, m.channel_id, c.name, m.agent, m.created_at,
                    snippet(messages_fts, 0, '**', '**', '…', 12)
             FROM messages_fts
             JOIN messages m ON m.id = messages_fts.rowid
             JOIN channels c ON c.id = m.channel_id
             WHERE c.hidden = 0 AND messages_fts MATCH :q
               AND EXISTS (SELECT 1 FROM memberships x
                           WHERE x.channel_id = m.channel_id AND x.agent = :me)
               AND (:ch IS NULL OR m.channel_id = :ch)
               AND (:from IS NULL OR m.agent = :from)
             ORDER BY m.id DESC
             LIMIT :limit",
        )?;
        let hits = stmt
            .query_map(
                named_params! {
                    ":q": fts_query(&filter.words),
                    ":me": filter.member,
                    ":ch": filter.channel_id,
                    ":from": filter.from_agent,
                    ":limit": filter.limit as i64,
                },
                |r| {
                    Ok(SearchHitRecord {
                        message_id: r.get(0)?,
                        channel_id: r.get(1)?,
                        channel_name: r.get(2)?,
                        agent: r.get(3)?,
                        created_at: r.get(4)?,
                        excerpt: r.get(5)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hits)
    }

    fn activity(&self, member: &str, after_id: i64) -> StoreResult<Vec<ActivityRecord>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT m.channel_id, c.name, count(*), max(m.id)
             FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE c.hidden = 0 AND m.id > :after
               AND m.channel_id IN (SELECT channel_id FROM memberships WHERE agent = :me)
             GROUP BY m.channel_id
             ORDER BY max(m.id) DESC",
        )?;
        let rows = stmt
            .query_map(named_params! { ":after": after_id, ":me": member }, |r| {
                Ok(ActivityRecord {
                    channel_id: r.get(0)?,
                    channel_name: r.get(1)?,
                    new_messages: r.get(2)?,
                    latest_id: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn channel_use(&self) -> StoreResult<Vec<ChannelUseRecord>> {
        // The newest message is found through the (channel_id, id) index.
        let mut stmt = self.0.prepare_cached(
            "SELECT c.id,
                    (SELECT count(*) FROM messages m WHERE m.channel_id = c.id),
                    last.created_at, last.agent
             FROM channels c
             LEFT JOIN messages last ON last.id =
                 (SELECT max(m.id) FROM messages m WHERE m.channel_id = c.id)",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ChannelUseRecord {
                    channel_id: r.get(0)?,
                    message_count: r.get(1)?,
                    last_message_at: r.get(2)?,
                    last_speaker: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn memberships(&self) -> StoreResult<Vec<MembershipRecord>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT m.channel_id, c.name, m.agent
             FROM memberships m JOIN channels c ON c.id = m.channel_id
             ORDER BY m.joined_at, m.rowid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(MembershipRecord {
                    channel_id: r.get(0)?,
                    channel_name: r.get(1)?,
                    agent: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn message_times_since(&self, since: &str) -> StoreResult<Vec<MessageTimeRecord>> {
        // A range on the messages_by_time index: it reads only the answer,
        // and does not assume times follow message IDs.
        let mut stmt = self.0.prepare_cached(
            "SELECT channel_id, created_at FROM messages WHERE created_at >= ? ORDER BY created_at",
        )?;
        let times = stmt
            .query_map([since], |r| {
                Ok(MessageTimeRecord {
                    channel_id: r.get(0)?,
                    created_at: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(times)
    }

    fn server_runs(&self) -> StoreResult<Vec<ServerRunRecord>> {
        let mut stmt = self.0.prepare_cached(
            "SELECT id, started_at, version, git_commit, settings, instructions_sha256
             FROM server_runs ORDER BY id DESC",
        )?;
        let runs = stmt
            .query_map([], |r| {
                Ok(ServerRunRecord {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    version: r.get(2)?,
                    git_commit: r.get(3)?,
                    settings: r.get(4)?,
                    instructions_sha256: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(runs)
    }
}

impl Writes for Sql<'_> {
    fn insert_agent(&self, agent: &NewAgentRecord) -> StoreResult<()> {
        self.0.execute(
            "INSERT INTO agents (name, token_hash, recovery_hash, created_at, last_seen, run_id)
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                agent.name,
                agent.token_hash,
                agent.recovery_hash,
                agent.created_at,
                agent.last_seen,
                agent.run_id
            ],
        )?;
        Ok(())
    }

    fn set_token_hash(&self, name: &str, hash: &[u8]) -> StoreResult<()> {
        self.0.execute(
            "UPDATE agents SET token_hash = ? WHERE name = ?",
            rusqlite::params![hash, name],
        )?;
        Ok(())
    }

    fn set_last_seen(&self, name: &str, at: &str) -> StoreResult<()> {
        self.0
            .execute("UPDATE agents SET last_seen = ? WHERE name = ?", [at, name])?;
        Ok(())
    }

    fn set_description(&self, name: &str, description: Option<&str>) -> StoreResult<()> {
        self.0.execute(
            "UPDATE agents SET description = ? WHERE name = ?",
            rusqlite::params![description, name],
        )?;
        Ok(())
    }

    fn set_revoked(&self, name: &str, at: &str) -> StoreResult<()> {
        self.0.execute(
            "UPDATE agents SET revoked_at = ? WHERE name = ?",
            [at, name],
        )?;
        Ok(())
    }

    fn insert_channel(&self, channel: &NewChannelRecord) -> StoreResult<()> {
        self.0.execute(
            "INSERT INTO channels (id, name, purpose, is_open, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                channel.id,
                channel.name,
                channel.purpose,
                channel.open,
                channel.created_by,
                channel.created_at
            ],
        )?;
        Ok(())
    }

    fn set_purpose(&self, channel_id: &str, purpose: Option<&str>) -> StoreResult<()> {
        self.0.execute(
            "UPDATE channels SET purpose = ? WHERE id = ?",
            rusqlite::params![purpose, channel_id],
        )?;
        Ok(())
    }

    fn set_channel_open(&self, channel_id: &str, open: bool) -> StoreResult<()> {
        self.0.execute(
            "UPDATE channels SET is_open = ? WHERE id = ?",
            rusqlite::params![open, channel_id],
        )?;
        Ok(())
    }

    fn hide_channel(&self, channel_id: &str) -> StoreResult<()> {
        self.0
            .execute("UPDATE channels SET hidden = 1 WHERE id = ?", [channel_id])?;
        self.0
            .execute("DELETE FROM memberships WHERE channel_id = ?", [channel_id])?;
        Ok(())
    }

    fn hide_agent(&self, name: &str, at: &str) -> StoreResult<()> {
        self.0.execute(
            "UPDATE agents SET hidden = 1, revoked_at = coalesce(revoked_at, ?) WHERE name = ?",
            [at, name],
        )?;
        self.0
            .execute("DELETE FROM memberships WHERE agent = ?", [name])?;
        Ok(())
    }

    fn set_note(&self, channel_id: &str, text: &str, editor: &str, at: &str) -> StoreResult<()> {
        self.0.execute(
            "UPDATE channels SET note = ?, note_edited_by = ?, note_edited_at = ? WHERE id = ?",
            rusqlite::params![text, editor, at, channel_id],
        )?;
        Ok(())
    }

    fn add_member(&self, channel_id: &str, agent: &str, joined_at: &str) -> StoreResult<()> {
        self.0.execute(
            "INSERT OR IGNORE INTO memberships (channel_id, agent, joined_at) VALUES (?, ?, ?)",
            [channel_id, agent, joined_at],
        )?;
        Ok(())
    }

    fn remove_member(&self, channel_id: &str, agent: &str) -> StoreResult<()> {
        self.0.execute(
            "DELETE FROM memberships WHERE channel_id = ? AND agent = ?",
            [channel_id, agent],
        )?;
        Ok(())
    }

    fn insert_message(&self, message: &NewMessageRecord) -> StoreResult<i64> {
        self.0.execute(
            "INSERT INTO messages (channel_id, agent, body, created_at, run_id)
             VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![
                message.channel_id,
                message.agent,
                message.body,
                message.created_at,
                message.run_id
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    fn insert_server_run(&self, run: &NewServerRunRecord) -> StoreResult<i64> {
        self.0.execute(
            "INSERT INTO server_runs (started_at, version, git_commit, settings, instructions_sha256)
             VALUES (?, ?, ?, ?, ?)",
            [
                &run.started_at,
                &run.version,
                &run.git_commit,
                &run.settings,
                &run.instructions_sha256,
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }
}

/// Escapes `%`, `_` and `\` so text matches literally in a `LIKE ... ESCAPE '\'` pattern.
pub(super) fn like_pattern(text: &str) -> String {
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

/// An FTS5 query that matches messages containing every word. Each word is
/// quoted, so FTS5 syntax in the words has no effect.
pub(super) fn fts_query(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}
