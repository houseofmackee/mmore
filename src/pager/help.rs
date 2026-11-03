//! The help screen.

use crossterm::event::KeyCode;

use super::keys::{Action, Keymap};
use crate::style::{Line, Style};

/// Keys and what they do (also in the manual page).
pub const KEYS: &[(&str, &str)] = &[
    (
        "Space  f  PgDn  Ctrl-F",
        "Next page (at the end: next file)",
    ),
    ("b  PgUp  Ctrl-B", "Previous page"),
    ("Enter  j  Down  Ctrl-N", "Next line"),
    ("k  Up  Ctrl-P", "Previous line"),
    ("d  Ctrl-D", "Half a page down"),
    ("u  Ctrl-U", "Half a page up"),
    ("g  <  Home", "Go to the top"),
    ("G  >  End", "Go to the bottom"),
    ("Ng  NG", "Go to line N (type the number first, as in 120g)"),
    ("N%  Np", "Go to N percent of the file"),
    ("w", "Turn wrapping of long lines off or on"),
    ("Left  Right", "Scroll sideways when lines are not wrapped"),
    ("Mouse wheel", "Scroll 3 lines (needs --mouse)"),
    ("Click a link", "Follow it (needs --mouse)"),
    (
        "F",
        "Follow new input at the end, also of a file (any key stops)",
    ),
    (
        "/pattern",
        "Search forward (regex; case-insensitive if all lowercase)",
    ),
    ("?pattern", "Search backward"),
    ("/!pattern  &!pattern", "Lines that do not match"),
    (
        "Ctrl-K  Ctrl-R",
        "While typing a search: only highlight / plain text, not regex",
    ),
    ("n  N", "Next / previous match (on into the other files)"),
    ("]  [", "Next / previous heading (Markdown)"),
    (
        "t",
        "Table of contents (Markdown): j/k to choose, Enter to go",
    ),
    (
        "&pattern",
        "Show only matching lines (& and Enter shows all again)",
    ),
    ("Esc", "Clear search highlighting"),
    (":n  :p", "Next / previous file"),
    (":f  =  Ctrl-G", "Show file name and line number"),
    ("r", "Toggle rendered view and raw source"),
    ("mX  'X", "Mark this place as letter X / go back to mark X"),
    ("''", "Go back to where you were before the last jump"),
    (
        "Tab  Shift-Tab",
        "Select the next / previous link (Markdown)",
    ),
    (
        "Enter",
        "Follow the selected link: a heading, a file, or a web page",
    ),
    ("Backspace", "Go back to where the last followed link was"),
    ("v", "Edit the file ($VISUAL or $EDITOR), then reload it"),
    ("s", "Save the lines shown to a file"),
    ("|", "Send the lines shown to a command (| wc -l, | pbcopy)"),
    (
        "R",
        "Reload the file (--watch does it when the file changes)",
    ),
    ("h", "Show or close this help"),
    ("Ctrl-Z", "Suspend (fg to resume)"),
    ("q  :q  Ctrl-C", "Quit"),
];

/// The keys that close the help screen (or the table of contents), by name.
pub fn close_keys(keymap: &Keymap) -> Vec<String> {
    keymap.keys_for(
        &[Action::Quit, Action::Help, Action::ClearSearch],
        &[
            ("q", KeyCode::Char('q')),
            ("h", KeyCode::Char('h')),
            ("Esc", KeyCode::Esc),
        ],
    )
}

pub fn lines(keymap: &Keymap) -> Vec<Line> {
    let bold = Style {
        bold: true,
        ..Style::default()
    };
    let width = KEYS.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut lines = vec![
        Line {
            spans: vec![crate::style::Span::new("mmore keys", bold)],
        },
        Line::default(),
    ];
    for (keys, what) in KEYS {
        let mut line = Line::default();
        line.push(&format!("  {keys:<width$}"), bold);
        line.push(&format!("  {what}"), Style::default());
        lines.push(line);
    }
    let custom: Vec<(&str, &str)> = keymap.bindings().collect();
    if !custom.is_empty() {
        lines.push(Line::default());
        lines.push(Line {
            spans: vec![crate::style::Span::new(
                "Your keys (from the config file)",
                bold,
            )],
        });
        lines.push(Line::default());
        let width = custom.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (key, action) in custom {
            let mut line = Line::default();
            line.push(&format!("  {key:<width$}"), bold);
            line.push(&format!("  {action}"), Style::default());
            lines.push(line);
        }
    }
    lines.push(Line::default());
    let close = close_keys(keymap);
    if !close.is_empty() {
        let keys: Vec<&str> = close.iter().take(2).map(String::as_str).collect();
        lines.push(Line::plain(format!(
            "Press {} to return.",
            keys.join(" or ")
        )));
    }
    lines
}
