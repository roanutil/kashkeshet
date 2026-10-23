//! Database setup and access.
//!
//! One connection writes; several others read. In WAL mode the readers do not
//! block the writer or each other. All calls run on Tokio's blocking pool.

use rusqlite::{Connection, OpenFlags, Transaction};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];
const READERS: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database task failed: {0}")]
    Task(String),
    #[error("database lock poisoned")]
    Poisoned,
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
    pub fn open(path: &Path) -> Result<Db, DbError> {
        let mut writer = Connection::open(path)?;
        configure(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut writer)?;
        let readers = (0..READERS)
            .map(|_| {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                configure(&conn)?;
                Ok(Mutex::new(conn))
            })
            .collect::<Result<Vec<_>, DbError>>()?;
        Ok(Db::from_parts(writer, readers))
    }

    /// A fresh database held in memory, for tests.
    pub fn open_in_memory() -> Result<Db, DbError> {
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
        E: From<DbError> + Send + 'static,
        F: FnOnce(&Connection) -> Result<T, E> + Send + 'static,
    {
        let inner = self.inner.clone();
        run_blocking(move || {
            let lock = if inner.readers.is_empty() {
                &inner.writer
            } else {
                let i = inner.next_reader.fetch_add(1, Ordering::Relaxed) % inner.readers.len();
                &inner.readers[i]
            };
            let conn = lock.lock().map_err(|_| DbError::Poisoned)?;
            f(&conn)
        })
        .await
    }

    /// Runs `f` inside a write transaction. The transaction commits only if `f` succeeds.
    pub async fn write<T, E, F>(&self, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DbError> + Send + 'static,
        F: FnOnce(&Transaction) -> Result<T, E> + Send + 'static,
    {
        let inner = self.inner.clone();
        run_blocking(move || {
            let mut conn = inner.writer.lock().map_err(|_| DbError::Poisoned)?;
            let tx = conn.transaction().map_err(DbError::from)?;
            let value = f(&tx)?;
            tx.commit().map_err(DbError::from)?;
            Ok(value)
        })
        .await
    }
}

async fn run_blocking<T, E, F>(f: F) -> Result<T, E>
where
    T: Send + 'static,
    E: From<DbError> + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(join_error) => Err(DbError::Task(join_error.to_string()).into()),
    }
}

fn configure(conn: &Connection) -> Result<(), DbError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(())
}

/// Applies any migrations newer than the database's `user_version`.
fn migrate(conn: &mut Connection) -> Result<(), DbError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current.max(0) as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_database_reopens_without_rerunning_migrations() {
        let dir = std::env::temp_dir().join(format!("kashkeshet-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.db");
        let _ = std::fs::remove_file(&path);

        let db = Db::open(&path).expect("open");
        db.write(|tx| {
            tx.execute(
                "INSERT INTO agents (name, token_hash, created_at) VALUES ('a', x'00', 't')",
                [],
            )
            .map_err(DbError::from)
        })
        .await
        .expect("insert");
        drop(db);

        let db = Db::open(&path).expect("reopen");
        let count: i64 = db
            .read(|c| {
                c.query_row("SELECT count(*) FROM agents", [], |r| r.get(0))
                    .map_err(DbError::from)
            })
            .await
            .expect("count");
        assert_eq!(count, 1);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
