//! Main iced desktop application.
//!
//! Implements the three-panel chat layout using iced 0.13's builder-pattern
//! API.  The application communicates with a background agent task via
//! [`tokio::sync::mpsc`] channels, following the same architecture as the
//! TUI crate.

use std::sync::Arc;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Color, Element, Length, Subscription, Task, Theme};
use tokio::sync::{Mutex, mpsc};

use crate::chat::{ChatMessage, MessageRole};
use crate::theme;

// ---------------------------------------------------------------------------
// Agent event types
// ---------------------------------------------------------------------------

/// Events sent from a background agent task to the UI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// The agent produced a text response.
    Response(String),
    /// A tool invocation has started.
    ToolStart(String),
    /// A tool invocation has completed.
    ToolEnd(String),
    /// An error occurred during agent execution.
    Error(String),
}

// ---------------------------------------------------------------------------
// Application messages
// ---------------------------------------------------------------------------

/// All messages that can be dispatched within the iced runtime.
#[derive(Debug, Clone)]
pub enum Message {
    /// The user changed text in the input field.
    InputChanged(String),
    /// The user pressed submit (button click or Enter key).
    Submit,
    /// An event arrived from the background agent.
    AgentEvent(AgentEvent),
    /// No-op message used when there is nothing to report.
    Tick,
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// The main desktop application state.
struct OpenIntentApp {
    /// Chat history displayed in the scrollable area.
    messages: Vec<ChatMessage>,
    /// Current text in the input field.
    input: String,
    /// Whether the agent is processing a request.
    thinking: bool,
    /// Sender half — kept for spawning agent tasks.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Receiver half, wrapped in Arc<Mutex> so the subscription can share it.
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
}

impl OpenIntentApp {
    /// Initialize the application state and return any startup tasks.
    fn new() -> (Self, Task<Message>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let app = Self {
            messages: vec![ChatMessage::system(
                "Welcome to OpenIntentOS Desktop. Type a message below to get started.",
            )],
            input: String::new(),
            thinking: false,
            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        };

        (app, Task::none())
    }

