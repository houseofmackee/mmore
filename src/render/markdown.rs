//! Markdown rendering: walks the pulldown-cmark event stream and lays out styled,
//! word-wrapped lines.
//!
//! Block structure is tracked with a stack of containers (block quotes and list
//! items). Each output line starts with the prefixes of all open containers: a bar
//! for quotes, and the item marker (first line) or matching indentation (later
//! lines) for list items.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::highlighting::Theme;
use unicode_width::UnicodeWidthStr;

use super::{Rendered, code, plain};
use crate::assets;
use crate::detect::{self, Kind};
use crate::style::{Color, Line, Span, Style};
use crate::wrap;

pub struct MdOptions<'a> {
    /// Width available for content, in columns.
    pub width: usize,
    /// Whether to syntax-highlight code blocks.
    pub highlight: bool,
    pub theme: &'a Theme,
}

const BULLETS: [&str; 3] = ["•", "◦", "▪"];

pub fn render(text: &str, opts: &MdOptions) -> Rendered {
    let events: Vec<(Event, Range<usize>)> = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH)
        .into_offset_iter()
        .collect();
    let mut r = Renderer::new(text, opts);
    for i in 0..events.len() {
        r.handle(&events, i);
    }
    r.flush_inline();
    r.out
}

fn fg(color: u8) -> Style {
    Style {
        fg: Some(Color::Ansi(color)),
        ..Style::default()
    }
}

fn dim() -> Style {
    Style {
        dim: true,
        ..Style::default()
    }
}

fn heading_style(level: HeadingLevel) -> Style {
    let color = match level {
        HeadingLevel::H1 => Some(Color::Ansi(5)),
        HeadingLevel::H2 => Some(Color::Ansi(4)),
        HeadingLevel::H3 => Some(Color::Ansi(6)),
        _ => None,
    };
    Style {
        fg: color,
        bold: true,
        ..Style::default()
    }
}

struct ListState {
    /// Next item number for ordered lists.
    next: Option<u64>,
    /// Width of the widest item number, for right alignment.
    number_width: usize,
}

enum Container {
    Quote,
    Item {
        marker: Span,
        width: usize,
        /// The marker has not been printed yet.
        first: bool,
    },
}

struct CodeBlock {
    lang: Option<String>,
    text: String,
}

struct Renderer<'a> {
    opts: &'a MdOptions<'a>,
    line_starts: Vec<usize>,
    out: Rendered,
    containers: Vec<Container>,
    lists: Vec<ListState>,
    /// Inline content of the current paragraph, heading, or tight list item.
    inline: Vec<Span>,
    emphasis: usize,
    strong: usize,
    strike: usize,
    link: usize,
    heading: Option<HeadingLevel>,
    /// Open links: destination and the index in `inline` where the link text starts.
    links: Vec<(String, usize)>,
    code: Option<CodeBlock>,
    html: Option<String>,
    /// A blank line separates the next block from the previous one.
    need_blank: bool,
    /// Source line of the block being rendered.
    src: usize,
}

impl<'a> Renderer<'a> {
    fn new(text: &str, opts: &'a MdOptions<'a>) -> Self {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Renderer {
            opts,
            line_starts,
            out: Rendered::default(),
            containers: Vec::new(),
            lists: Vec::new(),
            inline: Vec::new(),
            emphasis: 0,
            strong: 0,
            strike: 0,
            link: 0,
            heading: None,
            links: Vec::new(),
            code: None,
            html: None,
            need_blank: false,
            src: 0,
        }
    }

