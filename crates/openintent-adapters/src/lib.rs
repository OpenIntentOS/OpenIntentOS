//! Service adapters for OpenIntentOS — filesystem, shell, memory, cron, email, browser, and more.
//!
//! Each adapter implements the [`Adapter`] trait defined in [`traits`],
//! providing a uniform interface for tool discovery and execution.

pub mod browser;
pub mod calendar;
pub mod cron;
pub mod email;
pub mod error;
pub mod feishu;
pub mod filesystem;
pub mod github;
pub mod http_request;
pub mod memory_tools;
pub mod shell;
pub mod traits;
pub mod web_fetch;
pub mod web_search;

pub use browser::BrowserAdapter;
pub use calendar::CalendarAdapter;
pub use cron::{CronAdapter, CronJob};
pub use email::EmailAdapter;
pub use error::{AdapterError, Result};
pub use feishu::FeishuAdapter;
pub use filesystem::FilesystemAdapter;
pub use github::GitHubAdapter;
pub use http_request::HttpRequestAdapter;
pub use memory_tools::MemoryToolsAdapter;
pub use shell::ShellAdapter;
pub use traits::{Adapter, AdapterType, AuthRequirement, HealthStatus, ToolDefinition};
pub use web_fetch::WebFetchAdapter;
pub use web_search::WebSearchAdapter;
