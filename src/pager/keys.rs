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
    /// Next or previous Markdown heading (`]` and `[`).
    NextHeading,
    PrevHeading,
    /// Table of contents (`t`).
    Contents,
    /// Load the file again (`R`).
    Reload,
    /// Set a mark (`m` and a letter) or go to one (`'` and a letter, `''`).
    SetMark,
    GoToMark,
    /// Select the next or previous link on screen (Tab, Shift-Tab).
    NextLink,
    PrevLink,
    /// Go back to where the last followed link was (Backspace).
    Back,
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

/// Actions that keys can be bound to in the config file, with their names.
pub const ACTIONS: &[(&str, Action)] = &[
    ("line-down", Action::LineDown),
    ("line-up", Action::LineUp),
    ("page-down", Action::PageDown),
    ("page-up", Action::PageUp),
    ("half-page-down", Action::HalfDown),
    ("half-page-up", Action::HalfUp),
    ("top", Action::Top),
    ("bottom", Action::Bottom),
    ("percent", Action::Percent),
    ("search-forward", Action::SearchForward),
    ("search-backward", Action::SearchBackward),
    ("next-match", Action::NextMatch),
    ("previous-match", Action::PrevMatch),
    ("clear-search", Action::ClearSearch),
    ("filter", Action::Filter),
    ("command", Action::Command),
    ("show-position", Action::ShowPosition),
    ("toggle-raw", Action::ToggleRaw),
    ("toggle-wrap", Action::ToggleWrap),
    ("scroll-left", Action::ScrollLeft),
    ("scroll-right", Action::ScrollRight),
    ("follow", Action::Follow),
    ("next-heading", Action::NextHeading),
    ("previous-heading", Action::PrevHeading),
    ("contents", Action::Contents),
    ("next-link", Action::NextLink),
    ("previous-link", Action::PrevLink),
    ("back", Action::Back),
    ("set-mark", Action::SetMark),
    ("go-to-mark", Action::GoToMark),
    ("edit", Action::Edit),
    ("reload", Action::Reload),
    ("help", Action::Help),
    ("suspend", Action::Suspend),
    ("quit", Action::Quit),
    ("nothing", Action::None),
];

/// Key bindings from the config file, checked before the default ones.
#[derive(Debug, Default, Clone)]
pub struct Keymap {
    /// Key, modifiers, action, and the binding as written (for the help screen).
    bindings: Vec<(KeyCode, KeyModifiers, Action, String, String)>,
}

impl Keymap {
    /// No bindings, in a constant (tests use it in a static).
    #[cfg(test)]
    pub const fn new() -> Self {
        Keymap {
            bindings: Vec::new(),
        }
    }

