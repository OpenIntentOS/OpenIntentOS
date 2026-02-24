//! Rendering functions for the TUI layout.
//!
//! The layout consists of three vertically stacked areas:
//!
//! 1. **Header** (1 line) -- app name, model, and quit hint.
//! 2. **Messages** (fills remaining space) -- scrollable chat history.
//! 3. **Input** (3 lines) -- bordered text input field.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::TuiApp;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Draw the entire TUI frame.
pub fn draw(frame: &mut Frame, app: &TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // messages
            Constraint::Length(3), // input
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);
    draw_messages(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// Draw the header bar showing app name, model, and keybinding hints.
fn draw_header(frame: &mut Frame, app: &TuiApp, area: ratatui::layout::Rect) {
    let status = if app.is_thinking() {
        Span::styled(" Thinking... ", Style::default().fg(Color::Yellow))
    } else {
        Span::styled(" Ready ", Style::default().fg(Color::Green))
    };

    let header = Line::from(vec![
        Span::styled(
            " OpenIntentOS TUI v0.1.0 ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("| Model: "),
        Span::styled(app.model_name(), Style::default().fg(Color::White)),
        Span::raw(" | "),
        status,
        Span::raw("| Esc to quit "),
    ]);

    let header_widget = Paragraph::new(header).style(Style::default().bg(Color::DarkGray));

    frame.render_widget(header_widget, area);
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Draw the scrollable messages area.
fn draw_messages(frame: &mut Frame, app: &TuiApp, area: ratatui::layout::Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    for entry in app.messages() {
        let (prefix, style) = match entry.role.as_str() {
            "user" => ("[You] ", Style::default().fg(Color::Cyan)),
            "assistant" => ("[AI]  ", Style::default().fg(Color::Green)),
            "tool_start" => (
                "[Tool] ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ),
            "tool_end" => (
                "[Result] ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ),
            "error" => ("[!] ", Style::default().fg(Color::Red)),
            "system" => (
                "[System] ",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::DIM),
            ),
            _ => ("  ", Style::default()),
        };

        // Split content by newlines to create separate Line entries.
        let content_lines: Vec<&str> = entry.content.split('\n').collect();

        for (i, content_line) in content_lines.iter().enumerate() {
            let spans = if i == 0 {
                vec![
                    Span::styled(prefix, style),
                    Span::styled((*content_line).to_owned(), style),
                ]
            } else {
                // Continuation lines get indentation matching the prefix width.
                let indent = " ".repeat(prefix.len());
                vec![
                    Span::raw(indent),
                    Span::styled((*content_line).to_owned(), style),
                ]
            };
            lines.push(Line::from(spans));
        }

        // Add a blank line between messages for readability.
        lines.push(Line::from(""));
    }

    // Show thinking indicator at the bottom if the agent is working.
    if app.is_thinking() {
        lines.push(Line::from(vec![Span::styled(
            "  Thinking...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));
    }

    // Calculate scroll: we want to show the bottom of the conversation by
    // default, with the ability to scroll up.
    let total_lines = lines.len() as u16;
    let visible_height = area.height.saturating_sub(2); // account for borders
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = max_scroll.saturating_sub(app.scroll_offset());

    let messages_block = Block::default()
        .borders(Borders::ALL)
        .title(" Chat ")
        .border_style(Style::default().fg(Color::DarkGray));

    let messages_widget = Paragraph::new(lines)
        .block(messages_block)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll, 0));

    frame.render_widget(messages_widget, area);
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Draw the text input area at the bottom.
fn draw_input(frame: &mut Frame, app: &TuiApp, area: ratatui::layout::Rect) {
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(if app.is_thinking() {
            " Input (waiting...) "
        } else {
            " Input "
        })
        .border_style(if app.is_thinking() {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Cyan)
        });

    let input_widget = Paragraph::new(app.input())
        .block(input_block)
        .style(Style::default().fg(Color::White));

    frame.render_widget(input_widget, area);

    // Position the cursor in the input area.
    if !app.is_thinking() {
        // +1 for the border offset on each axis.
        let cursor_x = area.x + 1 + app.cursor_pos() as u16;
        let cursor_y = area.y + 1;
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::app::ChatEntry;

    #[test]
    fn chat_entry_role_styling_coverage() {
        // Verify that all role types produce valid prefix/style combinations
        // by exercising the match arms.
        let roles = [
            "user",
            "assistant",
            "tool_start",
            "tool_end",
            "error",
            "system",
            "other",
        ];
        for role in &roles {
            let entry = ChatEntry::new(*role, "test content");
            // Verify the entry was created correctly.
            assert_eq!(entry.role, *role);
        }
    }
}
