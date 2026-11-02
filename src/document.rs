//! The loaded files, renderable at any width.
//!
//! Plain text and code are rendered once, when loading. Markdown depends on the
//! width, so it is laid out again whenever the terminal is resized. Every file also
//! keeps its plain source lines for the raw view.

use syntect::highlighting::Theme;
use unicode_width::UnicodeWidthStr;

use crate::render::Rendered;
use crate::render::markdown::{self, MdOptions};
use crate::style::{Line, Span, Style};

/// How a file is displayed when not in the raw view.
pub enum Section {
    /// Same as the raw source lines.
    Plain,
    /// Pre-rendered lines (highlighted code, or a message for binary files).
    Fixed(Vec<Line>),
    /// Markdown source, laid out at render time.
    Markdown(String),
}

pub struct File {
    pub name: String,
    pub content: Section,
    /// The source as plain lines (tabs expanded, control characters escaped).
    pub raw: Vec<Line>,
}

pub struct Document {
    pub files: Vec<File>,
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
    /// Show source line numbers in a gutter.
    pub line_numbers: bool,
}

impl Document {
    /// Render one file's content (no header). Source line numbers are 0-based
    /// lines of that file.
    pub fn render_file(&self, index: usize, width: usize, raw: bool) -> Rendered {
        let file = &self.files[index];
        match &file.content {
            Section::Markdown(text) if !raw => {
                let width = self.width.unwrap_or(width).max(1);
                let opts = MdOptions {
                    width: width.min(self.max_width).max(1),
                    highlight: self.highlight,
                    theme: self.theme,
                    ascii: self.ascii,
                    front_matter: self.front_matter,
                };
                markdown::render(text, &opts)
            }
            Section::Fixed(lines) if !raw => fixed(lines),
            _ => fixed(&file.raw),
        }
    }

    /// Render every file one after the other, for printing without the pager.
    /// With several files, each starts with a `more`-style header.
    pub fn render_all(&self, width: usize) -> Rendered {
        let mut out = Rendered::default();
        let mut base = 0;
        for index in 0..self.files.len() {
            if self.files.len() > 1 {
                let bar = ":".repeat(14);
                for line in [bar.clone(), self.files[index].name.clone(), bar] {
                    out.push(Line::plain(line), base);
                }
            }
            let gutter = self.gutter_width(index);
            let rendered = self.render_file(index, width.saturating_sub(gutter).max(1), false);
            // Link indexes are per file; shift them into the shared table.
            let link_base = out.links.len() as u32;
            out.links.extend(rendered.links);
            let mut previous = None;
            for (mut line, source) in rendered.lines.into_iter().zip(rendered.source) {
                for span in &mut line.spans {
                    if let Some(link) = &mut span.style.link {
                        *link += link_base;
                    }
                }
                if gutter > 0 {
                    let number = (previous != Some(source)).then_some(source + 1);
                    let mut numbered = Line::default();
                    numbered.spans.push(gutter_span(number, gutter));
                    numbered.append(line);
                    line = numbered;
                }
                previous = Some(source);
                out.push(line, base + source);
            }
            base += self.files[index].raw.len() + 1;
        }
        out
    }

    /// Width of the line number gutter for a file (0 when line numbers are off).
    pub fn gutter_width(&self, index: usize) -> usize {
        if !self.line_numbers {
            return 0;
        }
        let lines = self.files[index].raw.len().max(1);
        lines.to_string().width().max(3) + 1
    }
}

/// A gutter cell: the right-aligned line number (or blanks) and a space.
pub fn gutter_span(number: Option<usize>, width: usize) -> Span {
    let digits = width.saturating_sub(1);
    let text = match number {
        Some(n) => format!("{n:>digits$} "),
        None => " ".repeat(width),
    };
    Span::new(
        text,
        Style {
            dim: true,
            ..Style::default()
        },
    )
}

fn fixed(lines: &[Line]) -> Rendered {
    let mut out = Rendered::default();
    for (i, line) in lines.iter().enumerate() {
        out.push(line.clone(), i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets;
    use crate::render::plain;

    fn file(name: &str, content: Section, raw: &str) -> File {
        File {
            name: name.into(),
            content,
            raw: plain::render(raw),
        }
    }

    fn markdown(name: &str, text: &str) -> File {
        file(name, Section::Markdown(text.into()), text)
    }

    fn doc(files: Vec<File>) -> Document {
        Document {
            files,
            width: None,
            max_width: 100,
            highlight: false,
            theme: assets::theme(assets::DEFAULT_DARK_THEME).unwrap(),
            ascii: false,
            front_matter: true,
            line_numbers: false,
        }
    }

    fn texts(r: &Rendered) -> Vec<String> {
        r.lines.iter().map(Line::text).collect()
    }

    #[test]
    fn raw_view_shows_source() {
        let d = doc(vec![markdown("a.md", "# H\n\ntext")]);
        assert_eq!(texts(&d.render_file(0, 20, false))[0], "H");
        assert_eq!(texts(&d.render_file(0, 20, true)), vec!["# H", "", "text"]);
    }

    #[test]
    fn plain_files_use_their_raw_lines() {
        let d = doc(vec![file("a.txt", Section::Plain, "x\ny")]);
        assert_eq!(texts(&d.render_file(0, 20, false)), vec!["x", "y"]);
    }

    #[test]
    fn render_all_adds_headers_for_several_files() {
        let d = doc(vec![
            file("a.txt", Section::Plain, "x"),
            file("b.txt", Section::Plain, "y"),
        ]);
        let all = texts(&d.render_all(40));
        assert_eq!(
            all,
            vec![
                "::::::::::::::",
                "a.txt",
                "::::::::::::::",
                "x",
                "::::::::::::::",
                "b.txt",
                "::::::::::::::",
                "y"
            ]
        );
        let r = d.render_all(40);
        assert!(r.source.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn links_from_several_files_share_one_table() {
        let d = doc(vec![
            markdown("a.md", "[a](https://one)"),
            markdown("b.md", "[b](https://two)"),
        ]);
        let r = d.render_all(40);
        assert_eq!(r.links, vec!["https://one", "https://two"]);
        let b_line = r
            .lines
            .iter()
            .find(|l| l.text().starts_with("b ("))
            .unwrap();
        assert_eq!(b_line.spans[0].style.link, Some(1));
    }

    #[test]
    fn line_numbers_in_gutter() {
        let mut d = doc(vec![file("a.txt", Section::Plain, "x\ny")]);
        d.line_numbers = true;
        assert_eq!(texts(&d.render_all(40)), vec!["  1 x", "  2 y"]);
        // Markdown: numbers mark where each block starts in the source.
        let mut d = doc(vec![markdown("a.md", "# H\n\npara one\nstill para")]);
        d.line_numbers = true;
        assert_eq!(
            texts(&d.render_all(20)),
            vec![
                "  1 H",
                "    ════════════════",
                "    ",
                "  3 para one still",
                "    para"
            ]
        );
    }

    #[test]
    fn markdown_width_is_capped() {
        let mut d = doc(vec![markdown("a.md", "# H")]);
        d.max_width = 10;
        assert_eq!(d.render_file(0, 80, false).lines[1].width(), 10);
        d.width = Some(5);
        assert_eq!(d.render_file(0, 80, false).lines[1].width(), 5);
    }
}
