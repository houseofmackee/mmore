//! Width-aware line wrapping of styled lines.
//!
//! For now this hard-wraps at the terminal width (like a terminal would on its
//! own). Word wrapping for Markdown prose will build on top of this.

use unicode_width::UnicodeWidthChar;

use crate::style::Line;

/// Lines wrapped to a fixed width, remembering which source line each row came from.
#[derive(Debug, Default)]
pub struct Wrapped {
    pub rows: Vec<Line>,
    /// `source[i]` is the index of the input line that produced `rows[i]`.
    pub source: Vec<usize>,
}

impl Wrapped {
    /// First row produced by source line `line`, or the closest row before it.
    pub fn first_row_of(&self, line: usize) -> usize {
        self.source.partition_point(|&s| s < line)
    }
}

pub fn wrap_all(lines: &[Line], width: usize) -> Wrapped {
    let mut wrapped = Wrapped::default();
    for (i, line) in lines.iter().enumerate() {
        for row in wrap_line(line, width) {
            wrapped.rows.push(row);
            wrapped.source.push(i);
        }
    }
    wrapped
}

/// Number of terminal rows `lines` occupy at `width`.
pub fn row_count(lines: &[Line], width: usize) -> usize {
    let width = width.max(1);
    lines
        .iter()
        .map(|l| {
            // Fast path: lines that fit need no splitting.
            if l.width() <= width {
                1
            } else {
                wrap_line(l, width).len()
            }
        })
        .sum()
}

/// Split one line into rows no wider than `width` columns, keeping span styles.
/// Always returns at least one row. A double-width character never straddles rows.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![line.clone()];
    }
    let mut rows = Vec::new();
    let mut current = Line::default();
    let mut current_width = 0;
    let mut buf = [0u8; 4];
    for span in &line.spans {
        for ch in span.text.chars() {
            let w = ch.width().unwrap_or(0);
            if current_width + w > width && current_width > 0 {
                rows.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(ch.encode_utf8(&mut buf), span.style);
            current_width += w;
        }
    }
    rows.push(current);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Span, Style};

    fn texts(rows: &[Line]) -> Vec<String> {
        rows.iter().map(Line::text).collect()
    }

    fn wrap(s: &str, width: usize) -> Vec<String> {
        texts(&wrap_line(&Line::plain(s), width))
    }

    #[test]
    fn short_and_empty_lines_are_one_row() {
        assert_eq!(wrap("", 10), vec![""]);
        assert_eq!(wrap("hello", 10), vec!["hello"]);
        assert_eq!(wrap("0123456789", 10), vec!["0123456789"]);
    }

    #[test]
    fn long_lines_split_at_width() {
        assert_eq!(wrap("abcdefg", 3), vec!["abc", "def", "g"]);
    }

    #[test]
    fn wide_chars_do_not_straddle() {
        // Each CJK char is 2 columns wide.
        assert_eq!(wrap("a日本", 4), vec!["a日", "本"]);
    }

    #[test]
    fn zero_width_never_loops() {
        assert_eq!(wrap("日", 1), vec!["日"]);
        assert_eq!(wrap("ab", 0), vec!["a", "b"]);
    }

    #[test]
    fn styles_survive_splitting() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let line = Line {
            spans: vec![Span::new("ab", Style::default()), Span::new("cde", bold)],
        };
        let rows = wrap_line(&line, 3);
        assert_eq!(texts(&rows), vec!["abc", "de"]);
        assert_eq!(rows[0].spans[1], Span::new("c", bold));
        assert_eq!(rows[1].spans, vec![Span::new("de", bold)]);
    }

    #[test]
    fn wrap_all_tracks_sources() {
        let lines: Vec<Line> = ["abcd", "x", "123456"]
            .into_iter()
            .map(Line::plain)
            .collect();
        let w = wrap_all(&lines, 2);
        assert_eq!(texts(&w.rows), vec!["ab", "cd", "x", "12", "34", "56"]);
        assert_eq!(w.source, vec![0, 0, 1, 2, 2, 2]);
        assert_eq!(w.first_row_of(2), 3);
        assert_eq!(row_count(&lines, 2), 6);
    }
}
