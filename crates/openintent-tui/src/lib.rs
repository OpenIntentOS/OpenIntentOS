//! Terminal UI for OpenIntentOS.
//!
//! This crate provides a rich terminal user interface using [`ratatui`] and
//! [`crossterm`].  It includes:
//!
//! - A three-panel vertical layout: header, scrollable chat, and input.
//! - Background agent execution via tokio tasks and channels.
//! - Real-time display of tool invocations and agent responses.
//! - Keyboard navigation for scrolling and input editing.
//!
//! # Quick start
//!
//! ```ignore
//! use openintent_tui::run_tui;
//!
//! run_tui(llm, adapters, config, system_prompt).await?;
//! ```

pub mod app;
pub mod error;
pub mod run;
pub mod ui;

pub use app::{ChatEntry, TuiApp};
pub use error::{Result, TuiError};
pub use run::run_tui;
