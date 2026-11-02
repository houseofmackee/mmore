//! Turning source text into displayable lines.

pub mod code;
pub mod markdown;
pub mod plain;

use crate::style::Line;

/// Rendered output with the source line each output line came from. Source line
/// numbers never decrease, so they can anchor the scroll position across resizes.
#[derive(Debug, Default)]
pub struct Rendered {
    pub lines: Vec<Line>,
    pub source: Vec<usize>,
}

impl Rendered {
    pub fn push(&mut self, line: Line, source: usize) {
        self.lines.push(line);
        self.source.push(source);
    }
}