    fn handle(&mut self, events: &[(Event, Range<usize>)], i: usize) {
        let (event, range) = &events[i];
        match event {
            Event::Start(tag) => self.start(tag, range, events, i),
            Event::End(tag) => self.end(*tag),
            Event::Text(t) => {
                if let Some(code) = &mut self.code {
                    code.text.push_str(t);
                } else {
                    self.push_text(t, self.style());
                }
            }
            Event::Code(t) => {
                let style = Style {
                    fg: Some(Color::Ansi(1)),
                    ..self.style()
                };
                self.push_text(t, style);
            }
            Event::Html(t) => {
                if let Some(html) = &mut self.html {
                    html.push_str(t);
                }
            }
            Event::InlineHtml(t) => {
                let tag = t.trim().to_ascii_lowercase();
                if tag.starts_with("<br") {
                    self.push_text("\n", Style::default());
                } else if !tag.starts_with("<!--") {
                    self.push_text(t, dim());
                }
            }
            Event::SoftBreak => self.push_text(" ", self.style()),
            Event::HardBreak => self.push_text("\n", Style::default()),
            Event::Rule => {
                self.begin_block(range);
                let width = self.available();
                self.emit(Line {
                    spans: vec![Span::new("─".repeat(width), dim())],
                });
                self.need_blank = true;
            }
            Event::TaskListMarker(done) => {
                self.push_text(if *done { "☑ " } else { "☐ " }, self.style());
            }
            Event::FootnoteReference(label) => {
                self.push_text(&format!("[^{label}]"), dim());
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => self.push_text(t, self.style()),
        }
    }

    fn start(
        &mut self,
        tag: &Tag,
        range: &Range<usize>,
        events: &[(Event, Range<usize>)],
        i: usize,
    ) {
        match tag {
            Tag::Paragraph => self.begin_block(range),
            Tag::Heading { level, .. } => {
                self.begin_block(range);
                self.heading = Some(*level);
                let depth = *level as usize;
                if depth >= 3 {
                    self.push_text(&format!("{} ", "#".repeat(depth)), dim());
                }
            }
            Tag::BlockQuote(_) => {
                self.begin_block(range);
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.begin_block(range);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => code_lang(info),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some(CodeBlock {
                    lang,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => {
                self.begin_block(range);
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.begin_block(range);
                let number_width = start.map_or(0, |s| {
                    let last = s + count_items(events, i).saturating_sub(1) as u64;
                    last.to_string().len()
                });
                self.lists.push(ListState {
                    next: *start,
                    number_width,
                });
            }
            Tag::Item => {
                self.begin_block(range);
                let depth = self.lists.len().saturating_sub(1);
                let text = match self.lists.last_mut() {
                    Some(ListState {
                        next: Some(n),
                        number_width,
                    }) => {
                        let text = format!("  {n:>number_width$}. ");
                        *n += 1;
                        text
                    }
                    _ => format!("  {} ", BULLETS[depth % BULLETS.len()]),
                };
                self.containers.push(Container::Item {
                    width: text.width(),
                    marker: Span::new(text, fg(6)),
                    first: true,
                });
            }
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.links.push((dest_url.to_string(), self.inline.len()));
                self.link += 1;
            }
            Tag::Image { .. } => self.push_text("[image: ", dim()),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_inline();
                self.need_blank = true;
            }
            TagEnd::Heading(level) => {
                self.flush_inline();
                let width = self.available();
                let rule = match level {
                    HeadingLevel::H1 => Some("═"),
                    HeadingLevel::H2 => Some("─"),
                    _ => None,
                };
                if let Some(rule) = rule {
                    let style = Style {
                        bold: false,
                        ..heading_style(level)
                    };
                    self.emit(Line {
                        spans: vec![Span::new(rule.repeat(width), style)],
                    });
                }
                self.heading = None;
                self.need_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.containers.pop();
                self.need_blank = true;
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.emit_code_block(code);
                }
                self.need_blank = true;
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    self.emit_html_block(&html);
                }
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                // Nested lists continue their parent item without a gap.
                if self.lists.is_empty() {
                    self.need_blank = true;
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                // An empty item still shows its marker.
                if let Some(Container::Item { first: true, .. }) = self.containers.last() {
                    self.emit(Line::default());
                }
                self.containers.pop();
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.link = self.link.saturating_sub(1);
                if let Some((url, start)) = self.links.pop() {
                    let text: String = self.inline[start.min(self.inline.len())..]
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect();
                    if show_url(&url, &text) {
                        self.push_text(&format!(" ({url})"), dim());
                    }
                }
            }
            TagEnd::Image => self.push_text("]", dim()),
            _ => {}
        }
    }

    /// Current inline style from the open emphasis, link, and heading tags.
    fn style(&self) -> Style {
        let mut style = self.heading.map(heading_style).unwrap_or_default();
        style.italic |= self.emphasis > 0;
        style.bold |= self.strong > 0;
        style.strike |= self.strike > 0;
        if self.link > 0 {
            style.underline = true;
            style.fg = Some(Color::Ansi(4));
        }
        style
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if !text.is_empty() {
            self.inline.push(Span::new(text, style));
        }
    }

    /// Start a new block: finish pending inline text and add a separating blank line.
    fn begin_block(&mut self, range: &Range<usize>) {
        self.flush_inline();
        if self.need_blank && !self.out.lines.is_empty() {
            let mut blank = self.prefix(false);
            blank.trim_end();
            self.out.push(blank, self.src);
        }
        self.need_blank = false;
        self.src = self.line_of(range.start);
    }

    fn line_of(&self, offset: usize) -> usize {
        self.line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1)
    }

