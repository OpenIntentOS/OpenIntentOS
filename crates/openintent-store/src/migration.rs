//! Schema migration system.
//!
//! Migrations are stored as static SQL strings keyed by version number.
//! The current version is tracked in a `_migrations` table so migrations
//! are idempotent and only run once.

use rusqlite::Connection;
use tracing::{debug, info, warn};

use crate::error::{StoreError, StoreResult};

/// A single migration definition.
struct Migration {
    /// Monotonically increasing version number (1, 2, 3, ...).
    version: u32,
    /// Human-readable description.
    description: &'static str,
    /// Raw SQL to execute. May contain multiple statements separated by `;`.
    sql: &'static str,
}

/// All migrations in order. Add new migrations to the end of this array.
static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "initial schema — workflows, tasks, episodes, memories, adapters",
        sql: r#"
            CREATE TABLE workflows (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                intent_raw  TEXT NOT NULL,
                steps       TEXT NOT NULL,
                trigger     TEXT,
                enabled     BOOLEAN DEFAULT 1,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            );

            CREATE TABLE tasks (
                id           TEXT PRIMARY KEY,
                workflow_id  TEXT REFERENCES workflows(id),
                status       TEXT NOT NULL CHECK(status IN ('pending','running','completed','failed','cancelled')),
                input        TEXT,
                output       TEXT,
                error        TEXT,
                started_at   INTEGER,
                completed_at INTEGER,
                created_at   INTEGER NOT NULL
            );
            CREATE INDEX idx_tasks_status ON tasks(status);
            CREATE INDEX idx_tasks_workflow ON tasks(workflow_id);

            CREATE TABLE episodes (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id   TEXT NOT NULL REFERENCES tasks(id),
                type      TEXT NOT NULL CHECK(type IN ('observation','action','result','reflection')),
                content   TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            );
            CREATE INDEX idx_episodes_task ON episodes(task_id);

            CREATE TABLE memories (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                category     TEXT NOT NULL CHECK(category IN ('preference','knowledge','pattern','skill')),
                content      TEXT NOT NULL,
                embedding    BLOB,
                importance   REAL DEFAULT 0.5,
                access_count INTEGER DEFAULT 0,
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL
            );

            CREATE TABLE adapters (
                id          TEXT PRIMARY KEY,
                type        TEXT NOT NULL,
                config      TEXT,
                status      TEXT DEFAULT 'disconnected',
                last_health INTEGER,
                created_at  INTEGER NOT NULL
            );
        "#,
    },
    Migration {
        version: 2,
        description: "session persistence — sessions and session_messages tables",
        sql: r#"
            CREATE TABLE sessions (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                model         TEXT NOT NULL DEFAULT '',
                message_count INTEGER DEFAULT 0,
                token_count   INTEGER DEFAULT 0,
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            );

            CREATE TABLE session_messages (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role          TEXT NOT NULL,
                content       TEXT NOT NULL,
                tool_calls    TEXT,
                tool_call_id  TEXT,
                created_at    INTEGER NOT NULL
            );
            CREATE INDEX idx_session_messages_session ON session_messages(session_id);
        "#,
    },
];

// ── public API ───────────────────────────────────────────────────────

/// Run all pending migrations against `conn`.
///
/// This is a **synchronous** function — call it from `spawn_blocking`.
pub fn run_all(conn: &Connection) -> StoreResult<()> {
    ensure_migrations_table(conn)?;

    let current = current_version(conn)?;
    let pending: Vec<&Migration> = MIGRATIONS.iter().filter(|m| m.version > current).collect();

    if pending.is_empty() {
        debug!(current_version = current, "database schema is up to date");
        return Ok(());
    }

    info!(
        current_version = current,
        pending = pending.len(),
        "running pending migrations"
    );

    for migration in pending {
        apply(conn, migration)?;
    }

    info!(
        new_version = MIGRATIONS.last().map(|m| m.version).unwrap_or(0),
        "all migrations applied"
    );
    Ok(())
}

/// Return the latest applied migration version, or 0 if none.
pub fn current_version(conn: &Connection) -> StoreResult<u32> {
    let version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| StoreError::Migration {
            version: 0,
            message: format!("failed to read current version: {e}"),
        })?;
    Ok(version)
}

// ── internals ────────────────────────────────────────────────────────

/// Create the `_migrations` bookkeeping table if it does not exist.
fn ensure_migrations_table(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version     INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at  INTEGER NOT NULL
        );",
    )
    .map_err(|e| StoreError::Migration {
        version: 0,
        message: format!("failed to create _migrations table: {e}"),
    })?;
    Ok(())
}

/// Apply a single migration inside a transaction.
fn apply(conn: &Connection, migration: &Migration) -> StoreResult<()> {
    info!(
        version = migration.version,
        description = migration.description,
        "applying migration"
    );

    // We cannot use `conn.transaction()` because that requires `&mut Connection`,
    // so we manage the transaction manually with SAVEPOINT.
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| StoreError::Migration {
            version: migration.version,
            message: format!("failed to begin transaction: {e}"),
        })?;

    let result = (|| -> StoreResult<()> {
        conn.execute_batch(migration.sql)
            .map_err(|e| StoreError::Migration {
                version: migration.version,
                message: format!("SQL execution failed: {e}"),
            })?;

        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO _migrations (version, description, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![migration.version, migration.description, now],
        )
        .map_err(|e| StoreError::Migration {
            version: migration.version,
            message: format!("failed to record migration: {e}"),
        })?;

        Ok(())
    })();

    match &result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")
                .map_err(|e| StoreError::Migration {
                    version: migration.version,
                    message: format!("failed to commit: {e}"),
                })?;
            info!(
                version = migration.version,
                "migration applied successfully"
            );
        }
        Err(err) => {
            warn!(version = migration.version, %err, "migration failed, rolling back");
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }

    result
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }

    #[test]
    fn migrations_are_ordered() {
        for window in MIGRATIONS.windows(2) {
            assert!(
                window[1].version > window[0].version,
                "migration versions must be strictly increasing: {} >= {}",
                window[0].version,
                window[1].version,
            );
        }
    }

    /// The expected latest migration version (update when adding migrations).
    const LATEST_VERSION: u32 = 2;

    #[test]
    fn run_all_on_fresh_db() {
        let conn = setup_conn();
        run_all(&conn).unwrap();

        let version = current_version(&conn).unwrap();
        assert_eq!(version, LATEST_VERSION);
    }

    #[test]
    fn run_all_is_idempotent() {
        let conn = setup_conn();
        run_all(&conn).unwrap();
        run_all(&conn).unwrap();

        let version = current_version(&conn).unwrap();
        assert_eq!(version, LATEST_VERSION);
    }

    #[test]
    fn migrations_create_all_tables() {
        let conn = setup_conn();
        run_all(&conn).unwrap();

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE '\\_%' ESCAPE '\\' ORDER BY name",
                )
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };

        // v1 tables
        assert!(tables.contains(&"workflows".to_string()));
        assert!(tables.contains(&"tasks".to_string()));
        assert!(tables.contains(&"episodes".to_string()));
        assert!(tables.contains(&"memories".to_string()));
        assert!(tables.contains(&"adapters".to_string()));
        // v2 tables
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"session_messages".to_string()));
    }
}
