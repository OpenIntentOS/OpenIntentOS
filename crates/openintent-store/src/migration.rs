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
    Migration {
        version: 3,
        description: "multi-user support — users table and session user linkage",
        sql: r#"
            CREATE TABLE users (
                id            TEXT PRIMARY KEY,
                username      TEXT NOT NULL UNIQUE,
                display_name  TEXT,
                password_hash TEXT NOT NULL,
                role          TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('admin', 'user', 'viewer')),
                active        BOOLEAN DEFAULT 1,
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            );
            CREATE INDEX idx_users_username ON users(username);

            ALTER TABLE sessions ADD COLUMN user_id TEXT REFERENCES users(id);
            CREATE INDEX idx_sessions_user ON sessions(user_id);
        "#,
    },
    Migration {
        version: 4,
        description: "dev tasks — self-development task tracking and message history",
        sql: r#"
            CREATE TABLE dev_tasks (
                id            TEXT PRIMARY KEY,
                source        TEXT NOT NULL CHECK(source IN ('telegram','cli','evolution','api')),
                chat_id       INTEGER,
                intent        TEXT NOT NULL,
                status        TEXT NOT NULL CHECK(status IN ('pending','branching','coding','testing','pr_created','awaiting_review','merging','completed','failed','cancelled')),
                branch        TEXT,
                pr_url        TEXT,
                current_step  TEXT,
                progress_log  TEXT NOT NULL DEFAULT '[]',
                error         TEXT,
                retry_count   INTEGER DEFAULT 0,
                max_retries   INTEGER DEFAULT 3,
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            );
            CREATE INDEX idx_dev_tasks_status ON dev_tasks(status);
            CREATE INDEX idx_dev_tasks_chat ON dev_tasks(chat_id);

            CREATE TABLE dev_task_messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id     TEXT NOT NULL REFERENCES dev_tasks(id) ON DELETE CASCADE,
                role        TEXT NOT NULL CHECK(role IN ('user','system','agent','progress')),
                content     TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX idx_dev_task_messages_task ON dev_task_messages(task_id);
        "#,
    },
    Migration {
        version: 5,
        description: "bot_state — key-value store for persistent bot state (offsets, etc.)",
        sql: r#"
            CREATE TABLE bot_state (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
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
    const LATEST_VERSION: u32 = 5;

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
        // v3 tables
        assert!(tables.contains(&"users".to_string()));
        // v4 tables
        assert!(tables.contains(&"dev_tasks".to_string()));
        assert!(tables.contains(&"dev_task_messages".to_string()));
    }

    #[test]
    fn v3_users_table_has_correct_columns() {
        let conn = setup_conn();
        run_all(&conn).unwrap();

        // Verify the users table can be queried with expected columns.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        // Verify the user_id column was added to sessions.
        conn.execute_batch(
            "INSERT INTO sessions (id, name, model, message_count, token_count, created_at, updated_at, user_id) \
             VALUES ('test', 'test', 'model', 0, 0, 0, 0, NULL)",
        )
        .unwrap();
    }

    #[test]
    fn v4_dev_tasks_table_exists() {
        let conn = setup_conn();
        run_all(&conn).unwrap();

        // Verify the dev_tasks table can be queried with expected columns.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dev_tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        // Verify the dev_task_messages table exists.
        let msg_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dev_task_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(msg_count, 0);

        // Verify we can insert a dev_task with all columns.
        conn.execute(
            "INSERT INTO dev_tasks (id, source, chat_id, intent, status, branch, pr_url, current_step, progress_log, error, retry_count, max_retries, created_at, updated_at) \
             VALUES ('test-id', 'telegram', 12345, 'fix a bug', 'pending', NULL, NULL, NULL, '[]', NULL, 0, 3, 0, 0)",
            [],
        )
        .unwrap();

        // Verify the CHECK constraint on source works.
        let bad_source = conn.execute(
            "INSERT INTO dev_tasks (id, source, intent, status, created_at, updated_at) \
             VALUES ('bad', 'invalid_source', 'intent', 'pending', 0, 0)",
            [],
        );
        assert!(bad_source.is_err());

        // Verify the CHECK constraint on status works.
        let bad_status = conn.execute(
            "INSERT INTO dev_tasks (id, source, intent, status, created_at, updated_at) \
             VALUES ('bad2', 'cli', 'intent', 'invalid_status', 0, 0)",
            [],
        );
        assert!(bad_status.is_err());

        // Verify dev_task_messages with foreign key to dev_tasks.
        conn.execute(
            "INSERT INTO dev_task_messages (task_id, role, content, created_at) \
             VALUES ('test-id', 'user', 'hello', 0)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn v5_bot_state_table_exists() {
        let conn = setup_conn();
        run_all(&conn).unwrap();

        // Verify bot_state table exists and supports UPSERT.
        conn.execute(
            "INSERT INTO bot_state (key, value) VALUES ('test_key', 'test_value') \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .unwrap();

        let value: String = conn
            .query_row(
                "SELECT value FROM bot_state WHERE key = 'test_key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "test_value");
    }
}
