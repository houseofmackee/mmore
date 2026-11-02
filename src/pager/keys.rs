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
    fn shifted_g_is_bottom() {
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(map(shift_g), Action::Bottom);
    }
}
