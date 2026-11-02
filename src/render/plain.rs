//! Plain text rendering: tab expansion and visible control characters.

pub const TAB_WIDTH: usize = 8;

/// Render text into lines, expanding tabs and making control characters visible
/// so they cannot corrupt the terminal.
pub fn render(text: &str) -> Vec<String> {
    text.lines().map(render_line).collect()
}

fn render_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    // Column counted in chars; good enough until wrapping lands with unicode-width.
    let mut col = 0;
    for ch in line.chars() {
        match ch {
            '\t' => {
                let spaces = TAB_WIDTH - (col % TAB_WIDTH);
                out.extend(std::iter::repeat_n(' ', spaces));
                col += spaces;
            }
            // Caret notation for C0 controls and DEL, like `cat -v`.
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push('^');
                out.push(char::from_u32((c as u32) ^ 0x40).unwrap_or('?'));
                col += 2;
            }
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_and_strips_crlf() {
        assert_eq!(render("a\r\nb\nc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn expands_tabs_to_next_stop() {
        assert_eq!(render_line("\tx"), "        x");
        assert_eq!(render_line("ab\tx"), "ab      x");
        assert_eq!(render_line("12345678\tx"), "12345678        x");
    }

    #[test]
    fn control_chars_are_visible() {
        assert_eq!(render_line("a\x1b[31mb"), "a^[[31mb");
        assert_eq!(render_line("\x7f"), "^?");
        assert_eq!(render_line("\x01"), "^A");
    }
}
