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
    /// Draw Markdown with ASCII characters only.
    pub ascii: bool,
    /// Show front matter in Markdown files.
    pub front_matter: bool,
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
                        ascii: self.ascii,
                        front_matter: self.front_matter,
                    };
                    let rendered = markdown::render(text, &opts);
                    // Link indexes are per section; shift them into the shared table.
                    let link_base = out.links.len() as u32;
                    out.links.extend(rendered.links);
                    let mut last = base;
                    for (mut line, source) in rendered.lines.into_iter().zip(rendered.source) {
                        for span in &mut line.spans {
                            if let Some(link) = &mut span.style.link {
                                *link += link_base;
                            }
                        }
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
            ascii: false,
            front_matter: true,
        }
    }

    #[test]
    fn links_from_several_files_share_one_table() {
        let d = doc(vec![
            Section::Markdown("[a](https://one)".into()),
            Section::Markdown("[b](https://two)".into()),
        ]);
        let r = d.render(40);
        assert_eq!(r.links, vec!["https://one", "https://two"]);
        let b_line = r.lines.iter().find(|l| l.text().starts_with('b')).unwrap();
        assert_eq!(b_line.spans[0].style.link, Some(1));
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
