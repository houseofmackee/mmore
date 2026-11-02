//! Colors and attributes in the input: terminal escape sequences (as written by
//! `git log --color`, `ls --color`, compilers) and backspace overstriking (as
//! written by `man`).
//!
//! Sequences are parsed into styles, never passed through: what reaches the
//! terminal is only what `ansi::render` writes for those styles. Color and
//! attribute codes (SGR) become styles; other control sequences (cursor moves,
//! erase line) and OSC sequences (titles, hyperlinks) are dropped, keeping any
//! text between them. Anything unrecognized is left for `plain::sanitize`, which
//! shows it in caret notation.

use crate::style::{Color, Line, Style};

/// Parses lines one after another; styles carry over from line to line, as they
/// do on a terminal.
#[derive(Debug, Default)]
pub struct Parser {
    style: Style,
}

impl Parser {
    pub fn new() -> Self {
        Parser::default()
    }

    /// One line of input (without its line ending) as a styled line. Tabs and
    /// other control characters are left in place for `plain::sanitize`.
    pub fn line(&mut self, text: &str) -> Line {
        // Fast path: nothing to interpret.
        if !text.contains(['\x1b', '\u{9b}', '\x08']) {
            let mut line = Line::default();
            line.push(text, self.style);
            return line;
        }
        let chars: Vec<char> = text.chars().collect();
        let mut cells: Vec<(char, Style)> = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\x1b' | '\u{9b}' => {
                    if let Some(next) = self.escape(&chars, i) {
                        i = next;
                        continue;
                    }
                    cells.push((chars[i], self.style));
                }
                '\x08' => {
                    if overstrike(&mut cells, chars.get(i + 1).copied()) {
                        i += 2;
                        continue;
                    }
                    cells.push(('\x08', self.style));
                }
                c => cells.push((c, self.style)),
            }
            i += 1;
        }
        let mut line = Line::default();
        let mut buf = [0u8; 4];
        for (c, style) in cells {
            line.push(c.encode_utf8(&mut buf), style);
        }
        line
    }

    /// Handle the escape sequence starting at `chars[start]`. Returns the index
    /// after it, or `None` if it is incomplete or unknown (then it is shown).
    fn escape(&mut self, chars: &[char], start: usize) -> Option<usize> {
        // An 8-bit CSI (U+009B) is the same as ESC [.
        let (kind, body) = if chars[start] == '\u{9b}' {
            ('[', start + 1)
        } else {
            (*chars.get(start + 1)?, start + 2)
        };
        match kind {
            '[' => {
                // Parameters 0x30-0x3F, intermediates 0x20-0x2F, final 0x40-0x7E.
                let mut end = body;
                while chars
                    .get(end)
                    .is_some_and(|c| ('\x20'..='\x3f').contains(c))
                {
                    end += 1;
                }
                let last = *chars.get(end)?;
                if !('\x40'..='\x7e').contains(&last) {
                    return None;
                }
                if last == 'm' {
                    let params: String = chars[body..end].iter().collect();
                    self.sgr(&params);
                }
                Some(end + 1)
            }
            ']' => {
                // OSC, ended by BEL or ST (ESC \). Hyperlinks (OSC 8) keep their text,
                // which is outside the sequences.
                let mut end = body;
                loop {
                    match chars.get(end)? {
                        '\x07' => return Some(end + 1),
                        '\x1b' if chars.get(end + 1) == Some(&'\\') => return Some(end + 2),
                        _ => end += 1,
                    }
                }
            }
            // Character set selection, as in `tput sgr0` output: ESC ( B.
            '(' | ')' | '*' | '+' => chars.get(body).map(|_| body + 1),
            // Keypad modes and other two-character sequences.
            '=' | '>' | '7' | '8' | 'M' | 'D' | 'E' | 'c' => Some(body),
            _ => None,
        }
    }

    /// Apply Select Graphic Rendition parameters, like "1;31" or "38:2::255:0:0".
    fn sgr(&mut self, params: &str) {
        let groups: Vec<Vec<u16>> = if params.is_empty() {
            vec![vec![0]]
        } else {
            params
                .split(';')
                .map(|g| g.split(':').map(|n| n.parse().unwrap_or(0)).collect())
                .collect()
        };
        let s = &mut self.style;
        let mut j = 0;
        while j < groups.len() {
            let group = &groups[j];
            let code = group[0];
            match code {
                0 => *s = Style::default(),
                1 => s.bold = true,
                2 => s.dim = true,
                3 => s.italic = true,
                4 => s.underline = group.get(1).is_none_or(|&style| style != 0),
                7 => s.reverse = true,
                9 => s.strike = true,
                21 => s.underline = true,
                22 => {
                    s.bold = false;
                    s.dim = false;
                }
                23 => s.italic = false,
                24 => s.underline = false,
                27 => s.reverse = false,
                29 => s.strike = false,
                30..=37 => s.fg = Some(Color::Ansi((code - 30) as u8)),
                39 => s.fg = None,
                40..=47 => s.bg = Some(Color::Ansi((code - 40) as u8)),
                49 => s.bg = None,
                90..=97 => s.fg = Some(Color::Ansi((code - 90 + 8) as u8)),
                100..=107 => s.bg = Some(Color::Ansi((code - 100 + 8) as u8)),
                38 | 48 => {
                    let (color, used) = extended_color(&groups[j..]);
                    if code == 38 {
                        s.fg = color.or(s.fg);
                    } else {
                        s.bg = color.or(s.bg);
                    }
                    j += used;
                    continue;
                }
                _ => {}
            }
            j += 1;
        }
    }
}

