//! Regex search over rendered text.
//!
//! Matching runs on whole rendered lines (before wrapping), so a match that crosses
//! a row boundary is still found; the ranges are then split onto the rows.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::style::{Line, Style};
use crate::wrap::Wrapped;

#[derive(Clone)]
pub struct Search {
    regex: Regex,
    /// The pattern as typed (with a leading `!` for lines that do not match).
    pub pattern: String,
    /// Started with `?`: `n` goes up and `N` goes down.
    pub backward: bool,
    /// `!pattern`: lines that do *not* match.
    pub invert: bool,
    /// The pattern is plain text, not a regular expression (Ctrl-R).
    pub literal: bool,
}

/// Matches mapped onto wrapped rows.
#[derive(Debug, Default, Clone)]
pub struct RowMatches {
    /// Character ranges to highlight in each row.
    pub ranges: Vec<Vec<Range<usize>>>,
    /// Whether a match starts in each row (the rows `n`/`N` stop at).
    pub starts: Vec<bool>,
}

impl Search {
    /// A regular expression search (see `with`).
    #[cfg(test)]
    pub fn new(pattern: &str, backward: bool) -> Result<Search, String> {
        Search::with(pattern, backward, false)
    }

    /// A search for `input` as typed: a leading `!` looks for lines that do not
    /// match (as in less), and with `literal` the rest is plain text. Smart
    /// case: case-insensitive unless the pattern has an uppercase letter.
    pub fn with(input: &str, backward: bool, literal: bool) -> Result<Search, String> {
        let (invert, pattern) = match input.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, input),
        };
        if invert && pattern.is_empty() {
            return Err("Type a pattern after !".into());
        }
        let insensitive = !pattern.chars().any(char::is_uppercase);
        let source = if literal {
            regex::escape(pattern)
        } else {
            pattern.to_string()
        };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(insensitive)
            .build()
            .map_err(|e| {
                let detail = e.to_string();
                let last = detail.lines().last().unwrap_or("").trim().to_string();
                format!("Invalid pattern: {last}")
            })?;
        Ok(Search {
            regex,
            pattern: input.to_string(),
            backward,
            invert,
            literal,
        })
    }

    /// Whether the line `text` is one this search looks for (for the `&` filter
    /// and for searching other files): it matches, or with `!` it does not.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text) != self.invert
    }

    /// Non-empty matches in `text`, as character ranges.
    fn line_matches(&self, text: &str) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        // Convert byte offsets to char offsets incrementally (matches are in order).
        let (mut byte, mut chars) = (0, 0);
        let mut to_chars = |target: usize| {
            chars += text[byte..target].chars().count();
            byte = target;
            chars
        };
        for m in self.regex.find_iter(text) {
            if m.is_empty() {
                continue;
            }
            let start = to_chars(m.start());
            let end = to_chars(m.end());
            ranges.push(start..end);
        }
        ranges
    }

    pub fn row_matches(&self, wrapped: &Wrapped) -> RowMatches {
        // Rebuild each rendered line's text from its rows (wrapping keeps every
        // character), then match on whole lines.
        let mut per_line: Vec<Vec<Range<usize>>> = Vec::new();
        let mut text = String::new();
        for (row, line) in wrapped.rows.iter().enumerate() {
            text.push_str(&line.text());
            let last = wrapped.line.get(row + 1) != Some(&wrapped.line[row]);
            if last {
                per_line.push(if self.invert {
                    // A line that does not match is found as a whole, at its
                    // start; there is nothing in it to highlight.
                    if self.regex.is_match(&text) {
                        Vec::new()
                    } else {
                        std::iter::once(0..0).collect()
                    }
                } else {
                    self.line_matches(&text)
                });
                text.clear();
            }
        }
        // Rendered lines are numbered from 0 in row order, so `per_line` lines up.
        let mut result = RowMatches::default();
        for (row, line) in wrapped.rows.iter().enumerate() {
            let matches = &per_line[wrapped.line[row]];
            let start = wrapped.offset[row];
            let end = start
                + line
                    .spans
                    .iter()
                    .map(|s| s.text.chars().count())
                    .sum::<usize>();
            let ranges = matches
                .iter()
                .filter(|m| m.start < end && m.end > start)
                .map(|m| m.start.max(start) - start..m.end.min(end) - start)
                .collect();
            result.ranges.push(ranges);
            // A match starts in this row; an empty row (a blank line) can only be
            // found as a whole, by a `!` search.
            result.starts.push(
                matches
                    .iter()
                    .any(|m| m.start >= start && (m.start < end || m.start == start)),
            );
        }
        result
    }
}

