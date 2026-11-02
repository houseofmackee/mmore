//! Turning source text into displayable lines.

pub mod code;
pub mod escapes;
pub mod markdown;
pub mod plain;
pub mod symbols;
pub mod table;

use crate::style::Line;

/// Rendered output with the source line each output line came from. Source line
/// numbers never decrease, so they can anchor the scroll position across resizes.
#[derive(Debug, Default)]
pub struct Rendered {
    pub lines: Vec<Line>,
    pub source: Vec<usize>,
    /// Hyperlink targets, indexed by `Style::link`.
    pub links: Vec<String>,
    /// Markdown headings, in order.
    pub headings: Vec<Heading>,
}

/// A Markdown heading, for jumping between headings and the table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1 for `#`, up to 6.
    pub level: u8,
    pub text: String,
    /// Source line where the heading is.
    pub source: usize,
}

impl Rendered {
    pub fn push(&mut self, line: Line, source: usize) {
        self.lines.push(line);
        self.source.push(source);
    }
}
