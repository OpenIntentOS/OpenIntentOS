//! Iced desktop GUI for OpenIntentOS.
//!
//! Provides a native desktop chat interface built with the iced GUI framework.
//! The application presents a three-panel layout: header, scrollable chat
//! messages area, and an input bar for sending messages.

pub mod app;
pub mod chat;
pub mod error;
pub mod launcher;
pub mod theme;

pub use app::run_desktop_ui;
pub use error::{Result, UiError};