/// Find the next row with a match start after `from` (or the previous one before
/// it). With `inclusive`, `from` itself counts.
pub fn find(starts: &[bool], from: usize, forward: bool, inclusive: bool) -> Option<usize> {
    if forward {
        let first = if inclusive { from } else { from + 1 };
        (first..starts.len()).find(|&r| starts[r])
    } else {
        let end = if inclusive { from + 1 } else { from };
        (0..end.min(starts.len())).rev().find(|&r| starts[r])
    }
}

/// Show `ranges` (character ranges) of a line in reverse video.
pub fn highlight(line: &Line, ranges: &[Range<usize>]) -> Line {
    if ranges.is_empty() {
        return line.clone();
    }
    let mut out = Line::default();
    let mut index = 0;
    let mut buf = [0u8; 4];
    for span in &line.spans {
        for ch in span.text.chars() {
            let hit = ranges.iter().any(|r| r.contains(&index));
            let style = Style {
                reverse: hit,
                ..span.style
            };
            out.push(ch.encode_utf8(&mut buf), style);
            index += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrap::wrap_all;

    use crate::render::Rendered;

    fn rendered(lines: &[&str]) -> Rendered {
        let mut r = Rendered::default();
        for (i, l) in lines.iter().enumerate() {
            r.push(Line::plain(*l), i);
        }
        r
    }

    #[test]
    fn smart_case() {
        let s = Search::new("hello", false).unwrap();
        assert_eq!(
            s.line_matches("Hello HELLO hello"),
            vec![0..5, 6..11, 12..17]
        );
        let s = Search::new("Hello", false).unwrap();
        assert_eq!(s.line_matches("Hello HELLO hello"), vec![0..5]);
    }

    #[test]
    fn regex_and_char_offsets() {
        let s = Search::new("b+", false).unwrap();
        // Multi-byte characters before the match must not shift char offsets.
        assert_eq!(s.line_matches("日本 abbb b"), vec![4..7, 8..9]);
        // Empty matches are ignored.
        assert!(
            Search::new("x*", false)
                .unwrap()
                .line_matches("abc")
                .is_empty()
        );
    }

    #[test]
    fn invalid_pattern_is_reported() {
        let err = Search::new("(unclosed", false).err().unwrap();
        assert!(err.starts_with("Invalid pattern"), "{err}");
    }

    #[test]
    fn matches_split_across_wrapped_rows() {
        let w = wrap_all(rendered(&["abcdef", "xx"]), 4);
        let m = Search::new("cde", false).unwrap().row_matches(&w);
        assert_eq!(m.ranges, vec![vec![2..4], vec![0..1], vec![]]);
        assert_eq!(m.starts, vec![true, false, false]);
    }

    #[test]
    fn find_next_and_previous() {
        let starts = [false, true, false, true, false];
        assert_eq!(find(&starts, 0, true, true), Some(1));
        assert_eq!(find(&starts, 1, true, true), Some(1));
        assert_eq!(find(&starts, 1, true, false), Some(3));
        assert_eq!(find(&starts, 3, true, false), None);
        assert_eq!(find(&starts, 3, false, false), Some(1));
        assert_eq!(find(&starts, 1, false, false), None);
        assert_eq!(find(&starts, 4, false, true), Some(3));
    }

    #[test]
    fn highlight_marks_ranges() {
        let line = Line::plain("abcd");
        let range = 1..3;
        let out = highlight(&line, std::slice::from_ref(&range));
        assert_eq!(out.spans.len(), 3);
        assert!(!out.spans[0].style.reverse);
        assert_eq!(out.spans[1].text, "bc");
        assert!(out.spans[1].style.reverse);
    }
}
