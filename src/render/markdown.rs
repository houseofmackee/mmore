//! Markdown rendering: walks the pulldown-cmark event stream and lays out styled,
//! word-wrapped lines.
//!
//! Block structure is tracked with a stack of containers (block quotes, list
//! items, and footnote definitions). Each output line starts with the prefixes of
//! all open containers: a bar for quotes, and the item marker (first line) or
//! matching indentation (later lines) for list items and footnotes.

use std::ops::Range;

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, LinkType, MetadataBlockKind, Options,
    Parser, Tag, TagEnd,
};
use syntect::highlighting::Theme;
use unicode_width::UnicodeWidthStr;

use super::symbols::{self, Symbols};
use super::table::{self, Table};
use super::{Heading, Rendered, code, plain};
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
    /// Draw with ASCII characters only.
    pub ascii: bool,
    /// Show YAML/TOML front matter.
    pub front_matter: bool,
}

pub fn render(text: &str, opts: &MdOptions) -> Rendered {
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS;
    let events: Vec<(Event, Range<usize>)> =
        Parser::new_ext(text, options).into_offset_iter().collect();
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

/// Title and palette color of a GitHub alert (`> [!NOTE]`).
fn alert(kind: BlockQuoteKind) -> (&'static str, u8) {
    match kind {
        BlockQuoteKind::Note => ("Note", 4),
        BlockQuoteKind::Tip => ("Tip", 2),
        BlockQuoteKind::Important => ("Important", 5),
        BlockQuoteKind::Warning => ("Warning", 3),
        BlockQuoteKind::Caution => ("Caution", 1),
    }
}

struct ListState {
    /// Next item number for ordered lists.
    next: Option<u64>,
    /// Width of the widest item number, for right alignment.
    number_width: usize,
}

enum Container {
    Quote {
        bar: Style,
    },
    /// A list item or footnote definition.
    Item {
        marker: Span,
        width: usize,
        /// The marker has not been printed yet.
        first: bool,
        /// The marker is a bullet, which a task checkbox may replace.
        bullet: bool,
    },
}

struct CodeBlock {
    lang: Option<String>,
    /// Shown in the top border instead of the language.
    label: Option<String>,
    text: String,
}

struct TableState {
    table: Table,
    row: Vec<Vec<Span>>,
}

struct Renderer<'a> {
    opts: &'a MdOptions<'a>,
    sym: &'static Symbols,
    line_starts: Vec<usize>,
    out: Rendered,
    containers: Vec<Container>,
    lists: Vec<ListState>,
    /// Inline content of the current paragraph, heading, table cell, or tight
    /// list item.
    inline: Vec<Span>,
    emphasis: usize,
    strong: usize,
    strike: usize,
    image: usize,
    heading: Option<HeadingLevel>,
    /// Open links: destination, link table index (absolute URLs only), and the
    /// position in `inline` where the link text starts.
    links: Vec<(String, Option<u32>, usize)>,
    /// Open images: destination URL.
    images: Vec<String>,
    code: Option<CodeBlock>,
    html: Option<String>,
    table: Option<TableState>,
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
            sym: symbols::get(opts.ascii),
            line_starts,
            out: Rendered::default(),
            containers: Vec::new(),
            lists: Vec::new(),
            inline: Vec::new(),
            emphasis: 0,
            strong: 0,
            strike: 0,
            image: 0,
            heading: None,
            links: Vec::new(),
            images: Vec::new(),
            code: None,
            html: None,
            table: None,
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
                    spans: vec![Span::new(self.sym.rule.repeat(width), dim())],
                });
                self.need_blank = true;
            }
            Event::TaskListMarker(done) => self.task_marker(*done),
            Event::FootnoteReference(label) => {
                self.push_text(&format!("[{label}]"), fg(6));
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
            Tag::BlockQuote(kind) => {
                self.begin_block(range);
                match kind.map(alert) {
                    Some((title, color)) => {
                        let style = fg(color);
                        self.containers.push(Container::Quote { bar: style });
                        self.emit(Line {
                            spans: vec![Span::new(
                                title,
                                Style {
                                    bold: true,
                                    ..style
                                },
                            )],
                        });
                    }
                    None => self.containers.push(Container::Quote { bar: dim() }),
                }
            }
            Tag::CodeBlock(kind) => {
                self.begin_block(range);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => code_lang(info),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some(CodeBlock {
                    lang,
                    label: None,
                    text: String::new(),
                });
            }
            Tag::MetadataBlock(kind) => {
                self.begin_block(range);
                let lang = match kind {
                    MetadataBlockKind::YamlStyle => "yaml",
                    MetadataBlockKind::PlusesStyle => "toml",
                };
                self.code = Some(CodeBlock {
                    lang: Some(lang.to_string()),
                    label: Some(format!("front matter ({lang})")),
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
                let (text, bullet) = match self.lists.last_mut() {
                    Some(ListState {
                        next: Some(n),
                        number_width,
                    }) => {
                        let text = format!("  {n:>number_width$}. ");
                        *n += 1;
                        (text, false)
                    }
                    _ => {
                        let bullets = &self.sym.bullets;
                        (format!("  {} ", bullets[depth % bullets.len()]), true)
                    }
                };
                self.push_item(Span::new(text, fg(6)), bullet);
            }
            Tag::FootnoteDefinition(label) => {
                self.begin_block(range);
                self.push_item(Span::new(format!("[{label}] "), fg(6)), false);
            }
            Tag::Table(aligns) => {
                self.begin_block(range);
                self.table = Some(TableState {
                    table: Table {
                        aligns: aligns.clone(),
                        rows: Vec::new(),
                    },
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.row.clear();
                }
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                // Email autolinks (<a@b.c>) come without their scheme.
                let url = if *link_type == LinkType::Email {
                    format!("mailto:{dest_url}")
                } else {
                    dest_url.to_string()
                };
                let index = self.register_link(&url);
                self.links.push((url, index, self.inline.len()));
            }
            Tag::Image { dest_url, .. } => {
                self.push_text(&format!("{} ", self.sym.image), dim());
                self.images.push(dest_url.to_string());
                self.image += 1;
            }
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
                // The text without the dim "### " marker of deeper headings.
                let text: String = self.inline.iter().map(|s| s.text.as_str()).collect();
                let text = text.trim_start_matches('#').trim().to_string();
                self.out.headings.push(Heading {
                    level: level as u8,
                    text,
                    source: self.src,
                });
                self.flush_inline();
                let width = self.available();
                let rule = match level {
                    HeadingLevel::H1 => Some(self.sym.h1_rule),
                    HeadingLevel::H2 => Some(self.sym.h2_rule),
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
            TagEnd::MetadataBlock(_) => {
                if let Some(code) = self.code.take()
                    && self.opts.front_matter
                {
                    self.emit_code_block(code);
                    self.need_blank = true;
                }
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
            TagEnd::Item => self.end_item(),
            TagEnd::FootnoteDefinition => {
                self.end_item();
                self.need_blank = true;
            }
            TagEnd::TableCell => {
                let cell = sanitize_spans(std::mem::take(&mut self.inline));
                if let Some(t) = &mut self.table {
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    for line in table::layout(&t.table, self.available(), self.sym) {
                        self.emit(line);
                    }
                }
                self.need_blank = true;
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some((url, index, start)) = self.links.pop() {
                    let text: String = self.inline[start.min(self.inline.len())..]
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect();
                    if show_url(&url, &text) {
                        let style = Style {
                            link: index,
                            ..dim()
                        };
                        self.push_text(&format!(" ({url})"), style);
                    }
                }
            }
            TagEnd::Image => {
                self.image = self.image.saturating_sub(1);
                let url = self.images.pop().unwrap_or_default();
                // Inside a link (a badge), the link's URL is the one that matters.
                if self.links.is_empty() && !url.is_empty() {
                    let index = self.register_link(&url);
                    let style = Style {
                        link: index,
                        ..dim()
                    };
                    self.push_text(&format!(" ({url})"), style);
                }
            }
            _ => {}
        }
    }

    /// Add a link target. All links can be followed in the pager; only absolute
    /// URLs become terminal hyperlinks (see `ansi::render`).
    fn register_link(&mut self, url: &str) -> Option<u32> {
        if url.is_empty() {
            return None;
        }
        self.out.links.push(url.to_string());
        Some((self.out.links.len() - 1) as u32)
    }

    fn push_item(&mut self, marker: Span, bullet: bool) {
        self.containers.push(Container::Item {
            width: marker.text.width(),
            marker,
            first: true,
            bullet,
        });
    }

    fn end_item(&mut self) {
        self.flush_inline();
        // An empty item still shows its marker.
        if let Some(Container::Item { first: true, .. }) = self.containers.last() {
            self.emit(Line::default());
        }
        self.containers.pop();
    }

    /// A task list checkbox replaces the bullet of its item, or else goes inline.
    fn task_marker(&mut self, done: bool) {
        let (symbol, style) = if done {
            (self.sym.checked, fg(2))
        } else {
            (self.sym.unchecked, fg(6))
        };
        if let Some(Container::Item {
            marker,
            width,
            first: true,
            bullet: true,
        }) = self.containers.last_mut()
        {
            *marker = Span::new(format!("  {symbol} "), style);
            *width = marker.text.width();
        } else {
            self.push_text(&format!("{symbol} "), style);
        }
    }

    /// Current inline style from the open emphasis, link, image, and heading tags.
    fn style(&self) -> Style {
        let mut style = self.heading.map(heading_style).unwrap_or_default();
        style.italic |= self.emphasis > 0 || self.image > 0;
        style.bold |= self.strong > 0;
        style.strike |= self.strike > 0;
        if let Some((_, index, _)) = self.links.last() {
            style.underline = true;
            style.fg = Some(Color::Ansi(4));
            style.link = *index;
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
        let bar = self.sym.quote_bar.width() + 1;
        let prefix: usize = self
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote { .. } => bar,
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
                Container::Quote { bar } => {
                    line.push(self.sym.quote_bar, *bar);
                    line.push(" ", Style::default());
                }
                Container::Item {
                    marker,
                    width,
                    first,
                    ..
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
        let spans = sanitize_spans(std::mem::take(&mut self.inline));
        for row in wrap::wrap_words(&spans, self.available()) {
            self.emit(row);
        }
    }

    fn emit_code_block(&mut self, block: CodeBlock) {
        let start = self.src;
        let mut top = Line::default();
        top.push(self.sym.code_top, dim());
        if let Some(label) = block.label.as_ref().or(block.lang.as_ref()) {
            top.push(" ", dim());
            top.push(
                label,
                Style {
                    italic: true,
                    ..dim()
                },
            );
        }
        self.emit(top);

        let lines = self.highlight(&block);
        let side = format!("{} ", self.sym.code_side);
        let width = self.available().saturating_sub(side.width()).max(1);
        for (i, line) in lines.iter().enumerate() {
            self.src = start + 1 + i;
            for row in wrap::wrap_line(line, width) {
                let mut content = Line::default();
                content.push(&side, dim());
                content.append(row);
                self.emit(content);
            }
        }
        self.src = start + lines.len() + 1;
        self.emit(Line {
            spans: vec![Span::new(self.sym.code_bottom, dim())],
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

/// Tabs and control characters must not reach the terminal raw. Spans that are a
/// single "\n" are hard line breaks and stay as they are.
fn sanitize_spans(spans: Vec<Span>) -> Vec<Span> {
    spans
        .into_iter()
        .map(|s| {
            if s.text == "\n" {
                s
            } else {
                let clean = plain::sanitize(&Line {
                    spans: vec![s.clone()],
                });
                Span::new(clean.text(), s.style)
            }
        })
        .collect()
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
            ascii: false,
            front_matter: true,
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
        assert_eq!(marked("[a](u)", 40), "{u,fg4,link0}a{/}{dim,link0} (u){/}");
        assert_eq!(
            marked("[a](https://u)", 40),
            "{u,fg4,link0}a{/}{dim,link0} (https://u){/}"
        );
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
    fn headings_are_recorded() {
        let r = render("# One\n\ntext\n\n## Two *b*\n\n### Three\n", &opts(40));
        let h: Vec<(u8, &str, usize)> = r
            .headings
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.source))
            .collect();
        assert_eq!(h, vec![(1, "One", 0), (2, "Two b", 4), (3, "Three", 6)]);
    }

    #[test]
    fn snapshot_full_document() {
        let md = include_str!("../../tests/fixtures/markdown/basic.md");
        insta::assert_snapshot!("basic_60", marked(md, 60));
        insta::assert_snapshot!("basic_32", text(md, 32));
    }

    fn with(md: &str, width: usize, f: impl FnOnce(&mut MdOptions)) -> String {
        let mut o = opts(width);
        f(&mut o);
        let r = render(md, &o);
        r.lines
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn links_are_registered_for_hyperlinks() {
        let r = render(
            "[a](https://a.io) ![img](https://x.io/p.png) [rel](docs/x.md) [top](#top)",
            &opts(80),
        );
        assert_eq!(
            r.links,
            vec!["https://a.io", "https://x.io/p.png", "docs/x.md", "#top"]
        );
        let texts: Vec<String> = r.lines.iter().map(markup).collect();
        assert!(texts[0].contains("{u,fg4,link0}a{/}"), "{texts:?}");
        assert!(
            texts[0].contains("{dim,link1} (https://x.io/p.png){/}"),
            "{texts:?}"
        );
        // Relative links are registered too, so the pager can follow them.
        assert!(
            texts[0].contains("{u,fg4,link2}rel{/}{dim,link2} (docs/x.md){/}"),
            "{texts:?}"
        );
        assert!(texts[0].contains("{u,fg4,link3}top{/}"), "{texts:?}");
    }

    #[test]
    fn email_autolinks_become_mailto() {
        let r = render("<me@x.io>", &opts(40));
        assert_eq!(r.links, vec!["mailto:me@x.io"]);
        assert_eq!(markup(&r.lines[0]), "{u,fg4,link0}me@x.io{/}");
    }

    #[test]
    fn absolute_url_detection() {
        use crate::render::is_absolute_url;
        assert!(is_absolute_url("https://example.com"));
        assert!(is_absolute_url("mailto:a@b.c"));
        assert!(is_absolute_url("file:///tmp/x"));
        assert!(!is_absolute_url("docs/readme.md"));
        assert!(!is_absolute_url("#anchor"));
        assert!(!is_absolute_url("C:\\path"));
        assert!(!is_absolute_url("javascript:alert(1)"));
    }

    #[test]
    fn images_show_alt_text_and_url() {
        assert_eq!(
            text("![a logo](img/logo.png)", 80),
            "▣ a logo (img/logo.png)"
        );
        assert_eq!(marked("![x](u)", 80), "{dim}▣ {/}{i}x{/}{dim,link0} (u){/}");
    }

    #[test]
    fn badges_show_only_the_link_url() {
        assert_eq!(
            text("[![build](https://ci/badge.svg)](https://ci)", 80),
            "▣ build (https://ci)"
        );
    }

    #[test]
    fn task_lists_replace_bullets() {
        assert_eq!(
            text("- [ ] todo\n- [x] done\n- plain", 40),
            "  ☐ todo\n  ☑ done\n  • plain"
        );
        assert_eq!(text("1. [x] first", 40), "  1. ☑ first");
        assert_eq!(
            with("- [ ] todo task\n- [x] done", 12, |o| o.ascii = true),
            "  [ ] todo\n      task\n  [x] done"
        );
    }

    #[test]
    fn footnotes() {
        let md = "Claim.[^1]\n\n[^1]: The source,\n    continued.";
        assert_eq!(text(md, 40), "Claim.[1]\n\n[1] The source, continued.");
        assert_eq!(
            text(md, 14),
            "Claim.[1]\n\n[1] The\n    source,\n    continued."
        );
    }

    #[test]
    fn github_alerts() {
        let md = "> [!WARNING]\n> Be careful.";
        assert_eq!(text(md, 40), "│ Warning\n│ Be careful.");
        assert_eq!(
            marked(md, 40),
            "{fg3}│{/} {b,fg3}Warning{/}\n{fg3}│{/} Be careful."
        );
        assert_eq!(text("> plain quote", 40), "│ plain quote");
    }

    #[test]
    fn front_matter() {
        let md = "---\ntitle: Hi\ntags: [a]\n---\n\n# Doc";
        assert_eq!(
            text(md, 20),
            "┌─ front matter (yaml)\n│ title: Hi\n│ tags: [a]\n└─\n\nDoc\n════════════════════"
        );
        assert_eq!(
            with(md, 20, |o| o.front_matter = false),
            "Doc\n════════════════════"
        );
        assert!(text("+++\ntitle = 1\n+++\n", 40).starts_with("┌─ front matter (toml)"));
    }

    #[test]
    fn tables() {
        let md = "| a | b |\n|:-|-:|\n| x | 10 |";
        assert_eq!(
            text(md, 40),
            "┌───┬────┐\n│ a │  b │\n├───┼────┤\n│ x │ 10 │\n└───┴────┘"
        );
    }

    #[test]
    fn table_cells_keep_inline_styles() {
        let r = render("| h |\n|---|\n| `c` **b** |", &opts(40));
        let row: String = markup(&r.lines[3]);
        assert!(
            row.contains("{fg1}c{/}") && row.contains("{b}b{/}"),
            "{row}"
        );
    }

    #[test]
    fn table_in_quote_respects_prefix_width() {
        let md = "> | col | other column |\n> |---|---|\n> | v | some longer text here |";
        let out = text(md, 30);
        assert!(out.lines().all(|l| l.width() <= 30), "{out}");
        assert!(out.lines().all(|l| l.starts_with("│ ")), "{out}");
    }

    #[test]
    fn ascii_symbols() {
        let md = "# T\n\n- a\n  - b\n\n> q\n\n```\nc\n```\n\n---";
        assert_eq!(
            with(md, 6, |o| o.ascii = true),
            "T\n======\n\n  * a\n      - b\n\n| q\n\n+-\n| c\n+-\n\n------"
        );
    }

    #[test]
    fn snapshot_extended_document() {
        let md = include_str!("../../tests/fixtures/markdown/extended.md");
        insta::assert_snapshot!("extended_80", text(md, 80));
        insta::assert_snapshot!("extended_36", text(md, 36));
        insta::assert_snapshot!("extended_ascii_50", with(md, 50, |o| o.ascii = true));
    }
}
