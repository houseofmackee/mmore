//! Width-aware line wrapping.
//!
//! For now this hard-wraps at the terminal width (like a terminal would on its
//! own). Word wrapping for Markdown prose will build on top of this.

use unicode_width::UnicodeWidthChar;

/// Lines wrapped to a fixed width, remembering which source line each row came from.
#[derive(Debug, Default)]
pub struct Wrapped {
    pub rows: Vec<String>,
    /// `source[i]` is the index of the input line that produced `rows[i]`.
    pub source: Vec<usize>,
}

impl Wrapped {
    /// First row produced by source line `line`, or the closest row before it.
    pub fn first_row_of(&self, line: usize) -> usize {
        self.source.partition_point(|&s| s < line)
    }
}

pub fn wrap_all(lines: &[String], width: usize) -> Wrapped {
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
pub fn row_count(lines: &[String], width: usize) -> usize {
    lines.iter().map(|l| wrap_line(l, width).len()).sum()
}

/// Split one line into rows no wider than `width` columns. Always returns at least
/// one row. A double-width character never straddles a row boundary.
pub fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for ch in line.chars() {
        let w = ch.width().unwrap_or(0);
        if current_width + w > width && current_width > 0 {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width += w;
    }
    rows.push(current);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_empty_lines_are_one_row() {
        assert_eq!(wrap_line("", 10), vec![""]);
        assert_eq!(wrap_line("hello", 10), vec!["hello"]);
        assert_eq!(wrap_line("0123456789", 10), vec!["0123456789"]);
    }

    #[test]
    fn long_lines_split_at_width() {
        assert_eq!(wrap_line("abcdefg", 3), vec!["abc", "def", "g"]);
    }

    #[test]
    fn wide_chars_do_not_straddle() {
        // Each CJK char is 2 columns wide.
        assert_eq!(wrap_line("a日本", 4), vec!["a日", "本"]);
    }

    #[test]
    fn zero_width_never_loops() {
        assert_eq!(wrap_line("日", 1), vec!["日"]);
        assert_eq!(wrap_line("ab", 0), vec!["a", "b"]);
    }

    #[test]
    fn wrap_all_tracks_sources() {
        let lines = vec!["abcd".to_string(), "x".to_string(), "123456".to_string()];
        let w = wrap_all(&lines, 2);
        assert_eq!(w.rows, vec!["ab", "cd", "x", "12", "34", "56"]);
        assert_eq!(w.source, vec![0, 0, 1, 2, 2, 2]);
        assert_eq!(w.first_row_of(2), 3);
        assert_eq!(row_count(&lines, 2), 6);
    }
}
