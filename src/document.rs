//! The loaded files, renderable at any width.
//!
//! Plain text and code are rendered once, when loading. Markdown depends on the
//! width, so it is laid out again whenever the terminal is resized. Every file also
//! keeps its plain source lines for the raw view.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use syntect::highlighting::Theme;
use syntect::parsing::SyntaxReference;
use unicode_width::UnicodeWidthStr;

use crate::render::Rendered;
use crate::render::code;
use crate::render::markdown::{self, MdOptions};
use crate::stream::Stream;
use crate::style::{Line, Span, Style};
use crate::wrap;

/// How a file is displayed when not in the raw view.
pub enum Section {
    /// Same as the raw source lines.
    Plain,
    /// Markdown source, laid out at render time.
    Markdown(String),
    /// Source code, highlighted lazily (see `LazyHighlight`).
    Code(Arc<LazyHighlight>),
}

/// Syntax highlighting that runs in the background, so large files open at once.
/// Highlighting must go from the top (the parser state carries over), so lines
/// become available in order; until then they are shown plain.
pub struct LazyHighlight {
    syntax: &'static SyntaxReference,
    theme: &'static Theme,
    text: String,
    lines: RwLock<Vec<Line>>,
    started: AtomicBool,
    done: AtomicBool,
}

/// Highlighted lines are published in batches of this many, or after this long,
/// whichever comes first, so the first screen gets its colors quickly.
const BATCH: usize = 256;
const BATCH_TIME: Duration = Duration::from_millis(20);

impl LazyHighlight {
    pub fn new(text: String, syntax: &'static SyntaxReference, theme: &'static Theme) -> Self {
        LazyHighlight {
            syntax,
            theme,
            text,
            lines: RwLock::new(Vec::new()),
            started: AtomicBool::new(false),
            done: AtomicBool::new(false),
        }
    }

    /// Highlight everything, publishing lines as it goes. Runs only once.
    pub fn run(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut batch = Vec::with_capacity(BATCH);
        let mut last = Instant::now();
        code::highlight_lines(&self.text, self.syntax, self.theme, |line| {
            batch.push(line);
            if batch.len() >= BATCH || last.elapsed() >= BATCH_TIME {
                self.publish(&mut batch);
                last = Instant::now();
            }
            true
        });
        self.publish(&mut batch);
        self.done.store(true, Ordering::Release);
    }

    fn publish(&self, batch: &mut Vec<Line>) {
        let mut lines = self.lines.write().unwrap_or_else(PoisonError::into_inner);
        lines.append(batch);
    }

    /// Number of lines highlighted so far.
    pub fn progress(&self) -> usize {
        self.lines
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// The highlighted version of line `index`, if it is ready.
    pub fn line(&self, index: usize) -> Option<Line> {
        let lines = self.lines.read().unwrap_or_else(PoisonError::into_inner);
        lines.get(index).cloned()
    }

    /// All lines, highlighting synchronously if the background work is not done.
    pub fn all(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        self.for_each_line(|line| {
            lines.push(line);
            true
        });
        lines
    }

    /// Hand every line to `f` in order, highlighting as it goes rather than
    /// first (so printing a big file starts at once), until `f` returns false.
    /// Lines after a point the highlighter gives up on are plain.
    pub fn for_each_line(&self, mut f: impl FnMut(Line) -> bool) {
        let mut sent = 0;
        let mut more = true;
        if self.is_done() {
            let lines = self.lines.read().unwrap_or_else(PoisonError::into_inner);
            for line in lines.iter() {
                sent += 1;
                if !f(line.clone()) {
                    return;
                }
            }
        } else {
            code::highlight_lines(&self.text, self.syntax, self.theme, |line| {
                sent += 1;
                more = f(line);
                more
            });
        }
        if !more {
            return;
        }
        for line in self.text.lines().skip(sent) {
            if !f(crate::render::plain::sanitize(&Line::plain(line))) {
                return;
            }
        }
    }
}

pub struct File {
    pub name: String,
    /// Where it came from on disk, if it can be opened in an editor.
    pub path: Option<std::path::PathBuf>,
    pub content: Section,
    /// The source as plain lines (tabs expanded, control characters escaped).
    /// Empty for streamed input, whose lines are in `stream`.
    pub raw: Vec<Line>,
    /// Input that is still being read (a pipe), shown as plain text.
    pub stream: Option<Arc<Stream>>,
}

impl File {
    /// The file's plain lines so far.
    fn plain_lines(&self) -> Vec<Line> {
        match &self.stream {
            Some(stream) => stream.lines_from(0),
            None => self.raw.clone(),
        }
    }

