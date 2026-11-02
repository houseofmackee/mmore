//! Styled text: the common output format of every renderer.

use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
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
    /// Hyperlink target: an index into the document's link table.
    pub link: Option<u32>,
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

    /// The text without styling.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Append all spans of `other`.
    pub fn append(&mut self, other: Line) {
        for span in other.spans {
            self.push(&span.text, span.style);
        }
    }

    /// Remove trailing whitespace.
    pub fn trim_end(&mut self) {
        while let Some(last) = self.spans.last_mut() {
            let len = last.text.trim_end().len();
            if len == 0 {
                self.spans.pop();
            } else {
                last.text.truncate(len);
                break;
            }
        }
    }

    /// Display width in terminal columns.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }
}

/// Readable markup for tests: `{b,fg5}text{/}` for styled spans, plain text otherwise.
#[cfg(test)]
pub fn markup(line: &Line) -> String {
    let mut out = String::new();
    for span in &line.spans {
        let s = &span.style;
        let mut tags = Vec::new();
        for (on, tag) in [
            (s.bold, "b"),
            (s.dim, "dim"),
            (s.italic, "i"),
            (s.underline, "u"),
            (s.strike, "s"),
        ] {
            if on {
                tags.push(tag.to_string());
            }
        }
        let color = |c: Color| match c {
            Color::Ansi(n) => n.to_string(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        };
        if let Some(c) = s.fg {
            tags.push(format!("fg{}", color(c)));
        }
        if let Some(c) = s.bg {
            tags.push(format!("bg{}", color(c)));
        }
        if let Some(l) = s.link {
            tags.push(format!("link{l}"));
        }
        if tags.is_empty() {
            out.push_str(&span.text);
        } else {
            out.push_str(&format!("{{{}}}{}{{/}}", tags.join(","), span.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_trim() {
        let mut line = Line::plain("a ");
        line.append(Line::plain("b  "));
        line.trim_end();
        assert_eq!(line.text(), "a b");
        let mut blank = Line::plain("   ");
        blank.trim_end();
        assert!(blank.spans.is_empty());
    }

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
