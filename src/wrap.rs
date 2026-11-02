//! Width-aware wrapping of styled lines: hard wrapping for display, and word
//! wrapping for Markdown prose.

use unicode_width::UnicodeWidthChar;

use crate::render::Rendered;
use crate::style::{Line, Span, Style};

/// Rows wrapped to a fixed width, remembering which source line each row came from.
#[derive(Debug, Default)]
pub struct Wrapped {
    pub rows: Vec<Line>,
    /// `source[i]` is the source line that produced `rows[i]`. Never decreases.
    pub source: Vec<usize>,
}

impl Wrapped {
    /// First row produced by source line `line`, or the closest row after it.
    pub fn first_row_of(&self, line: usize) -> usize {
        self.source.partition_point(|&s| s < line)
    }
}

pub fn wrap_all(rendered: &Rendered, width: usize) -> Wrapped {
    let mut wrapped = Wrapped::default();
    for (line, &source) in rendered.lines.iter().zip(&rendered.source) {
        for row in wrap_line(line, width) {
            wrapped.rows.push(row);
            wrapped.source.push(source);
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

/// Word-wrap styled text into lines of at most `width` columns. Runs of whitespace
/// collapse to one space, and a span containing "\n" forces a line break. Words
/// longer than the width are split.
pub fn wrap_words(spans: &[Span], width: usize) -> Vec<Line> {
    let mut w = WordWrap {
        width: width.max(1),
        ..WordWrap::default()
    };
    let mut buf = [0u8; 4];
    for span in spans {
        for ch in span.text.chars() {
            if ch == '\n' {
                w.end_word();
                w.break_line();
            } else if ch.is_whitespace() {
                w.end_word();
                w.space.get_or_insert(span.style);
            } else {
                w.word.push(ch.encode_utf8(&mut buf), span.style);
                w.word_width += ch.width().unwrap_or(0);
            }
        }
    }
    w.finish()
}

#[derive(Default)]
struct WordWrap {
    width: usize,
    lines: Vec<Line>,
    current: Line,
    current_width: usize,
    word: Line,
    word_width: usize,
    /// Style of the pending space before the next word, if any.
    space: Option<Style>,
}

impl WordWrap {
    fn end_word(&mut self) {
        if self.word.spans.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        let word_width = std::mem::take(&mut self.word_width);
        // No leading space at the start of a line.
        let space = self.space.take().filter(|_| self.current_width > 0);

        let space_width = usize::from(space.is_some());
        if self.current_width > 0 && self.current_width + space_width + word_width > self.width {
            self.break_line();
        } else if let Some(style) = space {
            self.current.push(" ", style);
            self.current_width += 1;
        }

        if self.current_width == 0 && word_width > self.width {
            let mut rows = wrap_line(&word, self.width);
            let last = rows.pop().unwrap_or_default();
            self.lines.extend(rows);
            self.current_width = last.width();
            self.current = last;
        } else {
            self.current.append(word);
            self.current_width += word_width;
        }
    }

    fn break_line(&mut self) {
        self.lines.push(std::mem::take(&mut self.current));
        self.current_width = 0;
        self.space = None;
    }

    fn finish(mut self) -> Vec<Line> {
        self.end_word();
        if !self.current.spans.is_empty() || self.lines.is_empty() {
            self.lines.push(self.current);
        }
        self.lines
    }
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

    fn words(s: &str, width: usize) -> Vec<String> {
        texts(&wrap_words(&[Span::new(s, Style::default())], width))
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
        let mut r = Rendered::default();
        r.push(Line::plain("abcd"), 0);
        r.push(Line::plain("x"), 3);
        r.push(Line::plain("123456"), 4);
        let w = wrap_all(&r, 2);
        assert_eq!(texts(&w.rows), vec!["ab", "cd", "x", "12", "34", "56"]);
        assert_eq!(w.source, vec![0, 0, 3, 4, 4, 4]);
        assert_eq!(w.first_row_of(3), 2);
        // A source line with no rows of its own anchors to the next one.
        assert_eq!(w.first_row_of(1), 2);
        assert_eq!(row_count(&r.lines, 2), 6);
    }

    #[test]
    fn word_wrap_breaks_between_words() {
        assert_eq!(
            words("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn word_wrap_collapses_whitespace() {
        assert_eq!(words("  a   b \t c  ", 20), vec!["a b c"]);
    }

    #[test]
    fn word_wrap_splits_long_words() {
        assert_eq!(words("ab abcdefgh c", 4), vec!["ab", "abcd", "efgh", "c"]);
    }

    #[test]
    fn word_wrap_hard_breaks() {
        assert_eq!(words("a\nb c", 10), vec!["a", "b c"]);
        assert_eq!(words("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn word_wrap_keeps_styles_across_spans() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let spans = [
            Span::new("plain ", Style::default()),
            Span::new("bold", bold),
            Span::new(" tail", Style::default()),
        ];
        let rows = wrap_words(&spans, 10);
        assert_eq!(texts(&rows), vec!["plain bold", "tail"]);
        assert_eq!(rows[0].spans[1], Span::new("bold", bold));
    }
}