/// An extended color (38 or 48) at the start of `groups`: "38;5;N", "38;2;R;G;B",
/// or the colon forms "38:5:N" and "38:2:[id]:R:G:B". Returns the color and how
/// many `;`-separated groups it took.
fn extended_color(groups: &[Vec<u16>]) -> (Option<Color>, usize) {
    let first = &groups[0];
    if first.len() > 1 {
        // Colon form: everything is in this one group.
        let color = match first.get(1) {
            Some(5) => first.get(2).map(|&n| indexed(n)),
            Some(2) if first.len() >= 5 => {
                let n = first.len();
                Some(rgb(first[n - 3], first[n - 2], first[n - 1]))
            }
            _ => None,
        };
        return (color, 1);
    }
    let value = |k: usize| groups.get(k).and_then(|g| g.first()).copied();
    match value(1) {
        Some(5) => (value(2).map(indexed), 3),
        Some(2) => match (value(2), value(3), value(4)) {
            (Some(r), Some(g), Some(b)) => (Some(rgb(r, g, b)), 5),
            _ => (None, groups.len()),
        },
        _ => (None, 1),
    }
}

fn rgb(r: u16, g: u16, b: u16) -> Color {
    let c = |v: u16| v.min(255) as u8;
    Color::Rgb(c(r), c(g), c(b))
}

/// A 256-color palette index as a color: the 16 basic colors stay palette colors
/// (so they follow the terminal's scheme); the rest have fixed RGB values.
fn indexed(n: u16) -> Color {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match n.min(255) as u8 {
        n @ 0..=15 => Color::Ansi(n),
        n @ 16..=231 => {
            let i = (n - 16) as usize;
            Color::Rgb(LEVELS[i / 36], LEVELS[(i / 6) % 6], LEVELS[i % 6])
        }
        n => {
            let gray = 8 + 10 * (n - 232);
            Color::Rgb(gray, gray, gray)
        }
    }
}

