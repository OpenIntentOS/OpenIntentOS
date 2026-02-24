//! Color theme constants for the desktop UI.
//!
//! These constants define the visual palette used throughout the iced
//! application, providing a consistent dark-mode appearance.

use iced::Color;

/// Main application background color.
pub const BACKGROUND: Color = Color::from_rgb(0.12, 0.12, 0.15);

/// Surface color for panels and cards.
pub const SURFACE: Color = Color::from_rgb(0.18, 0.18, 0.22);

/// Primary accent color.
pub const PRIMARY: Color = Color::from_rgb(0.35, 0.55, 0.95);

/// Standard text color.
pub const TEXT: Color = Color::from_rgb(0.9, 0.9, 0.92);

/// Dimmed text color for secondary information.
pub const TEXT_DIM: Color = Color::from_rgb(0.5, 0.5, 0.55);

/// Color for user messages.
pub const USER_COLOR: Color = Color::from_rgb(0.4, 0.7, 1.0);

/// Color for assistant messages.
pub const ASSISTANT_COLOR: Color = Color::from_rgb(0.4, 0.9, 0.5);

/// Color for error messages.
pub const ERROR_COLOR: Color = Color::from_rgb(1.0, 0.4, 0.4);

/// Color for tool invocation messages.
pub const TOOL_COLOR: Color = Color::from_rgb(1.0, 0.8, 0.3);

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_is_dark() {
        const { assert!(BACKGROUND.r < 0.2) };
        const { assert!(BACKGROUND.g < 0.2) };
        const { assert!(BACKGROUND.b < 0.2) };
    }

    #[test]
    fn text_is_bright() {
        const { assert!(TEXT.r > 0.8) };
        const { assert!(TEXT.g > 0.8) };
        const { assert!(TEXT.b > 0.8) };
    }

    #[test]
    fn primary_is_blue_ish() {
        const { assert!(PRIMARY.b > PRIMARY.r) };
        const { assert!(PRIMARY.b > PRIMARY.g) };
    }

    #[test]
    fn error_color_is_reddish() {
        const { assert!(ERROR_COLOR.r > ERROR_COLOR.g) };
        const { assert!(ERROR_COLOR.r > ERROR_COLOR.b) };
    }

    #[test]
    fn tool_color_is_yellowish() {
        const { assert!(TOOL_COLOR.r > TOOL_COLOR.b) };
        const { assert!(TOOL_COLOR.g > TOOL_COLOR.b) };
    }

    #[test]
    fn all_colors_are_opaque() {
        let colors = [
            BACKGROUND,
            SURFACE,
            PRIMARY,
            TEXT,
            TEXT_DIM,
            USER_COLOR,
            ASSISTANT_COLOR,
            ERROR_COLOR,
            TOOL_COLOR,
        ];
        for color in &colors {
            assert!(
                (color.a - 1.0).abs() < f32::EPSILON,
                "Color should be fully opaque"
            );
        }
    }
}
