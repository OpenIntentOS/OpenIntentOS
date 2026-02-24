//! Main TUI application state and input handling.
//!
//! [`TuiApp`] holds the chat history, input buffer, scroll state, and agent
//! integration.  It communicates with background agent tasks via a
//! [`tokio::sync::mpsc`] channel.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{AgentConfig, ChatRequest, LlmClient, LlmResponse, Message, ToolDefinition};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single entry in the chat display.
#[derive(Debug, Clone)]
pub struct ChatEntry {
    /// The role: `"user"`, `"assistant"`, `"tool_start"`, `"tool_end"`, or
    /// `"error"`.
    pub role: String,
    /// The display content.
    pub content: String,
}

impl ChatEntry {
    /// Create a new chat entry.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// Events sent from the background agent task to the UI loop.
#[derive(Debug)]
pub enum AgentEvent {
    /// A tool invocation has started.
    ToolStart(String),
    /// A tool invocation completed with the given result.
    ToolEnd(String),
    /// The agent produced a final text response.
    Response(String),
    /// An error occurred during agent execution.
    Error(String),
}

/// Actions the UI loop should take after processing a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    /// Continue the main loop.
    Continue,
    /// Exit the application.
    Quit,
}

// ---------------------------------------------------------------------------
// TuiApp
// ---------------------------------------------------------------------------

/// The main TUI application state.
pub struct TuiApp {
    /// Current text in the input field.
    input: String,
    /// Cursor position within the input field.
    cursor_pos: usize,
    /// Chat messages displayed in the messages area.
    messages: Vec<ChatEntry>,
    /// Vertical scroll offset for the messages area.
    scroll_offset: u16,
    /// Whether the agent is currently processing a request.
    thinking: bool,
    /// The LLM client used for agent requests.
    llm: Arc<LlmClient>,
    /// Registered tool adapters.
    adapters: Vec<Arc<dyn ToolAdapter>>,
    /// Agent configuration (model, max turns, etc.).
    config: AgentConfig,
    /// System prompt prepended to every conversation.
    system_prompt: String,
    /// Conversation history maintained for LLM context.
    agent_messages: Vec<Message>,
    /// Receiver for events from background agent tasks.
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    /// Sender cloned into spawned agent tasks.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

impl TuiApp {
    /// Create a new TUI application with the given agent components.
    pub fn new(
        llm: Arc<LlmClient>,
        adapters: Vec<Arc<dyn ToolAdapter>>,
        config: AgentConfig,
        system_prompt: String,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let agent_messages = vec![Message::system(&system_prompt)];

        Self {
            input: String::new(),
            cursor_pos: 0,
            messages: vec![ChatEntry::new(
                "system",
                "Welcome to OpenIntentOS TUI. Type a message and press Enter.",
            )],
            scroll_offset: 0,
            thinking: false,
            llm,
            adapters,
            config,
            system_prompt,
            agent_messages,
            event_rx,
            event_tx,
        }
    }

    // -- Accessors ----------------------------------------------------------

    /// Return the current input text.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Return the cursor position within the input.
    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    /// Return all chat messages.
    pub fn messages(&self) -> &[ChatEntry] {
        &self.messages
    }

    /// Return the current scroll offset.
    pub fn scroll_offset(&self) -> u16 {
        self.scroll_offset
    }

    /// Return whether the agent is currently thinking.
    pub fn is_thinking(&self) -> bool {
        self.thinking
    }

    /// Return the model name from the config.
    pub fn model_name(&self) -> &str {
        if self.config.model.is_empty() {
            "default"
        } else {
            &self.config.model
        }
    }

    /// Return the system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    // -- Key handling -------------------------------------------------------

    /// Handle a key event and return the action the UI should take.
    pub fn handle_key(&mut self, key: KeyEvent) -> AppAction {
        // Ctrl+C or Escape always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return AppAction::Quit;
        }
        if key.code == KeyCode::Esc {
            return AppAction::Quit;
        }

        match key.code {
            KeyCode::Enter => {
                self.submit_input();
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::Up => {
                self.scroll_up(1);
            }
            KeyCode::Down => {
                self.scroll_down(1);
            }
            KeyCode::PageUp => {
                self.scroll_up(10);
            }
            KeyCode::PageDown => {
                self.scroll_down(10);
            }
            _ => {}
        }

        AppAction::Continue
    }