    /// Bind `key` (like `x`, `Ctrl-x`, `Alt-x`, `Shift-Tab`, `PageDown`) to the
    /// action named `action` (one of `ACTIONS`).
    pub fn bind(&mut self, key: &str, action: &str) -> Result<(), String> {
        let (code, modifiers) = parse_key(key)?;
        let Some(&(name, action)) = ACTIONS.iter().find(|(name, _)| *name == action) else {
            return Err(format!(
                "unknown action '{action}' (one of: {})",
                ACTIONS
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        self.bindings
            .push((code, modifiers, action, key.to_string(), name.to_string()));
        Ok(())
    }

    /// The action for `key`: a binding from the config file (the last one for
    /// the key), or the default.
    pub fn map(&self, key: KeyEvent) -> Action {
        let (code, modifiers) = normalize(key.code, key.modifiers);
        self.bindings
            .iter()
            .rev()
            .find(|(c, m, ..)| *c == code && *m == modifiers)
            .map_or_else(|| map(key), |(_, _, action, ..)| *action)
    }

    /// Names of the keys that do one of `actions` now, for messages: those of
    /// `defaults` (key name and key) that still do, then keys bound to them in
    /// the config file.
    pub fn keys_for(&self, actions: &[Action], defaults: &[(&str, KeyCode)]) -> Vec<String> {
        let mut names: Vec<String> = defaults
            .iter()
            .filter(|(_, code)| {
                actions.contains(&self.map(KeyEvent::new(*code, KeyModifiers::NONE)))
            })
            .map(|(name, _)| name.to_string())
            .collect();
        for (code, modifiers, action, key, _) in &self.bindings {
            // Only bindings that are still in effect (not bound again later).
            let current = self.map(KeyEvent::new(*code, *modifiers));
            if actions.contains(action) && current == *action && !names.contains(key) {
                names.push(key.clone());
            }
        }
        names
    }

    /// The bindings as written: key and action name.
    pub fn bindings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.bindings
            .iter()
            .map(|(.., key, action)| (key.as_str(), action.as_str()))
    }
}

/// Only Ctrl and Alt matter. Shift is part of the character (`G`), and
/// terminals differ in whether they report it.
fn normalize(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    let modifiers = modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
    let code = match code {
        KeyCode::Char(c) if modifiers.contains(KeyModifiers::CONTROL) => {
            KeyCode::Char(c.to_ascii_lowercase())
        }
        code => code,
    };
    (code, modifiers)
}

/// A key as written in the config file: a character, or a name like `Enter`,
/// with `Ctrl-` and `Alt-` in front as needed. Names ignore case.
pub fn parse_key(text: &str) -> Result<(KeyCode, KeyModifiers), String> {
    let mut rest = text;
    let mut modifiers = KeyModifiers::NONE;
    loop {
        let lower = rest.to_ascii_lowercase();
        if rest.chars().count() > 1 && (lower.starts_with("ctrl-") || lower.starts_with("alt-")) {
            let (prefix, after) = rest.split_once('-').expect("has a dash");
            modifiers |= if prefix.eq_ignore_ascii_case("ctrl") {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::ALT
            };
            rest = after;
        } else {
            break;
        }
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        _ => match rest.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "shift-tab" | "backtab" => KeyCode::BackTab,
            "backspace" => KeyCode::Backspace,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "insert" => KeyCode::Insert,
            "delete" | "del" => KeyCode::Delete,
            name => match name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=12) => KeyCode::F(n),
                _ => return Err(format!("unknown key '{text}'")),
            },
        },
    };
    Ok(normalize(code, modifiers))
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
        KeyCode::Char('r') => Action::ToggleRaw,
        KeyCode::Char('m') => Action::SetMark,
        KeyCode::Char('\'') => Action::GoToMark,
        KeyCode::Char('h' | 'H') | KeyCode::F(1) => Action::Help,
        KeyCode::Char('F') => Action::Follow,
        KeyCode::Char(c @ '0'..='9') => Action::Digit(c as u8 - b'0'),
        KeyCode::Char('%' | 'p') => Action::Percent,
        KeyCode::Char('w') => Action::ToggleWrap,
        KeyCode::Left => Action::ScrollLeft,
        KeyCode::Right => Action::ScrollRight,
        KeyCode::Char('v') => Action::Edit,
        KeyCode::Char('&') => Action::Filter,
        KeyCode::Char(']') => Action::NextHeading,
        KeyCode::Char('[') => Action::PrevHeading,
        KeyCode::Char('t') => Action::Contents,
        KeyCode::Char('R') => Action::Reload,
        KeyCode::Tab => Action::NextLink,
        KeyCode::BackTab => Action::PrevLink,
        KeyCode::Backspace => Action::Back,
        _ => Action::None,
    }
}

/// Drops terminal replies that arrive as keys. The background color query gives
/// up after a short wait; a terminal that answers later sends `ESC ] 11 ; rgb:...
/// ESC \` into the pager, which reads as Alt-], text, and Alt-\ (or Ctrl-G for a
/// BEL ending). Without this, `r`, `g`, `b`, `:`, and `/` in it would act as keys.
#[derive(Debug, Default)]
pub struct LateReplies {
    /// Inside a reply: how many characters of it have been dropped.
    inside: Option<usize>,
}