    /// Width left for content after the container prefixes.
    fn available(&self) -> usize {
        let prefix: usize = self
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote => 2,
                Container::Item { width, .. } => *width,
            })
            .sum();
        self.opts.width.saturating_sub(prefix).max(1)
    }

    /// Prefix for the next line. With `consume`, list markers not yet shown are
    /// printed (and marked as shown); otherwise they become indentation.
    fn prefix(&mut self, consume: bool) -> Line {
        let mut line = Line::default();
        for container in &mut self.containers {
            match container {
                Container::Quote => line.push("│ ", dim()),
                Container::Item {
                    marker,
                    width,
                    first,
                } => {
                    if *first && consume {
                        line.push(&marker.text, marker.style);
                        *first = false;
                    } else {
                        line.push(&" ".repeat(*width), Style::default());
                    }
                }
            }
        }
        line
    }

    fn emit(&mut self, content: Line) {
        let mut line = self.prefix(true);
        line.append(content);
        self.out.push(line, self.src);
    }

    /// Word-wrap and emit the pending inline content.
    fn flush_inline(&mut self) {
        if self.inline.iter().all(|s| s.text.trim().is_empty()) {
            self.inline.clear();
            return;
        }
        let spans: Vec<Span> = std::mem::take(&mut self.inline)
            .into_iter()
            .map(|s| {
                if s.text == "\n" {
                    s
                } else {
                    // Tabs and control characters must not reach the terminal raw.
                    let clean = plain::sanitize(&Line {
                        spans: vec![s.clone()],
                    });
                    Span::new(clean.text(), s.style)
                }
            })
            .collect();
        for row in wrap::wrap_words(&spans, self.available()) {
            self.emit(row);
        }
    }

    fn emit_code_block(&mut self, block: CodeBlock) {
        let start = self.src;
        let mut top = Line::default();
        top.push("┌─", dim());
        if let Some(lang) = &block.lang {
            top.push(" ", dim());
            top.push(
                lang,
                Style {
                    italic: true,
                    ..dim()
                },
            );
        }
        self.emit(top);

        let lines = self.highlight(&block);
        let width = self.available().saturating_sub(2).max(1);
        for (i, line) in lines.iter().enumerate() {
            self.src = start + 1 + i;
            for row in wrap::wrap_line(line, width) {
                let mut content = Line::default();
                content.push("│ ", dim());
                content.append(row);
                self.emit(content);
            }
        }
        self.src = start + lines.len() + 1;
        self.emit(Line {
            spans: vec![Span::new("└─", dim())],
        });
    }

    fn highlight(&self, block: &CodeBlock) -> Vec<Line> {
        if self.opts.highlight && code::should_highlight(&block.text) {
            let kind = block
                .lang
                .as_deref()
                .and_then(|l| detect::by_language(assets::syntaxes(), l));
            if let Some(Kind::Code(syntax)) = kind {
                return code::render(&block.text, syntax, self.opts.theme);
            }
        }
        plain::render(&block.text)
    }

    fn emit_html_block(&mut self, html: &str) {
        let trimmed = html.trim();
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            return;
        }
        let width = self.available();
        for line in plain::render(html.trim_end()) {
            let line = Line {
                spans: vec![Span::new(line.text(), dim())],
            };
            for row in wrap::wrap_line(&line, width) {
                self.emit(row);
            }
        }
        self.need_blank = true;
    }
}