    // -- Scrolling ----------------------------------------------------------

    /// Scroll up by the given number of lines.
    fn scroll_up(&mut self, lines: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
    }

    /// Scroll down by the given number of lines.
    fn scroll_down(&mut self, lines: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    // -- Input submission ---------------------------------------------------

    /// Submit the current input: add it to chat, spawn the agent task.
    fn submit_input(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() || self.thinking {
            return;
        }

        // Add user message to display.
        self.messages.push(ChatEntry::new("user", &text));

        // Add to LLM context.
        self.agent_messages.push(Message::user(&text));

        // Clear input.
        self.input.clear();
        self.cursor_pos = 0;

        // Reset scroll to bottom.
        self.scroll_offset = 0;

        // Mark as thinking and spawn agent task.
        self.thinking = true;
        self.spawn_agent_task();
    }

    /// Spawn a background tokio task to run the ReAct loop.
    fn spawn_agent_task(&self) {
        let llm = Arc::clone(&self.llm);
        let adapters = self.adapters.clone();
        let config = self.config.clone();
        let messages = self.agent_messages.clone();
        let tx = self.event_tx.clone();

        // Collect tool definitions from all adapters.
        let tools: Vec<ToolDefinition> =
            adapters.iter().flat_map(|a| a.tool_definitions()).collect();

        tokio::spawn(async move {
            if let Err(e) = run_agent_loop(llm, &adapters, &config, messages, &tools, &tx).await {
                let _ = tx.send(AgentEvent::Error(e.to_string()));
            }
        });
    }

    // -- Agent event polling ------------------------------------------------

    /// Poll the agent event channel and update state accordingly.
    ///
    /// Should be called on every iteration of the main UI loop.
    pub fn check_agent_response(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AgentEvent::ToolStart(name) => {
                    tracing::debug!(tool = %name, "tool invocation started");
                    self.messages.push(ChatEntry::new(
                        "tool_start",
                        format!("Calling tool: {name}"),
                    ));
                    self.scroll_offset = 0;
                }
                AgentEvent::ToolEnd(result) => {
                    // Truncate long tool results for display.
                    let display = if result.len() > 500 {
                        format!("{}... (truncated)", &result[..500])
                    } else {
                        result
                    };
                    self.messages.push(ChatEntry::new("tool_end", display));
                    self.scroll_offset = 0;
                }
                AgentEvent::Response(text) => {
                    tracing::debug!("agent response received");
                    self.messages.push(ChatEntry::new("assistant", &text));
                    // Append assistant message to LLM context.
                    self.agent_messages.push(Message::assistant(&text));
                    self.thinking = false;
                    self.scroll_offset = 0;
                }
                AgentEvent::Error(msg) => {
                    tracing::warn!(error = %msg, "agent error");
                    self.messages
                        .push(ChatEntry::new("error", format!("Error: {msg}")));
                    self.thinking = false;
                    self.scroll_offset = 0;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Background agent loop
// ---------------------------------------------------------------------------

/// Run the ReAct loop in the background, sending events through the channel.
///
/// This mirrors the pattern used by the WebSocket handler: it builds a
/// [`ChatRequest`], calls [`LlmClient::stream_chat`], and handles tool calls
/// iteratively until a text response is produced or the turn limit is reached.
async fn run_agent_loop(
    llm: Arc<LlmClient>,
    adapters: &[Arc<dyn ToolAdapter>],
    config: &AgentConfig,
    mut messages: Vec<Message>,
    tools: &[ToolDefinition],
    tx: &mpsc::UnboundedSender<AgentEvent>,
) -> std::result::Result<(), openintent_agent::AgentError> {
    for _turn in 0..config.max_turns {
        let request = ChatRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            tools: tools.to_vec(),
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: true,
        };

        let response = llm.stream_chat(&request).await?;

        match response {
            LlmResponse::Text(text) => {
                messages.push(Message::assistant(&text));
                let _ = tx.send(AgentEvent::Response(text));
                return Ok(());
            }
            LlmResponse::ToolCalls(calls) => {
                messages.push(Message::assistant_tool_calls(calls.clone()));

                for call in &calls {
                    let _ = tx.send(AgentEvent::ToolStart(call.name.clone()));

                    let adapter = adapters
                        .iter()
                        .find(|a| a.tool_definitions().iter().any(|td| td.name == call.name));

                    let result_str = match adapter {
                        Some(a) => match a.execute(&call.name, call.arguments.clone()).await {
                            Ok(r) => r,
                            Err(e) => format!("Error: {e}"),
                        },
                        None => format!("Error: unknown tool `{}`", call.name),
                    };

                    let _ = tx.send(AgentEvent::ToolEnd(result_str.clone()));
                    messages.push(Message::tool_result(&call.id, &result_str));
                }
            }
        }
    }

    let _ = tx.send(AgentEvent::Response(
        "Reached maximum number of reasoning turns.".to_owned(),
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn make_key_with_mods(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn make_app() -> TuiApp {
        let llm_config = openintent_agent::LlmClientConfig::anthropic("test-key", "test-model");
        let llm = Arc::new(openintent_agent::LlmClient::new(llm_config).unwrap());
        let config = AgentConfig {
            model: "test-model".to_owned(),
            ..AgentConfig::default()
        };
        TuiApp::new(llm, vec![], config, "test prompt".to_owned())
    }

    #[test]
    fn typing_characters_appends_to_input() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::Char('h')));
        app.handle_key(make_key(KeyCode::Char('i')));
        assert_eq!(app.input(), "hi");
        assert_eq!(app.cursor_pos(), 2);
    }

    #[test]
    fn backspace_removes_character() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::Char('a')));
        app.handle_key(make_key(KeyCode::Char('b')));
        app.handle_key(make_key(KeyCode::Backspace));
        assert_eq!(app.input(), "a");
        assert_eq!(app.cursor_pos(), 1);
    }

