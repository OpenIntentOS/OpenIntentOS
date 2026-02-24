//! Main event loop for the terminal UI.
//!
//! Sets up the terminal in raw mode with an alternate screen, runs the
//! draw-and-poll loop, and restores the terminal on exit.

use std::io;
use std::sync::Arc;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{AgentConfig, LlmClient};

use crate::app::{AppAction, TuiApp};
use crate::error::Result;
use crate::ui;

/// Run the terminal UI event loop.
///
/// This function takes ownership of the terminal for the duration of the
/// session.  It enables raw mode and switches to an alternate screen buffer
/// so the user's existing terminal content is preserved.
///
/// # Arguments
///
/// * `llm` -- The LLM client for agent requests.
/// * `adapters` -- Tool adapters available to the agent.
/// * `config` -- Agent configuration (model, max turns, etc.).
/// * `system_prompt` -- System prompt prepended to conversations.
///
/// # Errors
///
/// Returns a [`TuiError`](crate::error::TuiError) if terminal setup, drawing,
/// or event handling fails.
pub async fn run_tui(
    llm: Arc<LlmClient>,
    adapters: Vec<Arc<dyn ToolAdapter>>,
    config: AgentConfig,
    system_prompt: String,
) -> Result<()> {
    // Set up the terminal.
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(llm, adapters, config, system_prompt);

    tracing::info!("TUI event loop started");

    // Main event loop.
    let result = event_loop(&mut terminal, &mut app).await;

    // Restore the terminal regardless of whether the loop succeeded.
    crossterm::terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    tracing::info!("TUI event loop ended");

    result
}

/// The inner event loop, separated so terminal cleanup always runs.
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
) -> Result<()> {
    loop {
        // Draw the current state.
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Poll for crossterm events with a short timeout so we can also
        // check for agent responses.
        if event::poll(std::time::Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == crossterm::event::KeyEventKind::Press
            && app.handle_key(key) == AppAction::Quit
        {
            break;
        }

        // Check for agent responses from the background task.
        app.check_agent_response();
    }

    Ok(())
}
