//! The loaded inputs, renderable at any width.
//!
//! Plain text and code are rendered once. Markdown depends on the width, so it is
//! rendered again whenever the terminal is resized.

use syntect::highlighting::Theme;

use crate::render::Rendered;
use crate::render::markdown::{self, MdOptions};
use crate::style::Line;

pub enum Section {
    /// Lines that do not depend on the width.
    Fixed(Vec<Line>),
    /// Markdown source, laid out at render time.
    Markdown(String),
}

pub struct Document {
    pub sections: Vec<Section>,
    /// Fixed render width (`--width`), instead of the terminal width.
    pub width: Option<usize>,
    /// Cap on the width of rendered Markdown (`--max-width`).
    pub max_width: usize,
    /// Whether to syntax-highlight code blocks inside Markdown.
    pub highlight: bool,
    pub theme: &'static Theme,
}

impl Document {
    /// Render every section. Source line numbers are made global across sections
    /// (and never decrease) so they can anchor the scroll position.
    pub fn render(&self, terminal_width: usize) -> Rendered {
        let width = self.width.unwrap_or(terminal_width).max(1);
        let mut out = Rendered::default();
        let mut base = 0;
        for section in &self.sections {
            match section {
                Section::Fixed(lines) => {
                    for (i, line) in lines.iter().enumerate() {
                        out.push(line.clone(), base + i);
                    }
                    base += lines.len();
                }
                Section::Markdown(text) => {
                    let opts = MdOptions {
                        width: width.min(self.max_width).max(1),
                        highlight: self.highlight,
                        theme: self.theme,
                    };
                    let rendered = markdown::render(text, &opts);
                    let mut last = base;
                    for (line, source) in rendered.lines.into_iter().zip(rendered.source) {
                        last = last.max(base + source);
                        out.push(line, last);
                    }
                    base = (last + 1).max(base + text.lines().count());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets;

    fn doc(sections: Vec<Section>) -> Document {
        Document {
            sections,
            width: None,
            max_width: 100,
            highlight: false,
            theme: assets::theme(assets::DEFAULT_DARK_THEME).unwrap(),
        }
    }

    #[test]
    fn sources_are_global_and_monotonic() {
        let d = doc(vec![
            Section::Fixed(vec![Line::plain("a"), Line::plain("b")]),
            Section::Markdown("# H\n\ntext\n".into()),
            Section::Fixed(vec![Line::plain("z")]),
        ]);
        let r = d.render(40);
        let texts: Vec<String> = r.lines.iter().map(Line::text).collect();
        assert_eq!(texts[0], "a");
        assert_eq!(texts[2], "H");
        assert_eq!(r.source[2], 2);
        assert_eq!(*texts.last().unwrap(), "z");
        assert_eq!(*r.source.last().unwrap(), 5);
        assert!(r.source.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn markdown_width_is_capped() {
        let mut d = doc(vec![Section::Markdown("# H".into())]);
        d.max_width = 10;
        assert_eq!(d.render(80).lines[1].width(), 10);
        d.width = Some(5);
        assert_eq!(d.render(80).lines[1].width(), 5);
    }
}