impl LateReplies {
    /// Whether `key` is part of a late reply (and must be ignored).
    pub fn is_reply(&mut self, key: &KeyEvent) -> bool {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = (key.modifiers - KeyModifiers::SHIFT).is_empty();
        match (self.inside, key.code) {
            (None, KeyCode::Char(']')) if alt => {
                self.inside = Some(0);
                true
            }
            (None, _) => false,
            (Some(_), KeyCode::Char('\\')) if alt => {
                self.inside = None;
                true
            }
            (Some(_), KeyCode::Char('g')) if ctrl => {
                self.inside = None;
                true
            }
            // Replies are short printable text; anything else is a real key.
            (Some(n), KeyCode::Char(c)) if plain && c.is_ascii_graphic() && n < 256 => {
                self.inside = Some(n + 1);
                true
            }
            (Some(_), _) => {
                self.inside = None;
                false
            }
        }
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
            '\t' => key(KeyCode::Tab),
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
                    ('Z', _) => KeyCode::BackTab,
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
        assert_eq!(map(key(KeyCode::Char('r'))), Action::ToggleRaw);
        assert_eq!(map(key(KeyCode::Char('m'))), Action::SetMark);
        assert_eq!(map(key(KeyCode::Char('\''))), Action::GoToMark);
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
        assert_eq!(map(key(KeyCode::Tab)), Action::NextLink);
        assert_eq!(map(key(KeyCode::BackTab)), Action::PrevLink);
        assert_eq!(map(key(KeyCode::Backspace)), Action::Back);
    }

    #[test]
    fn config_bindings_come_before_the_defaults() {
        let mut keys = Keymap::new();
        keys.bind("x", "quit").unwrap();
        keys.bind("Ctrl-J", "page-down").unwrap();
        keys.bind("q", "nothing").unwrap();
        keys.bind("Shift-Tab", "back").unwrap();
        keys.bind("F5", "reload").unwrap();
        keys.bind("alt-n", "next-heading").unwrap();
        keys.bind("space", "half-page-down").unwrap();
        assert_eq!(keys.map(key(KeyCode::Char('x'))), Action::Quit);
        assert_eq!(keys.map(ctrl('j')), Action::PageDown);
        assert_eq!(keys.map(key(KeyCode::Char('q'))), Action::None);
        let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);
        assert_eq!(keys.map(shift(KeyCode::BackTab)), Action::Back);
        assert_eq!(keys.map(key(KeyCode::F(5))), Action::Reload);
        let alt = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT);
        assert_eq!(keys.map(alt), Action::NextHeading);
        assert_eq!(keys.map(key(KeyCode::Char(' '))), Action::HalfDown);
        // Other keys keep their defaults; a later binding wins.
        assert_eq!(keys.map(key(KeyCode::Char('g'))), Action::Top);
        keys.bind("x", "help").unwrap();
        assert_eq!(keys.map(key(KeyCode::Char('x'))), Action::Help);
        // Uppercase letters match with or without Shift reported.
        keys.bind("G", "quit").unwrap();
        assert_eq!(keys.map(shift(KeyCode::Char('G'))), Action::Quit);
        assert_eq!(keys.map(key(KeyCode::Char('G'))), Action::Quit);
    }

    #[test]
    fn bad_bindings_are_errors() {
        let mut keys = Keymap::new();
        assert_eq!(
            keys.bind("Hyper-x", "quit").unwrap_err(),
            "unknown key 'Hyper-x'"
        );
        assert!(
            keys.bind("x", "explode")
                .unwrap_err()
                .starts_with("unknown action 'explode' (one of: line-down,")
        );
        assert_eq!(keys.bind("F13", "quit").unwrap_err(), "unknown key 'F13'");
        // A dash alone, or "Ctrl-" followed by a dash, is still a key.
        assert!(keys.bind("-", "page-up").is_ok());
        assert!(keys.bind("Ctrl--", "page-up").is_ok());
    }

    #[test]
    fn late_terminal_replies_are_dropped() {
        let alt = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        let mut late = LateReplies::default();
        let mut reply = vec![alt(']')];
        reply.extend(
            "11;rgb:0b0b/1010/2020"
                .chars()
                .map(|c| key(KeyCode::Char(c))),
        );
        reply.push(alt('\\'));
        assert!(reply.iter().all(|k| late.is_reply(k)));
        // Keys after the reply count again, and so do keys without one.
        assert!(!late.is_reply(&key(KeyCode::Char('q'))));
        assert!(!late.is_reply(&key(KeyCode::Char('r'))));
        // A BEL ending, then a real key.
        assert!(late.is_reply(&alt(']')));
        assert!(late.is_reply(&key(KeyCode::Char('1'))));
        assert!(late.is_reply(&ctrl('g')));
        assert!(!late.is_reply(&ctrl('g')));
        // A key that cannot be in a reply ends it and is kept.
        assert!(late.is_reply(&alt(']')));
        assert!(!late.is_reply(&key(KeyCode::Enter)));
        assert!(!late.is_reply(&key(KeyCode::Char('q'))));
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
                KeyCode::BackTab,
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
