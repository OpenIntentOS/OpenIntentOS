//! # openintent-store
//!
//! Storage engine for OpenIntentOS.
//!
//! Provides SQLite-backed persistence with WAL mode and mmap for
//! microsecond reads, a 3-layer memory system (working / episodic /
//! semantic), and a lock-free hot cache via `moka`.
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
//! │  Database (rusqlite WAL + mmap)          │
//! │  Migrations (versioned, transactional)   │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Quick start
//!
//! ```ignore
//! use openintent_store::{Database, EpisodicMemory, CacheLayer};
//!
//! let db = Database::open_and_migrate("data/openintent.db").await?;
//! let episodes = EpisodicMemory::new(db.clone());
//! let cache: CacheLayer<String> = CacheLayer::builder("strings")
//!     .max_capacity(1000)
//!     .ttl_seconds(60)
//!     .build();
//! ```

pub mod cache;
pub mod db;
pub mod error;
pub mod memory;
pub mod migration;

// ── re-exports ───────────────────────────────────────────────────────

pub use cache::{CacheLayer, CacheLayerBuilder, CacheStats};
pub use db::Database;
pub use error::{StoreError, StoreResult};
pub use memory::{
    Episode, EpisodeKind, EpisodicMemory, Memory, MemoryCategory, NewMemory, SemanticMemory,
    WorkingMemory,
};