    /// Number of source lines so far.
    pub fn line_count(&self) -> usize {
        self.stream.as_ref().map_or(self.raw.len(), |s| s.len())
    }
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
        if let Some(stream) = &file.stream {
            return fixed(&stream.lines_from(0));
        }
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
            // Code starts plain; the pager shows highlighted lines as they arrive.
            Section::Markdown(_) | Section::Plain | Section::Code(_) => fixed(&file.raw),
        }
    }

    /// The input stream of a file that is still (or was) read from a pipe.
    pub fn stream(&self, index: usize) -> Option<&Stream> {
        self.files[index].stream.as_deref()
    }

    /// The lazy highlighter of a code file.
    pub fn code(&self, index: usize) -> Option<&LazyHighlight> {
        match &self.files[index].content {
            Section::Code(lazy) => Some(lazy),
            _ => None,
        }
    }

    /// Start highlighting every code file in the background.
    pub fn start_highlighting(&self) {
        for index in 0..self.files.len() {
            self.start_highlighting_file(index);
        }
    }

    /// Start highlighting one code file in the background (once).
    pub fn start_highlighting_file(&self, index: usize) {
        if let Section::Code(lazy) = &self.files[index].content {
            let lazy = Arc::clone(lazy);
            thread::spawn(move || lazy.run());
        }
    }

    /// Whether `render_all` at `width` would take fewer than `max_rows` rows. Needs
    /// no highlighting and stops early, so large files are rejected quickly.
    pub fn fits(&self, width: usize, max_rows: usize) -> bool {
        let mut lines = Vec::new();
        for (i, file) in self.files.iter().enumerate() {
            if self.files.len() > 1 {
                lines.extend(header(&file.name));
            }
            let gutter = self.gutter_width(i);
            let inner = width.saturating_sub(gutter).max(1);
            // Every line takes at least one row: skip the work for big files.
            if lines.len() + file.line_count() >= max_rows {
                return false;
            }
            match &file.content {
                Section::Markdown(_) => {
                    for line in self.render_file(i, inner, false).lines {
                        let mut row = Line::plain(" ".repeat(gutter));
                        row.append(line);
                        lines.push(row);
                    }
                }
                _ => {
                    for line in file.plain_lines() {
                        let mut row = Line::plain(" ".repeat(gutter));
                        row.append(line);
                        lines.push(row);
                    }
                }
            }
            if !wrap::fits(&lines, width, max_rows) {
                return false;
            }
        }
        wrap::fits(&lines, width, max_rows)
    }

    /// One file, ready to print: content with the line number gutter, and code
    /// highlighted now. Link indexes refer to the returned link table. Streams
    /// give the lines read so far.
    pub fn render_one(&self, index: usize, width: usize) -> Rendered {
        let gutter = self.gutter_width(index);
        let file = &self.files[index];
        let rendered = match (&file.content, &file.stream) {
            // Printing needs every line highlighted now.
            (Section::Code(lazy), None) => fixed(&lazy.all()),
            _ => self.render_file(index, width.saturating_sub(gutter).max(1), false),
        };
        if gutter == 0 {
            return rendered;
        }
        let mut out = Rendered {
            links: rendered.links,
            ..Rendered::default()
        };
        let mut previous = None;
        for (line, source) in rendered.lines.into_iter().zip(rendered.source) {
            let number = (previous != Some(source)).then_some(source + 1);
            previous = Some(source);
            out.push(numbered(line, number, gutter), source);
        }
        out
    }

    /// Every file one after the other with `more`-style headers, as printed.
    #[cfg(test)]
    pub fn render_all(&self, width: usize) -> Rendered {
        let mut out = Rendered::default();
        let mut base = 0;
        for index in 0..self.files.len() {
            if self.files.len() > 1 {
                for line in header(&self.files[index].name) {
                    out.push(line, base);
                }
            }
            let rendered = self.render_one(index, width);
            let link_base = out.links.len() as u32;
            out.links.extend(rendered.links);
            for (mut line, source) in rendered.lines.into_iter().zip(rendered.source) {
                for span in &mut line.spans {
                    if let Some(link) = &mut span.style.link {
                        *link += link_base;
                    }
                }
                out.push(line, base + source);
            }
            base += self.files[index].line_count() + 1;
        }
        out
    }

    /// Width of the line number gutter for a file (0 when line numbers are off).
    pub fn gutter_width(&self, index: usize) -> usize {
        if !self.line_numbers {
            return 0;
        }
        let file = &self.files[index];
        // Streams always leave room for growth: their length is unknown until they
        // end, and the layout should not depend on how fast input arrives.
        let lines = match &file.stream {
            Some(stream) => stream.len().max(99_999),
            None => file.line_count().max(1),
        };
        lines.to_string().width().max(3) + 1
    }
}