    /// Process a message and return any follow-up tasks.
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::InputChanged(value) => {
                self.input = value;
                Task::none()
            }
            Message::Submit => {
                let trimmed = self.input.trim().to_owned();
                if trimmed.is_empty() || self.thinking {
                    return Task::none();
                }

                // Add the user message to chat history.
                self.messages.push(ChatMessage::user(&trimmed));
                self.input.clear();
                self.thinking = true;

                // Since the agent is not wired yet, simulate a placeholder
                // response after a short delay.
                let tx = self.event_tx.clone();
                Task::perform(
                    async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let _ = tx.send(AgentEvent::Response(
                            "Agent not connected yet. This is a placeholder response.".to_owned(),
                        ));
                    },
                    |()| Message::Tick,
                )
            }
            Message::AgentEvent(event) => {
                match event {
                    AgentEvent::Response(resp) => {
                        tracing::debug!("agent response received");
                        self.messages.push(ChatMessage::assistant(resp));
                        self.thinking = false;
                    }
                    AgentEvent::ToolStart(name) => {
                        tracing::debug!(tool = %name, "tool invocation started");
                        self.messages
                            .push(ChatMessage::tool(format!("Calling tool: {name}")));
                    }
                    AgentEvent::ToolEnd(result) => {
                        let display = if result.len() > 500 {
                            format!("{}... (truncated)", &result[..500])
                        } else {
                            result
                        };
                        self.messages.push(ChatMessage::tool(display));
                    }
                    AgentEvent::Error(msg) => {
                        tracing::warn!(error = %msg, "agent error");
                        self.messages
                            .push(ChatMessage::error(format!("Error: {msg}")));
                        self.thinking = false;
                    }
                }
                Task::none()
            }
            Message::Tick => Task::none(),
        }
    }

    /// Build the view tree for the current application state.
    fn view(&self) -> Element<'_, Message> {
        // -- Header --
        let header = container(text("OpenIntentOS v0.1.0").size(20).color(theme::TEXT))
            .width(Length::Fill)
            .padding(12)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Background::Color(theme::SURFACE)),
                ..Default::default()
            });

        // -- Chat messages --
        let mut chat_column = column![].spacing(8).padding(12);

        for msg in &self.messages {
            let (label, color) = match msg.role {
                MessageRole::User => ("You", theme::USER_COLOR),
                MessageRole::Assistant => ("AI", theme::ASSISTANT_COLOR),
                MessageRole::System => ("System", theme::TEXT_DIM),
                MessageRole::Tool => ("Tool", theme::TOOL_COLOR),
                MessageRole::Error => ("Error", theme::ERROR_COLOR),
            };

            let role_text = text(format!("[{label}]")).size(14).color(color);

            let content_text = text(msg.content.clone()).size(14).color(theme::TEXT);

            let timestamp_text = text(msg.timestamp.clone()).size(11).color(theme::TEXT_DIM);

            let msg_row = row![role_text, content_text].spacing(8);

            let msg_with_time = column![msg_row, timestamp_text].spacing(2);

            chat_column = chat_column.push(msg_with_time);
        }

        // Thinking indicator
        if self.thinking {
            let thinking_text = text("Thinking...").size(14).color(theme::TEXT_DIM);
            chat_column = chat_column.push(thinking_text);
        }

        let chat_area = scrollable(container(chat_column).width(Length::Fill))
            .height(Length::Fill)
            .width(Length::Fill);

        // -- Input bar --
        let input_field = text_input("Type a message...", &self.input)
            .on_input(Message::InputChanged)
            .on_submit(Message::Submit)
            .padding(10)
            .size(14)
            .width(Length::Fill);

        let submit_btn = button(text("Send").size(14).color(Color::WHITE))
            .on_press(Message::Submit)
            .padding([8, 16]);

        let input_bar = container(row![input_field, submit_btn].spacing(8).padding(8))
            .width(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Background::Color(theme::SURFACE)),
                ..Default::default()
            });

        // -- Full layout --
        let content = column![header, chat_area, input_bar];

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Background::Color(theme::BACKGROUND)),
                ..Default::default()
            })
            .into()
    }

    /// Set up subscriptions for receiving agent events via the mpsc channel.
    ///
    /// Uses `iced::stream::channel` to bridge the tokio mpsc receiver into
    /// an iced-compatible stream, which is then wrapped in a named
    /// subscription via `Subscription::run_with_id`.
    fn subscription(&self) -> Subscription<Message> {
        let rx = Arc::clone(&self.event_rx);

        Subscription::run_with_id(
            "agent-events",
            iced::stream::channel(16, move |mut output| async move {
                use iced::futures::SinkExt;
                loop {
                    let event = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };
                    match event {
                        Some(evt) => {
                            let _ = output.send(Message::AgentEvent(evt)).await;
                        }
                        None => {
                            // Channel closed; keep subscription alive but idle.
                            std::future::pending::<()>().await;
                        }
                    }
                }
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Launch the iced desktop UI application.
///
/// This is the main entry point called from the CLI or other binary crates.
pub fn run_desktop_ui() -> iced::Result {
    iced::application("OpenIntentOS", OpenIntentApp::update, OpenIntentApp::view)
        .subscription(OpenIntentApp::subscription)
        .window_size((900.0, 650.0))
        .run_with(OpenIntentApp::new)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_has_welcome_message() {
        let (app, _task) = OpenIntentApp::new();
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].role, MessageRole::System);
        assert!(app.messages[0].content.contains("Welcome"));
    }

    #[test]
    fn new_app_is_not_thinking() {
        let (app, _task) = OpenIntentApp::new();
        assert!(!app.thinking);
    }

    #[test]
    fn new_app_has_empty_input() {
        let (app, _task) = OpenIntentApp::new();
        assert!(app.input.is_empty());
    }

    #[test]
    fn input_changed_updates_input() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::InputChanged("hello".to_owned()));
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn submit_with_empty_input_does_nothing() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::Submit);
        assert_eq!(app.messages.len(), 1);
        assert!(!app.thinking);
    }

    #[test]
    fn submit_with_whitespace_only_does_nothing() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::InputChanged("   ".to_owned()));
        let _ = app.update(Message::Submit);
        assert_eq!(app.messages.len(), 1);
        assert!(!app.thinking);
    }

    #[test]
    fn submit_adds_user_message_and_sets_thinking() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::InputChanged("Hello agent".to_owned()));
        let _ = app.update(Message::Submit);
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].role, MessageRole::User);
        assert_eq!(app.messages[1].content, "Hello agent");
        assert!(app.thinking);
        assert!(app.input.is_empty());
    }

    #[test]
    fn submit_while_thinking_is_ignored() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::InputChanged("first".to_owned()));
        let _ = app.update(Message::Submit);
        let _ = app.update(Message::InputChanged("second".to_owned()));
        let _ = app.update(Message::Submit);
        assert_eq!(app.messages.len(), 2);
    }

    #[test]
    fn agent_response_adds_assistant_message_and_clears_thinking() {
        let (mut app, _task) = OpenIntentApp::new();
        app.thinking = true;
        let _ = app.update(Message::AgentEvent(AgentEvent::Response(
            "Hello!".to_owned(),
        )));
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].role, MessageRole::Assistant);
        assert_eq!(app.messages[1].content, "Hello!");
        assert!(!app.thinking);
    }

    #[test]
    fn agent_error_adds_error_message_and_clears_thinking() {
        let (mut app, _task) = OpenIntentApp::new();
        app.thinking = true;
        let _ = app.update(Message::AgentEvent(AgentEvent::Error("timeout".to_owned())));
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].role, MessageRole::Error);
        assert!(app.messages[1].content.contains("timeout"));
        assert!(!app.thinking);
    }

    #[test]
    fn tool_start_adds_tool_message() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::AgentEvent(AgentEvent::ToolStart(
            "filesystem".to_owned(),
        )));
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].role, MessageRole::Tool);
        assert!(app.messages[1].content.contains("filesystem"));
    }

    #[test]
    fn tool_end_truncates_long_results() {
        let (mut app, _task) = OpenIntentApp::new();
        let long_result = "x".repeat(600);
        let _ = app.update(Message::AgentEvent(AgentEvent::ToolEnd(long_result)));
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].role, MessageRole::Tool);
        assert!(app.messages[1].content.contains("(truncated)"));
        assert!(app.messages[1].content.len() < 600);
    }

    #[test]
    fn tool_end_short_result_not_truncated() {
        let (mut app, _task) = OpenIntentApp::new();
        let _ = app.update(Message::AgentEvent(AgentEvent::ToolEnd("done".to_owned())));
        assert_eq!(app.messages[1].content, "done");
    }

    #[test]
    fn tick_message_is_noop() {
        let (mut app, _task) = OpenIntentApp::new();
        let msg_count = app.messages.len();
        let _ = app.update(Message::Tick);
        assert_eq!(app.messages.len(), msg_count);
    }

    #[test]
    fn event_tx_can_send() {
        let (app, _task) = OpenIntentApp::new();
        let result = app.event_tx.send(AgentEvent::Response("test".to_owned()));
        assert!(result.is_ok());
    }
}
