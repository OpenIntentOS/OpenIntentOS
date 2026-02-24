//! Quick-launch input bar widget state.
//!
//! [`LauncherBar`] manages the text input state for the bottom input bar
//! in the desktop UI. It can be reused wherever a text submission field
//! is needed.

/// State for a reusable text input bar with submit functionality.
#[derive(Debug, Clone)]
pub struct LauncherBar {
    /// The current input text.
    input: String,
    /// Placeholder text shown when the input is empty.
    placeholder: String,
}

impl LauncherBar {
    /// Create a new launcher bar with the given placeholder text.
    pub fn new(placeholder: &str) -> Self {
        Self {
            input: String::new(),
            placeholder: placeholder.to_owned(),
        }
    }

    /// Return a reference to the current input text.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Return a reference to the placeholder text.
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Replace the current input with the given text.
    pub fn set_input(&mut self, text: String) {
        self.input = text;
    }

    /// Clear the current input text.
    pub fn clear(&mut self) {
        self.input.clear();
    }

    /// Return `true` if the input is empty or contains only whitespace.
    pub fn is_empty(&self) -> bool {
        self.input.trim().is_empty()
    }

    /// Take the trimmed input text and clear the buffer, returning the
    /// submitted text. Returns `None` if the input was blank.
    pub fn take_input(&mut self) -> Option<String> {
        let trimmed = self.input.trim().to_owned();
        if trimmed.is_empty() {
            return None;
        }
        self.input.clear();
        Some(trimmed)
    }
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_launcher_has_empty_input() {
        let bar = LauncherBar::new("Type here...");
        assert!(bar.input().is_empty());
        assert_eq!(bar.placeholder(), "Type here...");
    }

    #[test]
    fn set_input_updates_text() {
        let mut bar = LauncherBar::new("placeholder");
        bar.set_input("hello".to_owned());
        assert_eq!(bar.input(), "hello");
    }

    #[test]
    fn clear_empties_input() {
        let mut bar = LauncherBar::new("placeholder");
        bar.set_input("some text".to_owned());
        bar.clear();
        assert!(bar.input().is_empty());
    }

    #[test]
    fn is_empty_with_whitespace() {
        let mut bar = LauncherBar::new("placeholder");
        assert!(bar.is_empty());
        bar.set_input("   ".to_owned());
        assert!(bar.is_empty());
        bar.set_input("hello".to_owned());
        assert!(!bar.is_empty());
    }

    #[test]
    fn take_input_returns_trimmed_text() {
        let mut bar = LauncherBar::new("placeholder");
        bar.set_input("  hello world  ".to_owned());
        let taken = bar.take_input();
        assert_eq!(taken.as_deref(), Some("hello world"));
        assert!(bar.input().is_empty());
    }

    #[test]
    fn take_input_returns_none_when_empty() {
        let mut bar = LauncherBar::new("placeholder");
        assert!(bar.take_input().is_none());
    }

    #[test]
    fn take_input_returns_none_for_whitespace() {
        let mut bar = LauncherBar::new("placeholder");
        bar.set_input("   ".to_owned());
        assert!(bar.take_input().is_none());
    }
}
