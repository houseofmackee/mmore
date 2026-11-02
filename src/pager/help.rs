//! The help screen.

use crate::style::{Line, Style};

const KEYS: &[(&str, &str)] = &[
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
    ("F", "Follow new input at the end (any key stops)"),
    (
        "/pattern",
        "Search forward (regex; case-insensitive if all lowercase)",
    ),
    ("?pattern", "Search backward"),
    ("n  N", "Next / previous match"),
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
    ("m", "Toggle rendered view and raw source"),
    ("v", "Edit the file ($VISUAL or $EDITOR), then reload it"),
    (
        "R",
        "Reload the file (--watch does it when the file changes)",
    ),
    ("h", "Show or close this help"),
    ("Ctrl-Z", "Suspend (fg to resume)"),
    ("q  :q  Ctrl-C", "Quit"),
];

pub fn lines() -> Vec<Line> {
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
    lines.push(Line::default());
    lines.push(Line::plain("Press q or h to return."));
    lines
}
