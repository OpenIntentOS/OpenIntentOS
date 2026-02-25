//! SQLite adapter for OpenIntentOS
//! 
//! Provides structured data storage capabilities using SQLite databases.

pub mod adapter;
pub mod connection;
pub mod error;
pub mod migration;

pub use adapter::SqliteAdapter;
pub use connection::SqliteConnectionPool;
pub use error::{SqliteError, SqliteResult};
pub use migration::MigrationManager;