/// Backspace overstriking, as `man` writes it: "X\bX" is a bold X and "_\bX" an
/// underlined X (either order). Any other pair shows the second character.
/// Returns false (show the backspace) if there is nothing to overstrike.
fn overstrike(cells: &mut [(char, Style)], next: Option<char>) -> bool {
    let (Some((prev, style)), Some(next)) = (cells.last_mut(), next) else {
        return false;
    };
    if next.is_control() {
        return false;
    }
    if *prev == next {
        style.bold = true;
    } else if *prev == '_' {
        *prev = next;
        style.underline = true;
    } else if next == '_' {
        style.underline = true;
    } else {
        *prev = next;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::markup;

    fn parse(lines: &[&str]) -> Vec<String> {
        let mut p = Parser::new();
        lines.iter().map(|l| markup(&p.line(l))).collect()
    }

    fn one(line: &str) -> String {
        parse(&[line]).remove(0)
    }

    #[test]
    fn basic_colors_and_attributes() {
        assert_eq!(one("\x1b[31mred\x1b[0m plain"), "{fg1}red{/} plain");
        assert_eq!(one("\x1b[1;4;92mx\x1b[m"), "{b,u,fg10}x{/}");
        assert_eq!(one("\x1b[44;3;9;7mx"), "{i,s,rev,bg4}x{/}");
        assert_eq!(one("\x1b[2mdim\x1b[22mnot"), "{dim}dim{/}not");
    }

    #[test]
    fn extended_colors() {
        assert_eq!(one("\x1b[38;5;1mx"), "{fg1}x{/}");
        assert_eq!(one("\x1b[38;5;196mx"), "{fg#ff0000}x{/}");
        assert_eq!(one("\x1b[48;5;244mx"), "{bg#808080}x{/}");
        assert_eq!(one("\x1b[38;2;10;20;30mx"), "{fg#0a141e}x{/}");
        assert_eq!(one("\x1b[38:2::10:20:30mx"), "{fg#0a141e}x{/}");
        assert_eq!(one("\x1b[38:2:10:20:30mx"), "{fg#0a141e}x{/}");
        assert_eq!(one("\x1b[38:5:2mx"), "{fg2}x{/}");
        // Codes after an extended color still apply.
        assert_eq!(one("\x1b[38;5;3;1mx"), "{b,fg3}x{/}");
        assert_eq!(one("\x1b[4:0mx"), "x");
    }

    #[test]
    fn styles_carry_across_lines() {
        assert_eq!(
            parse(&["\x1b[32mgreen", "still", "\x1b[0mreset"]),
            vec!["{fg2}green{/}", "{fg2}still{/}", "reset"]
        );
    }

    #[test]
    fn other_sequences_are_dropped() {
        // Erase line and cursor movement, as in progress output.
        assert_eq!(one("\x1b[2K\x1b[1Gdone"), "done");
        // OSC 8 hyperlinks keep their text; titles disappear.
        assert_eq!(
            one("\x1b]8;;https://x.io\x1b\\link\x1b]8;;\x1b\\ end"),
            "link end"
        );
        assert_eq!(one("\x1b]0;title\x07text"), "text");
        // `tput sgr0` writes a character set selection too.
        assert_eq!(one("\x1b[1mb\x1b(B\x1b[mx"), "{b}b{/}x");
        // An 8-bit CSI is understood like ESC [.
        assert_eq!(one("\u{9b}31mx"), "{fg1}x{/}");
    }

    #[test]
    fn incomplete_or_unknown_escapes_are_left_for_sanitize() {
        assert_eq!(one("a\x1b[31"), "a\x1b[31");
        assert_eq!(one("a\x1b]0;no end"), "a\x1b]0;no end");
        assert_eq!(one("a\x1bZb"), "a\x1bZb");
        assert_eq!(one("end\x1b"), "end\x1b");
    }

    #[test]
    fn man_page_overstriking() {
        assert_eq!(one("N\x08NA\x08AM\x08ME\x08E"), "{b}NAME{/}");
        assert_eq!(one("_\x08f_\x08i_\x08l_\x08e"), "{u}file{/}");
        assert_eq!(one("x\x08_"), "{u}x{/}");
        assert_eq!(one("a\x08b"), "b");
        // Nothing to overstrike: the backspace stays (sanitize shows ^H).
        assert_eq!(one("\x08a"), "\x08a");
        assert_eq!(one("a\x08"), "a\x08");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(one("just\ttext"), "just\ttext");
    }
}
