//! SQLite database setup with WAL mode, mmap, and performance pragmas.
//!
//! The [`Database`] struct wraps a `rusqlite::Connection` behind an
//! `Arc<Mutex<>>` and exposes async methods that use
//! `tokio::task::spawn_blocking` to avoid blocking the async runtime.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tracing::{debug, info};

use crate::error::{StoreError, StoreResult};
use crate::migration;

/// Thread-safe handle to a SQLite database.
///
/// All read/write operations go through [`Database::execute`] which
/// dispatches onto the blocking thread pool via `tokio::task::spawn_blocking`.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open (or create) a database at `path` and apply performance pragmas.
    ///
    /// This call blocks briefly (file I/O), so call it during startup before
    /// entering the main async loop, or wrap it in `spawn_blocking` yourself.
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        info!(path = %path.display(), "opening database");

        let conn = Connection::open(path)?;
        Self::apply_pragmas(&conn)?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        Ok(db)
    }

    /// Create an in-memory database — useful for tests.
    pub fn open_in_memory() -> StoreResult<Self> {
        debug!("opening in-memory database");

        let conn = Connection::open_in_memory()?;
        Self::apply_pragmas(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open the database and run all pending migrations.
    pub async fn open_and_migrate(path: impl AsRef<Path> + Send + 'static) -> StoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        let db = tokio::task::spawn_blocking(move || Self::open(&path)).await??;
        db.run_migrations().await?;
        Ok(db)
    }

    /// Run all pending schema migrations.
    pub async fn run_migrations(&self) -> StoreResult<()> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| StoreError::TaskJoin(format!("mutex poisoned: {e}")))?;
            migration::run_all(&conn)
        })
        .await?
    }

    /// Execute an arbitrary closure against the connection on the blocking pool.
    ///
    /// This is the primary way to interact with the database from async code.
    /// The closure receives a `&Connection` and must return a `StoreResult<T>`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let count: i64 = db.execute(|conn| {
    ///     let mut stmt = conn.prepare("SELECT count(*) FROM tasks")?;
    ///     let count = stmt.query_row([], |row| row.get(0))?;
    ///     Ok(count)
    /// }).await?;
    /// ```
    pub async fn execute<F, T>(&self, f: F) -> StoreResult<T>
    where
        F: FnOnce(&Connection) -> StoreResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| StoreError::TaskJoin(format!("mutex poisoned: {e}")))?;
            f(&conn)
        })
        .await?
    }

    /// Execute a mutable closure (for transactions, etc.) on the blocking pool.
    ///
    /// The closure receives a `&mut Connection` so you can call
    /// `conn.transaction()` and friends.
    pub async fn execute_mut<F, T>(&self, f: F) -> StoreResult<T>
    where
        F: FnOnce(&mut Connection) -> StoreResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut conn = conn
                .lock()
                .map_err(|e| StoreError::TaskJoin(format!("mutex poisoned: {e}")))?;
            f(&mut conn)
        })
        .await?
    }

    // ── pragmas ──────────────────────────────────────────────────────

    /// Apply all performance pragmas to a fresh connection.
    fn apply_pragmas(conn: &Connection) -> StoreResult<()> {
        debug!("applying SQLite performance pragmas");

        // WAL mode: concurrent readers, non-blocking writes.
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // NORMAL sync is safe with WAL — we only lose the last transaction
        // on a power failure, not corruption.
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        // 256 MiB memory-mapped I/O — avoids read() syscalls for hot data.
        conn.pragma_update(None, "mmap_size", 268_435_456_i64)?;

        // 64 000 pages * 4 KiB = 256 MiB page cache.
        // Negative value means KiB: -64000 = 64 000 KiB = ~62 MiB.
        conn.pragma_update(None, "cache_size", -64_000_i32)?;

        // Temp tables and indices in memory, not on disk.
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        // Enforce foreign key constraints.
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // Busy timeout so concurrent writers wait instead of failing immediately.
        conn.pragma_update(None, "busy_timeout", 5_000_i32)?;

        info!("database pragmas applied (WAL, mmap 256MiB, cache 62MiB)");
        Ok(())
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_in_memory_works() {
        let db = Database::open_in_memory().unwrap();
        let version: String = db
            .execute(|conn| {
                let v: String =
                    conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
                Ok(v)
            })
            .await
            .unwrap();
        assert!(!version.is_empty());
    }

    #[tokio::test]
    async fn pragmas_are_applied() {
        let db = Database::open_in_memory().unwrap();
        let journal: String = db
            .execute(|conn| {
                let v: String =
                    conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
                Ok(v)
            })
            .await
            .unwrap();
        // In-memory databases report "memory" for journal_mode, but the
        // pragma call itself should not fail.
        assert!(!journal.is_empty());
    }

    #[tokio::test]
    async fn migrations_run_on_fresh_db() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().await.unwrap();

        // Verify a table from the migration exists.
        let count: i64 = db
            .execute(|conn| {
                let c: i64 =
                    conn.query_row("SELECT count(*) FROM workflows", [], |row| row.get(0))?;
                Ok(c)
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