/// First word of a fenced code block's info string: "rust,ignore" and "{.python}"
/// both name the language.
fn code_lang(info: &str) -> Option<String> {
    info.split(|c: char| c.is_whitespace() || c == ',')
        .map(|w| w.trim_matches(|c| c == '{' || c == '}' || c == '.'))
        .find(|w| !w.is_empty())
        .map(str::to_string)
}

/// Show a link's URL unless it would only repeat the text or point inside the page.
fn show_url(url: &str, text: &str) -> bool {
    !url.is_empty()
        && !url.starts_with('#')
        && url != text
        && url.strip_prefix("mailto:") != Some(text)
}

/// Number of items in the list starting at `events[start]`.
fn count_items(events: &[(Event, Range<usize>)], start: usize) -> usize {
    let mut depth = 0;
    let mut count = 0;
    for (event, _) in &events[start + 1..] {
        match event {
            Event::Start(Tag::List(_)) => depth += 1,
            Event::End(TagEnd::List(_)) if depth == 0 => break,
            Event::End(TagEnd::List(_)) => depth -= 1,
            Event::Start(Tag::Item) if depth == 0 => count += 1,
            _ => {}
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::markup;

    fn opts(width: usize) -> MdOptions<'static> {
        MdOptions {
            width,
            highlight: false,
            theme: assets::theme(assets::DEFAULT_DARK_THEME).unwrap(),
        }
    }

    fn text(md: &str, width: usize) -> String {
        let r = render(md, &opts(width));
        r.lines
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn marked(md: &str, width: usize) -> String {
        let r = render(md, &opts(width));
        r.lines.iter().map(markup).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn headings() {
        assert_eq!(text("# Title", 10), "Title\n══════════");
        assert_eq!(text("## Sub", 5), "Sub\n─────");
        assert_eq!(text("### Third", 20), "### Third");
        assert_eq!(marked("# T", 3), "{b,fg5}T{/}\n{fg5}═══{/}");
    }

    #[test]
    fn paragraphs_wrap_and_are_separated() {
        assert_eq!(
            text("one two three\nfour\n\nnext para", 9),
            "one two\nthree\nfour\n\nnext para"
        );
    }

    #[test]
    fn inline_styles() {
        assert_eq!(
            marked("*a* **b** ~~c~~ `d`", 40),
            "{i}a{/} {b}b{/} {s}c{/} {fg1}d{/}"
        );
    }

    #[test]
    fn links_show_url_unless_redundant() {
        assert_eq!(text("[site](https://x.io)", 40), "site (https://x.io)");
        assert_eq!(text("<https://x.io>", 40), "https://x.io");
        assert_eq!(text("[top](#top)", 40), "top");
        assert_eq!(text("<me@x.io>", 40), "me@x.io");
        assert_eq!(marked("[a](u)", 40), "{u,fg4}a{/}{dim} (u){/}");
    }

    #[test]
    fn tight_and_nested_lists() {
        let md = "- one\n- two\n  - inner\n- three\n\nafter";
        assert_eq!(
            text(md, 40),
            "  • one\n  • two\n      ◦ inner\n  • three\n\nafter"
        );
    }

    #[test]
    fn ordered_lists_align_numbers() {
        let md = (1..=10).map(|i| format!("{i}. x\n")).collect::<String>();
        let out = text(&md, 40);
        assert!(out.starts_with("   1. x\n   2. x"), "{out}");
        assert!(out.ends_with("  10. x"), "{out}");
        assert_eq!(text("3. a\n4. b", 40), "  3. a\n  4. b");
    }

    #[test]
    fn list_items_use_hanging_indent() {
        assert_eq!(text("- aaa bbb ccc", 10), "  • aaa\n    bbb\n    ccc");
    }

    #[test]
    fn loose_lists_have_blank_lines() {
        assert_eq!(text("- a\n\n- b", 40), "  • a\n\n  • b");
    }

    #[test]
    fn block_quotes_prefix_every_line() {
        assert_eq!(text("> a b c\n>\n> d", 5), "│ a b\n│ c\n│\n│ d");
        assert_eq!(text("> > deep", 20), "│ │ deep");
    }

    #[test]
    fn code_blocks_are_boxed_and_not_wrapped_as_prose() {
        let md = "```rust\nfn  main() {}\n```\n\nafter";
        assert_eq!(text(md, 40), "┌─ rust\n│ fn  main() {}\n└─\n\nafter");
        assert_eq!(text("    indented", 40), "┌─\n│ indented\n└─");
    }

    #[test]
    fn long_code_lines_wrap_inside_the_box() {
        assert_eq!(text("```\nabcdefgh\n```", 6), "┌─\n│ abcd\n│ efgh\n└─");
    }

    #[test]
    fn code_block_in_list_item() {
        let md = "- item\n\n  ```\n  code\n  ```";
        assert_eq!(text(md, 40), "  • item\n\n    ┌─\n    │ code\n    └─");
    }

    #[test]
    fn highlighting_colors_code_blocks() {
        let md = "```rust\nfn main() {}\n```";
        let r = render(
            md,
            &MdOptions {
                highlight: true,
                ..opts(40)
            },
        );
        assert!(
            r.lines[1]
                .spans
                .iter()
                .any(|s| matches!(s.style.fg, Some(Color::Rgb(..))))
        );
    }

    #[test]
    fn rules_and_breaks() {
        assert_eq!(text("a\n\n---\n\nb", 5), "a\n\n─────\n\nb");
        assert_eq!(text("a  \nb", 40), "a\nb");
        assert_eq!(text("a<br>b", 40), "a\nb");
    }

    #[test]
    fn html_comments_are_hidden() {
        assert_eq!(text("a\n\n<!-- hidden -->\n\nb", 40), "a\n\nb");
        assert_eq!(text("a <!-- x --> b", 40), "a b");
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(text("a\x1b[31mb", 40), "a^[[31mb");
    }

    #[test]
    fn source_lines_are_tracked() {
        let r = render("# A\n\npara\n\n```\nx\ny\n```", &opts(20));
        let texts: Vec<String> = r.lines.iter().map(Line::text).collect();
        let x = texts.iter().position(|t| t == "│ x").unwrap();
        assert_eq!(r.source[0], 0);
        assert_eq!(r.source[texts.iter().position(|t| t == "para").unwrap()], 2);
        assert_eq!(r.source[x], 5);
        assert_eq!(r.source[x + 1], 6);
        assert!(r.source.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn code_lang_parsing() {
        assert_eq!(code_lang("rust,ignore").as_deref(), Some("rust"));
        assert_eq!(code_lang("{.python}").as_deref(), Some("python"));
        assert_eq!(code_lang("  sh  title=x").as_deref(), Some("sh"));
        assert_eq!(code_lang(""), None);
    }

    #[test]
    fn snapshot_full_document() {
        let md = include_str!("../../tests/fixtures/markdown/basic.md");
        insta::assert_snapshot!("basic_60", marked(md, 60));
        insta::assert_snapshot!("basic_32", text(md, 32));
    }
}
