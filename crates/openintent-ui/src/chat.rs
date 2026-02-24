//! Chat message types for the desktop UI.
//!
//! [`ChatMessage`] represents a single entry in the chat history, tagged
//! with a [`MessageRole`] to control styling and presentation.

/// A single chat message displayed in the conversation view.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// The role of the message author.
    pub role: MessageRole,
    /// The text content of the message.
    pub content: String,
    /// A formatted timestamp string for display.
    pub timestamp: String,
}

/// Identifies the author or category of a chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    /// A message from the user.
    User,
    /// A response from the AI assistant.
    Assistant,
    /// A system notification or informational message.
    System,
    /// A tool invocation or result message.
    Tool,
    /// An error message.
    Error,
}

impl ChatMessage {
    /// Create a user message with the current timestamp.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            timestamp: current_timestamp(),
        }
    }

    /// Create an assistant message with the current timestamp.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            timestamp: current_timestamp(),
        }
    }

    /// Create a system message with the current timestamp.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            timestamp: current_timestamp(),
        }
    }

    /// Create a tool message with the current timestamp.
    pub fn tool(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            timestamp: current_timestamp(),
        }
    }

    /// Create an error message with the current timestamp.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Error,
            content: content.into(),
            timestamp: current_timestamp(),
        }
    }
}

/// Return a simple HH:MM:SS timestamp string.
fn current_timestamp() -> String {
    // Use a basic approach that avoids heavy dependencies; chrono is in the
    // workspace but we keep it simple here with std.
    let now = std::time::SystemTime::now();
    let since_epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since_epoch.as_secs();
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_has_correct_role() {
        let msg = ChatMessage::user("Hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello");
        assert!(!msg.timestamp.is_empty());
    }

    #[test]
    fn assistant_message_has_correct_role() {
        let msg = ChatMessage::assistant("Hi there");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Hi there");
    }

    #[test]
    fn system_message_has_correct_role() {
        let msg = ChatMessage::system("Welcome");
        assert_eq!(msg.role, MessageRole::System);
        assert_eq!(msg.content, "Welcome");
    }

    #[test]
    fn tool_message_has_correct_role() {
        let msg = ChatMessage::tool("Calling filesystem...");
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.content, "Calling filesystem...");
    }

    #[test]
    fn error_message_has_correct_role() {
        let msg = ChatMessage::error("Something went wrong");
        assert_eq!(msg.role, MessageRole::Error);
        assert_eq!(msg.content, "Something went wrong");
    }

    #[test]
    fn timestamp_format_is_hh_mm_ss() {
        let ts = current_timestamp();
        // Should match pattern "XX:XX:XX"
        assert_eq!(ts.len(), 8);
        assert_eq!(ts.as_bytes()[2], b':');
        assert_eq!(ts.as_bytes()[5], b':');
    }

    #[test]
    fn message_accepts_string_and_str() {
        let from_str = ChatMessage::user("hello");
        let from_string = ChatMessage::user(String::from("hello"));
        assert_eq!(from_str.content, from_string.content);
    }

    #[test]
    fn message_role_equality() {
        assert_eq!(MessageRole::User, MessageRole::User);
        assert_ne!(MessageRole::User, MessageRole::Assistant);
        assert_ne!(MessageRole::Tool, MessageRole::Error);
    }
}