/// The separator printed before each file when there are several, as `more` does.
pub fn header(name: &str) -> [Line; 3] {
    let bar = ":".repeat(14);
    [Line::plain(&bar), Line::plain(name), Line::plain(bar)]
}

/// `line` with a gutter in front: its line number, or blanks for continuations.
pub fn numbered(line: Line, number: Option<usize>, gutter: usize) -> Line {
    let mut out = Line::default();
    out.spans.push(gutter_span(number, gutter));
    out.append(line);
    out
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
            path: None,
            content,
            raw: plain::render(raw),
            stream: None,
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

    fn code_file(text: &str) -> File {
        let syntax = assets::syntaxes().find_syntax_by_extension("rs").unwrap();
        let theme = assets::theme(assets::DEFAULT_DARK_THEME).unwrap();
        file(
            "a.rs",
            Section::Code(Arc::new(LazyHighlight::new(text.into(), syntax, theme))),
            text,
        )
    }

    #[test]
    fn code_is_plain_until_highlighted() {
        let d = doc(vec![code_file("fn main() {}\nlet x = 1;\n")]);
        let lazy = d.code(0).unwrap();
        assert_eq!(lazy.progress(), 0);
        assert!(lazy.line(0).is_none());
        let r = d.render_file(0, 40, false);
        assert!(r.lines[0].spans.iter().all(|s| s.style.fg.is_none()));
        lazy.run();
        assert!(lazy.is_done());
        assert_eq!(lazy.progress(), 2);
        let line = lazy.line(0).unwrap();
        assert_eq!(line.text(), "fn main() {}");
        assert!(line.spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn for_each_line_streams_and_can_stop() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let d = doc(vec![code_file(text)]);
        let lazy = d.code(0).unwrap();
        let mut seen = Vec::new();
        lazy.for_each_line(|line| {
            seen.push(line.text());
            seen.len() < 2
        });
        assert_eq!(seen, vec!["fn a() {}", "fn b() {}"]);
        // Same result whether or not the background highlighting has finished.
        let before = lazy.all();
        lazy.run();
        assert_eq!(lazy.all(), before);
        assert_eq!(before.len(), 3);
        assert!(before[2].spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn for_each_line_continues_plain_after_a_very_long_line() {
        let long = "x".repeat(code::MAX_HIGHLIGHT_LINE + 1);
        let text = format!("fn a() {{}}\n{long}\nfn b() {{}}\n");
        let d = doc(vec![code_file(&text)]);
        let lines = d.code(0).unwrap().all();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.style.fg.is_some()));
        assert!(lines[2].spans.iter().all(|s| s.style.fg.is_none()));
        assert_eq!(lines[2].text(), "fn b() {}");
    }

    #[test]
    fn printing_highlights_code_synchronously() {
        let d = doc(vec![code_file("fn main() {}\n")]);
        let r = d.render_all(40);
        assert!(r.lines[0].spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn fits_agrees_with_render_all() {
        let mut d = doc(vec![
            code_file("fn main() {}\n"),
            markdown("b.md", "# H\n\ntext"),
            file("c.txt", Section::Plain, "a long line that wraps around"),
        ]);
        for numbers in [false, true] {
            d.line_numbers = numbers;
            for width in [10, 12, 40] {
                let rows = wrap::row_count(&d.render_all(width).lines, width);
                assert!(d.fits(width, rows + 1), "width {width}, numbers {numbers}");
                assert!(!d.fits(width, rows), "width {width}, numbers {numbers}");
            }
        }
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