    #[test]
    fn escape_returns_quit() {
        let mut app = make_app();
        assert_eq!(app.handle_key(make_key(KeyCode::Esc)), AppAction::Quit);
    }

    #[test]
    fn ctrl_c_returns_quit() {
        let mut app = make_app();
        let action = app.handle_key(make_key_with_mods(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn arrow_keys_scroll() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::Up));
        assert_eq!(app.scroll_offset(), 1);
        app.handle_key(make_key(KeyCode::Down));
        assert_eq!(app.scroll_offset(), 0);
    }

    #[test]
    fn page_up_down_scroll() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::PageUp));
        assert_eq!(app.scroll_offset(), 10);
        app.handle_key(make_key(KeyCode::PageDown));
        assert_eq!(app.scroll_offset(), 0);
    }

    #[test]
    fn home_end_move_cursor() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::Char('a')));
        app.handle_key(make_key(KeyCode::Char('b')));
        app.handle_key(make_key(KeyCode::Char('c')));
        app.handle_key(make_key(KeyCode::Home));
        assert_eq!(app.cursor_pos(), 0);
        app.handle_key(make_key(KeyCode::End));
        assert_eq!(app.cursor_pos(), 3);
    }

    #[test]
    fn left_right_arrows_move_cursor() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::Char('x')));
        app.handle_key(make_key(KeyCode::Char('y')));
        app.handle_key(make_key(KeyCode::Left));
        assert_eq!(app.cursor_pos(), 1);
        app.handle_key(make_key(KeyCode::Right));
        assert_eq!(app.cursor_pos(), 2);
    }

    #[test]
    fn empty_enter_does_nothing() {
        let mut app = make_app();
        let initial_count = app.messages().len();
        app.handle_key(make_key(KeyCode::Enter));
        assert_eq!(app.messages().len(), initial_count);
    }

    #[test]
    fn chat_entry_creation() {
        let entry = ChatEntry::new("user", "Hello there");
        assert_eq!(entry.role, "user");
        assert_eq!(entry.content, "Hello there");
    }

    #[test]
    fn model_name_returns_config_model() {
        let app = make_app();
        assert_eq!(app.model_name(), "test-model");
    }

    #[test]
    fn delete_key_removes_character_after_cursor() {
        let mut app = make_app();
        app.handle_key(make_key(KeyCode::Char('a')));
        app.handle_key(make_key(KeyCode::Char('b')));
        app.handle_key(make_key(KeyCode::Home));
        app.handle_key(make_key(KeyCode::Delete));
        assert_eq!(app.input(), "b");
        assert_eq!(app.cursor_pos(), 0);
    }
}
