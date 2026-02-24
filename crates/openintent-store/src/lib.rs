//! # openintent-store
//!
//! Storage engine for OpenIntentOS.
//!
//! Provides SQLite-backed persistence with WAL mode and mmap for
//! microsecond reads, a 3-layer memory system (working / episodic /
//! semantic), a lock-free hot cache via `moka`, and multi-user
//! account management with PBKDF2 password hashing.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  CacheLayer (moka, < 0.05 us)           │
//! ├─────────────────────────────────────────┤
//! │  WorkingMemory   (HashMap, per-task)     │
//! │  EpisodicMemory  (SQLite episodes)       │
//! │  SemanticMemory  (SQLite memories + vec) │
//! ├─────────────────────────────────────────┤
//! │  UserStore     (multi-user, PBKDF2)      │
//! │  SessionStore  (conversation history)    │
//! │  WorkflowStore (persistent workflows)    │
//! ├─────────────────────────────────────────┤
//! │  Database (rusqlite WAL + mmap)          │
//! │  Migrations (versioned, transactional)   │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Quick start
//!
//! ```ignore
//! use openintent_store::{Database, EpisodicMemory, CacheLayer, UserStore};
//!
//! let db = Database::open_and_migrate("data/openintent.db").await?;
//! let episodes = EpisodicMemory::new(db.clone());
//! let users = UserStore::new(db.clone());
//! let cache: CacheLayer<String> = CacheLayer::builder("strings")
//!     .max_capacity(1000)
//!     .ttl_seconds(60)
//!     .build();
//! ```

pub mod bot_state;
pub mod cache;
pub mod db;
pub mod dev_task_store;
pub mod error;
pub mod memory;
pub mod migration;
pub mod session;
pub mod user_store;
pub mod workflow_store;

// ── re-exports ───────────────────────────────────────────────────────

pub use bot_state::BotStateStore;
pub use cache::{CacheLayer, CacheLayerBuilder, CacheStats};
pub use db::Database;
pub use dev_task_store::{DevTask, DevTaskMessage, DevTaskStore};
pub use error::{StoreError, StoreResult};
pub use memory::{
    Episode, EpisodeKind, EpisodicMemory, Memory, MemoryCategory, NewMemory, SemanticMemory,
    WorkingMemory,
};
pub use session::{Session, SessionMessage, SessionStore};
pub use user_store::{User, UserRole, UserStore};
pub use workflow_store::{StoredWorkflow, WorkflowStore};
