//! Width-aware wrapping of styled lines: wrapping long lines for display (at
//! spaces where possible), and word wrapping for Markdown prose.

use unicode_width::UnicodeWidthChar;

use crate::render::Rendered;
use crate::style::{Line, Span, Style};

/// Rows wrapped to a fixed width, remembering which source line each row came from.
#[derive(Debug, Default)]
pub struct Wrapped {
    pub rows: Vec<Line>,
    /// `source[i]` is the source line that produced `rows[i]`. Never decreases.
    pub source: Vec<usize>,
    /// Hyperlink targets, indexed by `Style::link`.
    pub links: Vec<String>,
    /// `line[i]` is the rendered line that `rows[i]` is part of.
    pub line: Vec<usize>,
    /// `offset[i]` is the character offset of `rows[i]` within its rendered line.
    pub offset: Vec<usize>,
}

impl Wrapped {
    /// First row produced by source line `line`, or the closest row after it.
    pub fn first_row_of(&self, line: usize) -> usize {
        self.source.partition_point(|&s| s < line)
    }
}

/// Wrap every line. Takes the rendered lines by value so lines that fit are
/// moved rather than copied (large files have hundreds of thousands).
pub fn wrap_all(rendered: Rendered, width: usize) -> Wrapped {
    let width = width.max(1);
    let count = rendered.lines.len();
    let mut wrapped = Wrapped {
        links: rendered.links,
        rows: Vec::with_capacity(count),
        source: Vec::with_capacity(count),
        line: Vec::with_capacity(count),
        offset: Vec::with_capacity(count),
    };
    for (index, (line, source)) in rendered.lines.into_iter().zip(rendered.source).enumerate() {
        let rows = if line.width() <= width {
            vec![line]
        } else {
            wrap_line(&line, width)
        };
        let mut offset = 0;
        for row in rows {
            let chars = row
                .spans
                .iter()
                .map(|s| s.text.chars().count())
                .sum::<usize>();
            wrapped.rows.push(row);
            wrapped.source.push(source);
            wrapped.line.push(index);
            wrapped.offset.push(offset);
            offset += chars;
        }
    }
    wrapped
}

/// Whether `lines` fit in fewer than `max_rows` rows at `width`. Stops counting as
/// soon as they do not.
pub fn fits(lines: &[Line], width: usize, max_rows: usize) -> bool {
    let width = width.max(1);
    let mut rows = 0;
    for line in lines {
        rows += if line.width() <= width {
            1
        } else {
            wrap_line(line, width).len()
        };
        if rows >= max_rows {
            return false;
        }
    }
    true
}

/// Number of terminal rows `lines` occupy at `width`.
#[cfg(test)]
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
/// Rows end after the last space that fits, so words stay whole; a word longer
/// than a row is split. Every character is kept (the spaces stay at the ends of
/// rows), so the rows put together are the line. Always returns at least one
/// row. A double-width character never straddles rows.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![line.clone()];
    }
    let cells: Vec<(char, Style, usize)> = line
        .spans
        .iter()
        .flat_map(|span| {
            span.text
                .chars()
                .map(move |c| (c, span.style, c.width().unwrap_or(0)))
        })
        .collect();
    let mut rows = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        // As many characters as fit (at least one).
        let mut end = start;
        let mut used = 0;
        while end < cells.len() && (used + cells[end].2 <= width || end == start) {
            used += cells[end].2;
            end += 1;
        }
        // Break after the last space, unless the rest fits or the next
        // character is a space anyway.
        if end < cells.len()
            && cells[end].0 != ' '
            && let Some(space) = (start..end).rev().find(|&i| cells[i].0 == ' ')
        {
            end = space + 1;
        }
        let mut row = Line::default();
        let mut buf = [0u8; 4];
        for &(c, style, _) in &cells[start..end] {
            row.push(c.encode_utf8(&mut buf), style);
        }
        rows.push(row);
        start = end;
    }
    rows
}

