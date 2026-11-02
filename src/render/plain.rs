//! Plain text rendering, and the tab/control-character cleanup shared by all renderers.

use unicode_width::UnicodeWidthChar;

use crate::style::Line;

pub const TAB_WIDTH: usize = 8;

/// Render text into lines, expanding tabs and making control characters visible.
pub fn render(text: &str) -> Vec<Line> {
    text.lines().map(|l| sanitize(&Line::plain(l))).collect()
}

/// Expand tabs to the next tab stop and show control characters in caret notation
/// (like `cat -v`) so they cannot corrupt the terminal. Styles are kept.
pub fn sanitize(line: &Line) -> Line {
    let needs_work = line
        .spans
        .iter()
        .any(|s| s.text.chars().any(|c| c.is_control()));
    if !needs_work {
        return line.clone();
    }

    let mut out = Line::default();
    let mut col = 0;
    let mut buf = String::new();
    for span in &line.spans {
        buf.clear();
        for ch in span.text.chars() {
            match ch {
                '\t' => {
                    let spaces = TAB_WIDTH - (col % TAB_WIDTH);
                    buf.extend(std::iter::repeat_n(' ', spaces));
                    col += spaces;
                }
                // C0 controls and DEL.
                c if (c as u32) < 0x20 || c == '\x7f' => {
                    buf.push('^');
                    buf.push(char::from_u32((c as u32) ^ 0x40).unwrap_or('?'));
                    col += 2;
                }
                // C1 controls (U+0080 to U+009F) can also act as escapes.
                c if c.is_control() => {
                    buf.push_str(&format!("<U+{:04X}>", c as u32));
                    col += 8;
                }
                c => {
                    buf.push(c);
                    col += c.width().unwrap_or(0);
                }
            }
        }
        out.push(&buf, span.style);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Span, Style};

    fn one(s: &str) -> String {
        sanitize(&Line::plain(s)).text()
    }

    #[test]
    fn splits_lines_and_strips_crlf() {
        let lines: Vec<String> = render("a\r\nb\nc").iter().map(Line::text).collect();
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn expands_tabs_to_next_stop() {
        assert_eq!(one("\tx"), "        x");
        assert_eq!(one("ab\tx"), "ab      x");
        assert_eq!(one("12345678\tx"), "12345678        x");
        // Wide chars count as two columns.
        assert_eq!(one("日\tx"), "日      x");
    }

    #[test]
    fn control_chars_are_visible() {
        assert_eq!(one("a\x1b[31mb"), "a^[[31mb");
        assert_eq!(one("\x7f"), "^?");
        assert_eq!(one("\x01"), "^A");
        assert_eq!(one("\u{9b}"), "<U+009B>");
    }

    #[test]
    fn tab_stops_continue_across_spans() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let line = Line {
            spans: vec![Span::new("abc", Style::default()), Span::new("\tx", bold)],
        };
        let out = sanitize(&line);
        assert_eq!(out.text(), "abc     x");
        assert_eq!(out.spans[1], Span::new("     x", bold));
    }
}
