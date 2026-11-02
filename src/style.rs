//! Styled text: the common output format of every renderer.

use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    // Used by the Markdown renderer (milestone 4).
    #[allow(dead_code)]
    /// One of the 16 terminal palette colors (0 to 15). Follows the user's terminal theme.
    Ansi(u8),
    /// An exact color, downgraded to the terminal's color depth on output.
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl Style {
    pub fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Span {
            text: text.into(),
            style,
        }
    }
}

/// One logical line of output, made of styled spans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.is_empty() {
            return Line::default();
        }
        Line {
            spans: vec![Span::new(text, Style::default())],
        }
    }

    /// Append text, merging with the previous span when the style matches.
    pub fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.spans.push(Span::new(text, style)),
        }
    }

    /// The text without styling. Used by tests now, and by search (milestone 6).
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Display width in terminal columns.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_merges_equal_styles() {
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let mut line = Line::default();
        line.push("a", bold);
        line.push("b", bold);
        line.push("", Style::default());
        line.push("c", Style::default());
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.text(), "abc");
        assert_eq!(line.width(), 3);
    }

    #[test]
    fn empty_plain_line_has_no_spans() {
        assert!(Line::plain("").spans.is_empty());
    }
}