/// The part of `line` from display column `start` that fits in `width` columns,
/// for showing long lines without wrapping. A double-width character cut by the
/// left edge becomes a space, so columns stay aligned.
pub fn slice_columns(line: &Line, start: usize, width: usize) -> Line {
    let mut out = Line::default();
    let mut col = 0;
    let end = start + width;
    let mut buf = [0u8; 4];
    for span in &line.spans {
        for ch in span.text.chars() {
            let w = ch.width().unwrap_or(0);
            if col >= end {
                return out;
            }
            if col >= start && col + w <= end {
                out.push(ch.encode_utf8(&mut buf), span.style);
            } else if col < start && col + w > start {
                out.push(&" ".repeat(col + w - start), span.style);
            }
            col += w;
        }
    }
    out
}

/// Display column at which character `index` of `line` starts.
pub fn column_of(line: &Line, index: usize) -> usize {
    line.spans
        .iter()
        .flat_map(|s| s.text.chars())
        .take(index)
        .map(|c| c.width().unwrap_or(0))
        .sum()
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
    fn long_lines_break_between_words() {
        let rows = |text: &str, width| -> Vec<String> {
            wrap_line(&Line::plain(text), width)
                .iter()
                .map(Line::text)
                .collect()
        };
        assert_eq!(
            rows("one two three four", 9),
            ["one two ", "three ", "four"]
        );
        // A space right at the edge starts the next row; nothing is lost.
        assert_eq!(rows("abcd efgh", 4), ["abcd", " ", "efgh"]);
        // Words longer than a row are split.
        assert_eq!(
            rows("supercalifragilistic is long", 8),
            ["supercal", "ifragili", "stic is ", "long"]
        );
        // Wide characters never straddle rows, and styles are kept.
        assert_eq!(rows("日本語 テキスト", 7), ["日本語 ", "テキス", "ト"]);
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let line = Line {
            spans: vec![
                Span::new("plain ", Style::default()),
                Span::new("bold words", bold),
            ],
        };
        let wrapped = wrap_line(&line, 8);
        assert_eq!(wrapped[0].text(), "plain ");
        assert_eq!(wrapped[1].spans, vec![Span::new("bold ", bold)]);
    }

    #[test]
    fn wrap_all_tracks_sources() {
        let mut r = Rendered::default();
        r.push(Line::plain("abcd"), 0);
        r.push(Line::plain("x"), 3);
        r.push(Line::plain("123456"), 4);
        let counted = row_count(&r.lines, 2);
        let w = wrap_all(r, 2);
        assert_eq!(texts(&w.rows), vec!["ab", "cd", "x", "12", "34", "56"]);
        assert_eq!(w.source, vec![0, 0, 3, 4, 4, 4]);
        assert_eq!(w.line, vec![0, 0, 1, 2, 2, 2]);
        assert_eq!(w.offset, vec![0, 2, 0, 0, 2, 4]);
        assert_eq!(w.first_row_of(3), 2);
        // A source line with no rows of its own anchors to the next one.
        assert_eq!(w.first_row_of(1), 2);
        assert_eq!(counted, 6);
        assert!(!fits(&[Line::plain("abcd")], 2, 2));
        assert!(fits(&[Line::plain("abcd")], 2, 3));
    }

    #[test]
    fn slicing_columns_for_unwrapped_lines() {
        let line = Line::plain("abcdefgh");
        assert_eq!(slice_columns(&line, 2, 3).text(), "cde");
        assert_eq!(slice_columns(&line, 6, 10).text(), "gh");
        assert_eq!(slice_columns(&line, 20, 5).text(), "");
        // A wide character cut by the left edge becomes a space.
        let wide = Line::plain("a日本x");
        assert_eq!(slice_columns(&wide, 2, 4).text(), " 本x");
        assert_eq!(slice_columns(&wide, 0, 2).text(), "a");
        assert_eq!(column_of(&wide, 2), 3);
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
