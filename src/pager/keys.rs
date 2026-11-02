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
    /// A digit of a count, as in `120g` (line 120) or `50%`.
    Digit(u8),
    /// Go to a percentage of the file (`%` or `p`, after a count).
    Percent,
    /// Turn wrapping of long lines on or off.
    ToggleWrap,
    ScrollLeft,
    ScrollRight,
    /// Open the file in an editor.
    Edit,
    /// Show only matching lines (`&pattern`).
    Filter,
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
        KeyCode::Char(c @ '0'..='9') => Action::Digit(c as u8 - b'0'),
        KeyCode::Char('%' | 'p') => Action::Percent,
        KeyCode::Char('w') => Action::ToggleWrap,
        KeyCode::Left => Action::ScrollLeft,
        KeyCode::Right => Action::ScrollRight,
        KeyCode::Char('v') => Action::Edit,
        KeyCode::Char('&') => Action::Filter,
        _ => Action::None,
    }
}

/// Key presses from raw terminal input that was read before the pager started
/// (see `background::take_typeahead`). Covers what people type quickly: text,
/// Enter, Backspace, Ctrl-letters, arrows, Home/End, and Page Up/Down.
pub fn from_bytes(bytes: &[u8]) -> Vec<KeyEvent> {
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut keys = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        let event = match c {
            '\r' | '\n' => key(KeyCode::Enter),
            '\x7f' | '\x08' => key(KeyCode::Backspace),
            '\x1b' if chars.get(i) == Some(&'[') || chars.get(i) == Some(&'O') => {
                // CSI or SS3: ESC [ A, ESC O A, ESC [ 5 ~, ...
                let start = i + 1;
                let mut end = start;
                while chars
                    .get(end)
                    .is_some_and(|c| c.is_ascii_digit() || *c == ';')
                {
                    end += 1;
                }
                let Some(&last) = chars.get(end) else { break };
                let params: String = chars[start..end].iter().collect();
                i = end + 1;
                let code = match (last, params.as_str()) {
                    ('A', _) => KeyCode::Up,
                    ('B', _) => KeyCode::Down,
                    ('C', _) => KeyCode::Right,
                    ('D', _) => KeyCode::Left,
                    ('H', _) | ('~', "1" | "7") => KeyCode::Home,
                    ('F', _) | ('~', "4" | "8") => KeyCode::End,
                    ('~', "5") => KeyCode::PageUp,
                    ('~', "6") => KeyCode::PageDown,
                    _ => continue,
                };
                key(code)
            }
            '\x1b' => key(KeyCode::Esc),
            c @ '\x01'..='\x1a' => KeyEvent::new(
                KeyCode::Char((b'a' + (c as u8 - 1)) as char),
                KeyModifiers::CONTROL,
            ),
            c if c.is_control() => continue,
            c => key(KeyCode::Char(c)),
        };
        keys.push(event);
    }
    keys
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
        assert_eq!(map(key(KeyCode::Char('7'))), Action::Digit(7));
        assert_eq!(map(key(KeyCode::Char('%'))), Action::Percent);
        assert_eq!(map(key(KeyCode::Char('w'))), Action::ToggleWrap);
        assert_eq!(map(key(KeyCode::Right)), Action::ScrollRight);
        assert_eq!(map(key(KeyCode::Char('v'))), Action::Edit);
        assert_eq!(map(key(KeyCode::Char('&'))), Action::Filter);
        assert_eq!(map(ctrl('z')), Action::Suspend);
    }

    #[test]
    fn typed_bytes_become_keys() {
        let keys = from_bytes(b" q\r\x7f\x03\x1b[B\x1bOA\x1b[6~\x1b[H\x1b\x1b[Zx");
        let codes: Vec<KeyCode> = keys.iter().map(|k| k.code).collect();
        assert_eq!(
            codes,
            vec![
                KeyCode::Char(' '),
                KeyCode::Char('q'),
                KeyCode::Enter,
                KeyCode::Backspace,
                KeyCode::Char('c'),
                KeyCode::Down,
                KeyCode::Up,
                KeyCode::PageDown,
                KeyCode::Home,
                KeyCode::Esc,
                // ESC [ Z (Shift-Tab) is not a key mmore uses: skipped.
                KeyCode::Char('x'),
            ]
        );
        assert_eq!(map(keys[4]), Action::Quit);
        assert_eq!(map(keys[2]), Action::LineDown);
    }

    #[test]
    fn shifted_g_is_bottom() {
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(map(shift_g), Action::Bottom);
    }
}
