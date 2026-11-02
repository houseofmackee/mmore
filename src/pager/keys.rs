//! Key bindings.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    LineDown,
    LineUp,
    PageDown,
    PageUp,
    HalfDown,
    HalfUp,
    Top,
    Bottom,
    SearchForward,
    SearchBackward,
    NextMatch,
    PrevMatch,
    ClearSearch,
    /// Start a `:` command (`:n`, `:p`, `:f`, `:q`).
    Command,
    ShowPosition,
    ToggleRaw,
    Help,
    /// Go to the end and keep following new input (`F`, as in less).
    Follow,
    /// Stop the process (Ctrl-Z).
    Suspend,
    Quit,
    None,
}

impl Action {
    /// Whether the action moves towards the end of the document.
    pub fn moves_forward(self) -> bool {
        matches!(
            self,
            Action::LineDown | Action::PageDown | Action::HalfDown | Action::Bottom
        )
    }
}

pub fn map(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('f') if ctrl => Action::PageDown,
        KeyCode::Char('b') if ctrl => Action::PageUp,
        KeyCode::Char('d') if ctrl => Action::HalfDown,
        KeyCode::Char('u') if ctrl => Action::HalfUp,
        KeyCode::Char('n') if ctrl => Action::LineDown,
        KeyCode::Char('p') if ctrl => Action::LineUp,
        KeyCode::Char('g') if ctrl => Action::ShowPosition,
        KeyCode::Char('z') if ctrl => Action::Suspend,
        _ if ctrl => Action::None,

        KeyCode::Char('q' | 'Q') => Action::Quit,
        KeyCode::Char(' ' | 'f') | KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('b') | KeyCode::PageUp => Action::PageUp,
        KeyCode::Enter | KeyCode::Char('j' | 'e') | KeyCode::Down => Action::LineDown,
        KeyCode::Char('k' | 'y') | KeyCode::Up => Action::LineUp,
        KeyCode::Char('d') => Action::HalfDown,
        KeyCode::Char('u') => Action::HalfUp,
        KeyCode::Char('g' | '<') | KeyCode::Home => Action::Top,
        KeyCode::Char('G' | '>') | KeyCode::End => Action::Bottom,
        KeyCode::Char('/') => Action::SearchForward,
        KeyCode::Char('?') => Action::SearchBackward,
        KeyCode::Char('n') => Action::NextMatch,
        KeyCode::Char('N') => Action::PrevMatch,
        KeyCode::Esc => Action::ClearSearch,
        KeyCode::Char(':') => Action::Command,
        KeyCode::Char('=') => Action::ShowPosition,
        KeyCode::Char('m') => Action::ToggleRaw,
        KeyCode::Char('h' | 'H') | KeyCode::F(1) => Action::Help,
        KeyCode::Char('F') => Action::Follow,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn more_style_keys() {
        assert_eq!(map(key(KeyCode::Char(' '))), Action::PageDown);
        assert_eq!(map(key(KeyCode::Enter)), Action::LineDown);
        assert_eq!(map(key(KeyCode::Char('b'))), Action::PageUp);
        assert_eq!(map(key(KeyCode::Char('q'))), Action::Quit);
    }

    #[test]
    fn control_keys() {
        assert_eq!(map(ctrl('c')), Action::Quit);
        assert_eq!(map(ctrl('d')), Action::HalfDown);
        assert_eq!(map(ctrl('q')), Action::None);
    }

    #[test]
    fn search_and_feature_keys() {
        assert_eq!(map(key(KeyCode::Char('/'))), Action::SearchForward);
        assert_eq!(map(key(KeyCode::Char('?'))), Action::SearchBackward);
        assert_eq!(map(key(KeyCode::Char('n'))), Action::NextMatch);
        assert_eq!(map(key(KeyCode::Char('N'))), Action::PrevMatch);
        assert_eq!(map(key(KeyCode::Char(':'))), Action::Command);
        assert_eq!(map(key(KeyCode::Char('='))), Action::ShowPosition);
        assert_eq!(map(key(KeyCode::Char('m'))), Action::ToggleRaw);
        assert_eq!(map(key(KeyCode::Char('h'))), Action::Help);
        // Ctrl-N still moves a line, it is not "next match".
        assert_eq!(map(ctrl('n')), Action::LineDown);
        assert_eq!(map(key(KeyCode::Char('F'))), Action::Follow);
        assert_eq!(map(ctrl('z')), Action::Suspend);
    }

    #[test]
    fn shifted_g_is_bottom() {
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(map(shift_g), Action::Bottom);
    }
}
