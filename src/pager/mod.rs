//! Interactive pager.
//!
//! `Pager` holds all state and handles keys; `run` connects it to the terminal.
//! Files are shown one at a time (`:n` and `:p` switch), each remembering its
//! scroll position.

mod editor;
pub mod help;
pub mod keys;
mod screen;
mod search;
mod viewport;

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::cursor::{self, MoveTo};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ansi::{self, ColorMode};
use crate::document::{Document, File, LazyHighlight, gutter_span};
use crate::render::{Heading, Rendered, is_absolute_url, slugs, squeeze_blank};
use crate::stream::Stream;
use crate::style::Line;
use crate::wrap::{Wrapped, column_of, slice_columns, wrap_all, wrap_line};
use keys::Action;
use screen::Screen;
use search::{RowMatches, Search};
use viewport::Viewport;

/// Loads a file again after it was edited (see `v`).
pub type Reload<'a> = &'a dyn Fn(&Path) -> Option<File>;
/// Give the terminal back after quitting. With `--keep-screen`, the last screen
/// is printed in the normal screen afterwards, so it stays in view.
fn finish(mut pager: Pager, screen: Screen, opts: &Options) -> io::Result<()> {
    drop(screen);
    if opts.keep_screen {
        // Laid out again for the normal screen, which can be a different size
        // (Warp gives full-screen programs a different one).
        if let Ok((cols, rows)) = terminal::size()
            && (cols, rows) != (pager.cols, pager.rows)
        {
            pager.resize(cols, rows);
        }
        let mut out = io::stdout().lock();
        for line in pager.screen_text() {
            writeln!(out, "{line}")?;
        }
        out.flush()?;
    }
    Ok(())
}

/// Where to start, from the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// `+120`: at line 120.
    Line(usize),
    /// `+G`: at the end.
    End,
    /// `+/pattern` or `+?pattern` (and `--pattern`): at the first match.
    Search { pattern: String, backward: bool },
}

impl Start {
    /// A `+` command (without the `+`).
    pub fn parse(command: &str) -> Result<Start, String> {
        if command == "G" {
            return Ok(Start::End);
        }
        if let Ok(line) = command.parse::<usize>() {
            return Ok(Start::Line(line));
        }
        match command.split_at_checked(1) {
            Some(("/", pattern)) if !pattern.is_empty() => Ok(Start::Search {
                pattern: pattern.to_string(),
                backward: false,
            }),
            Some(("?", pattern)) if !pattern.is_empty() => Ok(Start::Search {
                pattern: pattern.to_string(),
                backward: true,
            }),
            _ => Err(format!(
                "unknown start command '+{command}' (use +LINE, +G, +/pattern, or +?pattern)"
            )),
        }
    }
}

/// How to fetch a document from the web (for links in one, and `R`).
pub type Fetch<'a> = &'a dyn Fn(&str) -> Result<File, String>;

pub struct Options<'a> {
    /// Quit when moving forward past the end of the last file (classic `more`).
    pub exit_at_eof: bool,
    pub color: ColorMode,
    /// Emit OSC 8 hyperlinks.
    pub hyperlinks: bool,
    /// Report mouse buttons so the wheel scrolls.
    pub mouse: bool,
    /// Start with long lines cut off instead of wrapped (`-S`).
    pub chop: bool,
    /// Reload the current file when it changes on disk (`--watch`).
    pub watch: bool,
    /// How to load a file again after editing it.
    pub reload: Option<Reload<'a>>,
    /// Key bindings from the config file.
    pub keymap: &'a keys::Keymap,
    /// How to fetch web documents.
    pub fetch: Option<Fetch<'a>>,
    /// Where to start in the first file (`+120`, `+/pattern`).
    pub start: Option<&'a Start>,
    /// Leave the last screen in the terminal after quitting (`-X`).
    pub keep_screen: bool,
}

/// Page the document interactively until the user quits.
pub fn run(document: Document, opts: &Options) -> io::Result<()> {
    if document.files.is_empty() {
        return Ok(());
    }
    document.start_highlighting();
    // Start listening for size changes before taking over the screen: a
    // terminal may change the size as a full-screen program starts, and a
    // change signalled before the first wait for input would be missed.
    let _ = event::poll(Duration::ZERO);
    let screen = Screen::enter(opts.mouse)?;
    let mut out = BufWriter::new(io::stdout());
    let (cols, rows) = terminal::size()?;
    let mut pager = Pager::new(document, opts, cols, rows);
    pager.wait_for_first_screen(FIRST_SCREEN_WAIT);
    if let Some(start) = opts.start {
        pager.start_at(start);
    }
    // Keys pressed while the terminal was asked about its background come first.
    for key in keys::from_bytes(&crate::background::take_typeahead()) {
        match pager.key(key) {
            Flow::Quit => return finish(pager, screen, opts),
            // Suspending or editing before the first screen would be odd.
            Flow::Suspend | Flow::Edit | Flow::Continue => {}
        }
    }
    let mut late_replies = keys::LateReplies::default();
    let mut dirty = true;
    // Highlighted lines available at the last draw.
    let mut drawn = 0;
    loop {
        // In case a size change was missed after all (the signal can come at a
        // moment when nothing listens), compare with the real size.
        if let Ok((cols, rows)) = terminal::size()
            && (cols, rows) != (pager.cols, pager.rows)
        {
            pager.resize(cols, rows);
            dirty = true;
        }
        if dirty {
            pager.draw(&mut out)?;
            drawn = pager.highlight_progress();
            dirty = false;
        }
        // Highlighting or input may have finished since the last draw: show what
        // is new before waiting for a key, or it would never be pulled in.
        if pager.pull_stream()
            || (!pager.highlighting() && pager.newly_highlighted_on_screen(drawn))
        {
            dirty = true;
            continue;
        }
        // While highlighting runs, input arrives, or a file is watched, wake up
        // now and then.
        let busy = pager.highlighting() || pager.reading();
        let wait = if busy {
            HIGHLIGHT_POLL
        } else if opts.watch {
            WATCH_POLL
        } else {
            SIZE_POLL
        };
        let event = event::poll(wait)?.then(event::read).transpose()?;
        match event {
            None => {
                let highlighted = pager.newly_highlighted_on_screen(drawn);
                let changed = pager.check_watched();
                dirty = pager.pull_stream() || highlighted || changed;
            }
            Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if late_replies.is_reply(&key) {
                    continue;
                }
                match pager.key(key) {
                    Flow::Quit => break,
                    Flow::Suspend => {
                        screen.suspend()?;
                        // The terminal may have been resized meanwhile.
                        let (cols, rows) = terminal::size()?;
                        pager.resize(cols, rows);
                    }
                    Flow::Edit => {
                        match pager.edit_target() {
                            Ok((path, line)) => {
                                let result = screen.outside(|| editor::open(&path, line))?;
                                match result {
                                    Ok(()) => pager.reload(),
                                    Err(e) => {
                                        pager.message =
                                            Some(format!("Could not run the editor: {e}"))
                                    }
                                }
                            }
                            Err(message) => pager.message = Some(message),
                        }
                        let (cols, rows) = terminal::size()?;
                        pager.resize(cols, rows);
                    }
                    Flow::Continue => {}
                }
                dirty = true;
            }
            Some(Event::Mouse(mouse)) => dirty = pager.mouse(mouse.kind, mouse.column, mouse.row),
            Some(Event::Resize(..)) => {
                // Query again: the size in the event can be stale during a drag.
                let (cols, rows) = terminal::size()?;
                pager.resize(cols, rows);
                dirty = true;
            }
            Some(_) => {}
        }
    }
    finish(pager, screen, opts)
}

/// How long to wait for the first screen to be highlighted before showing it
/// plain. Small files are done by then; big files do not keep you waiting.
const FIRST_SCREEN_WAIT: Duration = Duration::from_millis(50);
/// Rows scrolled per wheel notch.
const WHEEL_ROWS: isize = 3;
/// How often to check for newly highlighted lines while highlighting runs.
const HIGHLIGHT_POLL: Duration = Duration::from_millis(25);
/// How often `--watch` looks at the file.
const WATCH_POLL: Duration = Duration::from_millis(250);
/// How often to check the terminal size when idle, in case a size change
/// signal was missed.
const SIZE_POLL: Duration = Duration::from_millis(500);

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
    /// Stop like a job (Ctrl-Z) and continue when resumed.
    Suspend,
    /// Open the current file in an editor (`v`).
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Search {
        backward: bool,
    },
    Command,
    Filter,
    /// Waiting for the letter after `m` (set) or `'` (go).
    Mark {
        set: bool,
    },
}

/// One laid-out screenful source: rows plus scroll position.
struct View {
    wrapped: Wrapped,
    vp: Viewport,
    /// Width of the line number gutter (0 when off).
    gutter: usize,
    /// Width the content was wrapped at (unlimited when long lines are cut off).
    width: usize,
    /// Number of source lines taken in (for streams: lines read so far). With a
    /// filter, more than the lines shown.
    lines: usize,
    /// Markdown headings, for `]`, `[`, and the table of contents.
    headings: Vec<Heading>,
    matches: RowMatches,
    /// Whether the last line taken in is blank (for `-s` as input arrives).
    ends_blank: bool,
}

impl View {
    /// Source line at the top of the screen, used to keep the place on re-layout.
    fn anchor(&self) -> usize {
        self.wrapped.source.get(self.vp.top).copied().unwrap_or(0)
    }
}

struct Pager<'a> {
    doc: Document,
    opts: &'a Options<'a>,
    cols: u16,
    rows: u16,
    file: usize,
    /// Saved anchor (source line) for each file.
    positions: Vec<usize>,
    raw: bool,
    view: View,
    help: Option<View>,
    prompt: Option<(PromptKind, String)>,
    search: Option<Search>,
    /// Row of the last match jumped to, with the scroll position at that time.
    /// `n` continues from there as long as the user has not scrolled since.
    cursor: Option<(usize, usize)>,
    message: Option<String>,
    /// Keep the end in view as input arrives (`F`).
    follow: bool,
    /// Whether the current file's input had ended at the last layout or pull.
    stream_done: bool,
    /// A count being typed, as in `120g`.
    count: Option<usize>,
    /// Long lines are cut off at the screen edge instead of wrapped.
    chop: bool,
    /// First column shown when lines are cut off.
    hscroll: usize,
    /// Only lines matching this are shown (`&pattern`).
    filter: Option<Search>,
    /// Heading last jumped to, with the scroll position then (like `cursor`).
    heading_cursor: Option<(usize, usize)>,
    /// The table of contents, while it is open.
    toc: Option<Toc>,
    /// Modification time and size of each file when last loaded, for `--watch`.
    stamps: Vec<Option<Stamp>>,
    /// When `--watch` last looked at the file.
    watched_at: std::time::Instant,
    /// Marks set with `m`: file and source line.
    marks: std::collections::HashMap<char, Place>,
    /// Where `''` goes: the place before the last jump.
    last_place: Option<Place>,
    /// While a search is being typed: how things were before, to go back to
    /// on Esc (search as you type moves the view).
    search_origin: Option<SearchOrigin>,
    /// The link selected with Tab (an index into the view's link table), which
    /// Enter follows.
    selected_link: Option<u32>,
    /// Places left by following links, for Backspace.
    history: Vec<Place>,
    /// Modifiers typed in the search or filter prompt (Ctrl-K, Ctrl-R).
    prompt_modifiers: PromptModifiers,
}

/// Ways to search, chosen with control keys in the prompt, as in less.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PromptModifiers {
    /// Ctrl-K: highlight the matches but stay where you are.
    keep: bool,
    /// Ctrl-R: the pattern is plain text, not a regular expression.
    literal: bool,
}

/// A place to come back to (marks, `''`, and Backspace after following a link).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    file: usize,
    /// The source line at the top of the screen.
    source: usize,
    /// How many of that line's rows were above the top: long lines, and lines
    /// with blank lines after them, take several rows.
    rows_in: usize,
}

/// The state that search as you type changes, saved when the prompt opens.
struct SearchOrigin {
    top: usize,
    hscroll: usize,
    matches: RowMatches,
    cursor: Option<(usize, usize)>,
}

/// What `--watch` compares to notice that a file changed.
type Stamp = (std::time::SystemTime, u64);

/// The path to watch for changes: a file on disk, read whole or decompressed
/// as it was read. Piped input and named pipes have nothing to compare.
fn watchable(file: &File) -> Option<&Path> {
    file.path
        .as_deref()
        .filter(|p| file.stream.is_none() || p.is_file())
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Table of contents state: the selected heading and the first one shown.
struct Toc {
    selected: usize,
    top: usize,
}

impl<'a> Pager<'a> {
    fn new(doc: Document, opts: &'a Options<'a>, cols: u16, rows: u16) -> Self {
        let files = doc.files.len();
        let stamps = doc
            .files
            .iter()
            .map(|f| watchable(f).and_then(stamp))
            .collect();
        let mut pager = Pager {
            doc,
            opts,
            cols,
            rows,
            file: 0,
            positions: vec![0; files],
            raw: false,
            view: empty_view(),
            help: None,
            prompt: None,
            search: None,
            cursor: None,
            message: None,
            follow: false,
            stream_done: true,
            count: None,
            chop: opts.chop,
            hscroll: 0,
            filter: None,
            heading_cursor: None,
            toc: None,
            stamps,
            watched_at: std::time::Instant::now(),
            marks: std::collections::HashMap::new(),
            last_place: None,
            search_origin: None,
            selected_link: None,
            history: Vec::new(),
            prompt_modifiers: PromptModifiers::default(),
        };
        pager.layout(0);
        pager
    }

    fn content_height(&self) -> usize {
        (self.rows as usize).saturating_sub(1).max(1)
    }

    /// Width available for content, next to the line number gutter.
    fn content_width(&self, gutter: usize) -> usize {
        (self.cols as usize).saturating_sub(gutter).max(1)
    }

    /// Lay out the current file at the current size, scrolled to `anchor`.
    fn layout(&mut self, anchor: usize) {
        let gutter = self.doc.gutter_width(self.file);
        let width = self.content_width(gutter);
        let mut rendered = self.doc.render_file(self.file, width, self.raw);
        let lines = rendered.lines.len();
        let headings = std::mem::take(&mut rendered.headings);
        let rendered = match &self.filter {
            Some(filter) => keep_matching(rendered, filter),
            None => rendered,
        };
        let (rendered, ends_blank) = if self.doc.squeeze {
            squeeze_blank(rendered, false)
        } else {
            (rendered, false)
        };
        let width = if self.chop { usize::MAX } else { width };
        let wrapped = wrap_all(rendered, width);
        let matches = self
            .search
            .as_ref()
            .map(|s| s.row_matches(&wrapped))
            .unwrap_or_default();
        let mut vp = Viewport::new(wrapped.rows.len(), self.content_height());
        let top = wrapped.first_row_of(anchor);
        vp.reset(wrapped.rows.len(), self.content_height(), top);
        self.view = View {
            wrapped,
            vp,
            gutter,
            width,
            lines,
            headings,
            matches,
            ends_blank,
        };
        self.heading_cursor = None;
        if self.follow {
            self.view.vp.top = self.view.vp.max_top();
        }
        self.stream_done = self.stream().is_none_or(Stream::is_done);
        self.cursor = None;
    }

    fn layout_help(&mut self) {
        let mut rendered = Rendered::default();
        for (i, line) in help::lines(self.opts.keymap).into_iter().enumerate() {
            rendered.push(line, i);
        }
        let wrapped = wrap_all(rendered, self.cols as usize);
        let vp = Viewport::new(wrapped.rows.len(), self.content_height());
        self.help = Some(View {
            wrapped,
            vp,
            gutter: 0,
            width: self.cols as usize,
            lines: 0,
            headings: Vec::new(),
            matches: RowMatches::default(),
            ends_blank: false,
        });
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        let anchor = self.view.anchor();
        self.layout(anchor);
        if let Some(help) = &self.help {
            let top = help.vp.top;
            self.layout_help();
            let height = self.content_height();
            if let Some(help) = &mut self.help {
                let total = help.wrapped.rows.len();
                help.vp.reset(total, height, top);
            }
        }
    }

    fn switch_file(&mut self, file: usize) {
        self.positions[self.file] = self.view.anchor();
        self.file = file;
        self.layout(self.positions[file]);
    }

    fn key(&mut self, key: KeyEvent) -> Flow {
        self.message = None;
        // Any key stops following, then does its usual job.
        self.follow = false;
        if let Some((kind, input)) = self.prompt.take() {
            return self.prompt_key(kind, input, key);
        }
        let action = self.opts.keymap.map(key);

        if let Some(help) = &mut self.help {
            match action {
                Action::Suspend => return Flow::Suspend,
                Action::Quit | Action::Help | Action::ClearSearch => self.help = None,
                _ => help.vp.apply(action),
            }
            return Flow::Continue;
        }
        if self.toc.is_some() {
            return self.toc_key(key, action);
        }

        // A selected link stays selected only while Tab moves between links.
        let selected = self.selected_link.take();
        match (action, selected) {
            (Action::LineDown, Some(link)) if key.code == KeyCode::Enter => {
                self.count = None;
                self.follow_link(link);
                return Flow::Continue;
            }
            (Action::NextLink | Action::PrevLink, _) => {
                self.count = None;
                self.select_link(action == Action::NextLink, selected);
                return Flow::Continue;
            }
            (Action::Back, _) => {
                self.count = None;
                self.go_back();
                return Flow::Continue;
            }
            _ => {}
        }

        // A count (as in `120g`) applies to the next key.
        if let Action::Digit(d) = action {
            let count = self.count.unwrap_or(0);
            self.count = Some(count.saturating_mul(10).saturating_add(usize::from(d)));
            return Flow::Continue;
        }
        let count = self.count.take();
        match (action, count) {
            (Action::Top | Action::Bottom, Some(line)) => {
                self.remember_place();
                self.go_to_line(line);
                return Flow::Continue;
            }
            (Action::Percent, count) => {
                self.remember_place();
                self.go_to_percent(count.unwrap_or(0));
                return Flow::Continue;
            }
            (Action::LineDown | Action::LineUp, Some(n)) => {
                let n = n.min(isize::MAX as usize) as isize;
                self.view
                    .vp
                    .scroll(if action == Action::LineDown { n } else { -n });
                return Flow::Continue;
            }
            // Esc first cancels a count being typed.
            (Action::ClearSearch, Some(_)) => return Flow::Continue,
            _ => {}
        }

        match action {
            Action::Quit => return Flow::Quit,
            Action::SearchForward | Action::SearchBackward => {
                let backward = action == Action::SearchBackward;
                self.search_origin = Some(SearchOrigin {
                    top: self.view.vp.top,
                    hscroll: self.hscroll,
                    matches: self.view.matches.clone(),
                    cursor: self.cursor,
                });
                self.prompt_modifiers = PromptModifiers::default();
                self.prompt = Some((PromptKind::Search { backward }, String::new()));
            }
            Action::Command => self.prompt = Some((PromptKind::Command, String::new())),
            Action::NextMatch => self.next_match(false),
            Action::PrevMatch => self.next_match(true),
            Action::ClearSearch => {
                self.search = None;
                self.view.matches = RowMatches::default();
                self.cursor = None;
            }
            Action::ShowPosition => self.message = Some(self.position()),
            Action::ToggleRaw => {
                let anchor = self.view.anchor();
                self.raw = !self.raw;
                self.layout(anchor);
                let what = if self.raw {
                    "raw source"
                } else {
                    "rendered view"
                };
                self.message = Some(format!("Showing {what}"));
            }
            Action::Help => self.layout_help(),
            Action::Follow => {
                self.follow = true;
                self.view.vp.apply(Action::Bottom);
            }
            Action::Suspend => return Flow::Suspend,
            Action::ToggleWrap => {
                let anchor = self.view.anchor();
                self.chop = !self.chop;
                self.hscroll = 0;
                self.layout(anchor);
                self.message = Some(if self.chop {
                    "Long lines are cut off (Left and Right scroll, w wraps them again)".into()
                } else {
                    "Long lines wrap".into()
                });
            }
            Action::ScrollLeft | Action::ScrollRight => {
                if !self.chop {
                    self.message =
                        Some("Long lines wrap; press w to cut them off and scroll".into());
                    return Flow::Continue;
                }
                let width = self.content_width(self.view.gutter);
                let step = (width / 2).max(1);
                if action == Action::ScrollLeft {
                    self.hscroll = self.hscroll.saturating_sub(step);
                } else {
                    let widest = self
                        .view
                        .wrapped
                        .rows
                        .iter()
                        .map(Line::width)
                        .max()
                        .unwrap_or(0);
                    self.hscroll = (self.hscroll + step).min(widest.saturating_sub(width));
                }
            }
            Action::Edit => return Flow::Edit,
            Action::Filter => {
                self.prompt_modifiers = PromptModifiers::default();
                self.prompt = Some((PromptKind::Filter, String::new()));
            }
            Action::NextHeading => self.next_heading(true),
            Action::PrevHeading => self.next_heading(false),
            Action::Contents => self.open_contents(),
            Action::SetMark => self.prompt = Some((PromptKind::Mark { set: true }, String::new())),
            Action::GoToMark => {
                self.prompt = Some((PromptKind::Mark { set: false }, String::new()))
            }
            // reload says why when a file cannot be reloaded.
            Action::Reload => {
                self.reload();
                if self.message.is_none() {
                    self.message = Some(format!("Reloaded {}", self.doc.files[self.file].name));
                }
            }
            Action::None | Action::Digit(_) | Action::Percent => {}
            movement => {
                if self.view.vp.at_end() && movement.moves_forward() {
                    let has_next = self.file + 1 < self.doc.files.len();
                    if has_next && (movement == Action::PageDown || self.opts.exit_at_eof) {
                        self.switch_file(self.file + 1);
                        return Flow::Continue;
                    }
                    // Input that is still arriving has no end to exit at yet.
                    if !has_next && self.opts.exit_at_eof && !self.reading() {
                        return Flow::Quit;
                    }
                }
                if matches!(movement, Action::Top | Action::Bottom) {
                    self.remember_place();
                }
                self.view.vp.apply(movement);
            }
        }
        Flow::Continue
    }

    /// Scroll on mouse wheel turns. Returns whether the screen needs a redraw.
    /// A mouse event at `column`, `row` (0-based). Returns whether to redraw.
    fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) -> bool {
        // Leave a prompt being typed alone.
        if self.prompt.is_some() {
            return false;
        }
        let delta = match kind {
            MouseEventKind::ScrollDown => WHEEL_ROWS,
            MouseEventKind::ScrollUp => -WHEEL_ROWS,
            MouseEventKind::Down(MouseButton::Left) => return self.click(column, row),
            _ => return false,
        };
        self.selected_link = None;
        let vp = match &mut self.help {
            Some(help) => &mut help.vp,
            None => &mut self.view.vp,
        };
        let before = vp.top;
        vp.scroll(delta);
        let moved = vp.top != before;
        if moved {
            self.message = None;
        }
        moved
    }

    fn prompt_key(&mut self, kind: PromptKind, mut input: String, key: KeyEvent) -> Flow {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.cancel_search_preview(),
            KeyCode::Char('c' | 'g') if ctrl => self.cancel_search_preview(),
            KeyCode::Char('u') if ctrl => {
                self.preview_search(kind, "");
                self.prompt = Some((kind, String::new()));
            }
            KeyCode::Char('k') if ctrl && matches!(kind, PromptKind::Search { .. }) => {
                self.prompt_modifiers.keep = !self.prompt_modifiers.keep;
                self.preview_search(kind, &input);
                self.prompt = Some((kind, input));
            }
            KeyCode::Char('r')
                if ctrl && matches!(kind, PromptKind::Search { .. } | PromptKind::Filter) =>
            {
                self.prompt_modifiers.literal = !self.prompt_modifiers.literal;
                self.preview_search(kind, &input);
                self.prompt = Some((kind, input));
            }
            KeyCode::Backspace => {
                // Backspace on an empty prompt cancels it, as in less.
                if input.pop().is_some() {
                    self.preview_search(kind, &input);
                    self.prompt = Some((kind, input));
                } else {
                    self.cancel_search_preview();
                }
            }
            KeyCode::Enter => match kind {
                PromptKind::Search { backward } => {
                    // Search from where the prompt was opened, as without the preview.
                    if let Some(origin) = self.search_origin.take() {
                        self.view.vp.top = origin.top;
                        self.hscroll = origin.hscroll;
                        self.view.matches = origin.matches;
                        self.cursor = origin.cursor;
                    }
                    self.start_search(&input, backward);
                }
                PromptKind::Filter => self.set_filter(&input),
                PromptKind::Command | PromptKind::Mark { .. } => {}
            },
            KeyCode::Char(c) if !ctrl => match kind {
                PromptKind::Command => return self.command(c),
                PromptKind::Mark { set: true } => self.set_mark(c),
                PromptKind::Mark { set: false } => self.go_to_mark(c),
                PromptKind::Search { .. } | PromptKind::Filter => {
                    input.push(c);
                    self.preview_search(kind, &input);
                    self.prompt = Some((kind, input));
                }
            },
            _ => self.prompt = Some((kind, input)),
        }
        Flow::Continue
    }

    /// Search as you type: show the first match of `input` from where the search
    /// prompt was opened, highlighting all matches. An empty or (still) invalid
    /// pattern shows the original place.
    fn preview_search(&mut self, kind: PromptKind, input: &str) {
        let PromptKind::Search { backward } = kind else {
            return;
        };
        let Some(origin) = &self.search_origin else {
            return;
        };
        let (top, hscroll) = (origin.top, origin.hscroll);
        let search = match Search::with(input, backward, self.prompt_modifiers.literal) {
            Ok(search) if !input.is_empty() => search,
            Ok(_) => {
                self.view.matches = origin.matches.clone();
                self.view.vp.top = top;
                self.hscroll = hscroll;
                return;
            }
            // Probably not finished typing (like "(" before ")"): keep the preview.
            Err(_) => return,
        };
        self.view.matches = search.row_matches(&self.view.wrapped);
        // Ctrl-K: highlight only, without moving.
        if self.prompt_modifiers.keep {
            self.view.vp.top = top;
            self.hscroll = hscroll;
            return;
        }
        let found = search::find(&self.view.matches.starts, top, !backward, !backward);
        let vp = &mut self.view.vp;
        match found {
            Some(row) => {
                vp.top = row.min(vp.max_top());
                self.reveal_match(row);
            }
            None => {
                vp.top = top;
                self.hscroll = hscroll;
            }
        }
    }

    /// Leaving the search prompt without searching: back to how things were.
    fn cancel_search_preview(&mut self) {
        if let Some(origin) = self.search_origin.take() {
            self.view.vp.top = origin.top;
            self.hscroll = origin.hscroll;
            self.view.matches = origin.matches;
            self.cursor = origin.cursor;
        }
    }

    /// `:` commands take effect on their first character, as in `more`.
    fn command(&mut self, c: char) -> Flow {
        let files = self.doc.files.len();
        match c {
            'n' if self.file + 1 < files => self.switch_file(self.file + 1),
            'n' => self.message = Some("No next file".into()),
            'p' if self.file > 0 => self.switch_file(self.file - 1),
            'p' => self.message = Some("No previous file".into()),
            'f' => self.message = Some(self.position()),
            'q' => return Flow::Quit,
            _ => self.message = Some(format!("Unknown command :{c} (h for help)")),
        }
        Flow::Continue
    }

    /// The current place, to come back to exactly.
    fn place(&self) -> Place {
        let source = self.view.anchor();
        let first = self.view.wrapped.first_row_of(source);
        Place {
            file: self.file,
            source,
            rows_in: self.view.vp.top.saturating_sub(first),
        }
    }

    /// Remember the current place for `''`, before a jump.
    fn remember_place(&mut self) {
        self.last_place = Some(self.place());
    }

    fn go_to_place(&mut self, place: Place) {
        if place.file != self.file && place.file < self.doc.files.len() {
            self.switch_file(place.file);
        }
        // Back to the same row of the line, if it still has that many (after a
        // resize it may wrap into fewer rows).
        let wrapped = &self.view.wrapped;
        let first = wrapped.first_row_of(place.source);
        let next = wrapped.first_row_of(place.source + 1);
        let row = (first + place.rows_in).min(next.saturating_sub(1).max(first));
        let vp = &mut self.view.vp;
        vp.top = row.min(vp.max_top());
    }

    /// A left click: follow the link under the mouse, if there is one.
    fn click(&mut self, column: u16, row: u16) -> bool {
        if self.help.is_some() || self.toc.is_some() {
            return false;
        }
        let Some(link) = self.link_at(column as usize, row as usize) else {
            return false;
        };
        self.message = None;
        self.selected_link = None;
        self.follow_link(link);
        true
    }

    /// The link shown at screen position `column`, `row`.
    fn link_at(&self, column: usize, row: usize) -> Option<u32> {
        let view = &self.view;
        if row >= view.vp.height {
            return None;
        }
        let line = view.wrapped.rows.get(view.vp.top + row)?;
        let mut x = column.checked_sub(view.gutter)?;
        if self.chop {
            x += self.hscroll;
        }
        let mut start = 0;
        for span in &line.spans {
            let end = start + span.text.width();
            if x < end {
                return span.style.link;
            }
            start = end;
        }
        None
    }

    /// Tab and Shift-Tab: select the next or previous link, starting from the
    /// screen, scrolling to it if needed.
    fn select_link(&mut self, forward: bool, selected: Option<u32>) {
        let order = link_order(&self.view.wrapped);
        if order.is_empty() {
            self.message = Some("No links here".into());
            return;
        }
        let top = self.view.vp.top;
        let bottom = top + self.view.vp.height;
        let current = selected.and_then(|s| order.iter().position(|&(_, i)| i == s));
        let target = match current {
            Some(p) if forward => (p + 1 < order.len()).then_some(p + 1),
            Some(p) => p.checked_sub(1),
            None if forward => order.iter().position(|&(row, _)| row >= top),
            None => order.iter().rposition(|&(row, _)| row < bottom),
        };
        let Some(target) = target else {
            self.selected_link = selected;
            self.message = Some("No more links".into());
            return;
        };
        let (row, link) = order[target];
        self.selected_link = Some(link);
        let vp = &mut self.view.vp;
        if row < top || row >= bottom {
            vp.top = row.saturating_sub(vp.height / 3).min(vp.max_top());
        }
        let url = &self.view.wrapped.links[link as usize];
        self.message = Some(format!(
            "Link: {url}  (Enter follows it, Backspace comes back)"
        ));
    }

    /// Enter on a selected link: a heading in this file (`#usage`), another local
    /// file (`docs/guide.md`, maybe with a heading), or a web page.
    fn follow_link(&mut self, link: u32) {
        let Some(url) = self.view.wrapped.links.get(link as usize).cloned() else {
            return;
        };
        let remote = self.doc.files[self.file].url.is_some();
        if let Some(path) = url.strip_prefix("file://") {
            // A page from the web does not get to open files on this machine.
            if remote {
                self.message = Some(format!("Cannot open {url} from a web page"));
            } else {
                self.follow_local(path);
            }
        } else if is_absolute_url(&url) {
            open_in_browser(&url, &mut self.message);
        } else {
            self.follow_local(&url);
        }
    }

    fn follow_local(&mut self, target: &str) {
        let (path, fragment) = match target.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (target, None),
        };
        let from = self.place();
        // In a document from the web, the link is an address relative to it.
        if let Some(base) = self.doc.files[self.file].url.clone()
            && !path.is_empty()
        {
            let Some(file) = self.open_remote(&crate::remote::join(&base, path)) else {
                return;
            };
            self.switch_file(file);
        }
        let path = percent_decode(path);
        if !path.is_empty() && self.doc.files[self.file].url.is_none() {
            let base = self.doc.files[self.file]
                .path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let Some(file) = self.open_linked(&base.join(&path)) else {
                return;
            };
            self.switch_file(file);
        }
        if let Some(fragment) = fragment.filter(|f| !f.is_empty()) {
            let wanted = percent_decode(fragment).to_lowercase();
            match slugs(&self.view.headings).iter().position(|s| *s == wanted) {
                Some(index) => self.go_to_heading(index),
                None => {
                    self.message = Some(format!("No heading #{wanted} here"));
                    if path.is_empty() {
                        return;
                    }
                }
            }
        } else if path.is_empty() {
            return;
        }
        self.history.push(from);
        self.last_place = Some(from);
    }

    /// The index of the document at web address `url`, fetching it if it is not
    /// open yet.
    fn open_remote(&mut self, url: &str) -> Option<usize> {
        if let Some(open) = self.doc.files.iter().position(|f| f.name == url) {
            return Some(open);
        }
        let Some(fetch) = self.opts.fetch else {
            self.message = Some(format!("Cannot fetch {url}"));
            return None;
        };
        match fetch(url) {
            Ok(file) => {
                self.doc.files.push(file);
                let index = self.doc.files.len() - 1;
                self.doc.start_highlighting_file(index);
                self.positions.push(0);
                self.stamps.push(None);
                Some(index)
            }
            Err(e) => {
                self.message = Some(format!("Cannot open {url}: {e}"));
                None
            }
        }
    }

    /// The index of the file at `path`, loading it if it is not open yet.
    fn open_linked(&mut self, path: &Path) -> Option<usize> {
        let Ok(real) = path.canonicalize() else {
            self.message = Some(format!("Cannot open {}: not found", path.display()));
            return None;
        };
        let open = self.doc.files.iter().position(|f| {
            f.path
                .as_deref()
                .and_then(|p| p.canonicalize().ok())
                .is_some_and(|p| p == real)
        });
        if open.is_some() {
            return open;
        }
        if real.is_dir() {
            self.message = Some(format!("Cannot open {}: a directory", path.display()));
            return None;
        }
        let file = self.opts.reload.and_then(|load| load(path));
        let Some(file) = file else {
            self.message = Some(format!("Cannot open {}", path.display()));
            return None;
        };
        self.doc.files.push(file);
        let index = self.doc.files.len() - 1;
        self.doc.start_highlighting_file(index);
        self.positions.push(0);
        self.stamps.push(stamp(path));
        Some(index)
    }

    /// Backspace: back to where the last followed link was.
    fn go_back(&mut self) {
        match self.history.pop() {
            Some(place) => {
                self.remember_place();
                self.go_to_place(place);
            }
            None => self.message = Some("No link to go back from".into()),
        }
    }

    /// `m` and a letter.
    fn set_mark(&mut self, letter: char) {
        if letter.is_ascii_alphabetic() {
            self.marks.insert(letter, self.place());
            self.message = Some(format!("Mark {letter} set"));
        } else {
            self.message = Some("Marks are letters (a to z, A to Z)".into());
        }
    }

    /// `'` and a letter goes to that mark; `''` goes back to before the last jump.
    fn go_to_mark(&mut self, letter: char) {
        let target = if letter == '\'' {
            self.last_place
        } else {
            self.marks.get(&letter).copied()
        };
        match target {
            Some(place) => {
                self.remember_place();
                self.go_to_place(place);
            }
            None if letter == '\'' => self.message = Some("No earlier place to go back to".into()),
            None => self.message = Some(format!("No mark {letter}")),
        }
    }

    /// Row where heading `index` is shown.
    fn heading_row(&self, index: usize) -> usize {
        self.view
            .wrapped
            .first_row_of(self.view.headings[index].source)
    }

    /// `]` and `[`: the next or previous heading. Like `n`, repeated presses keep
    /// going even when the screen cannot scroll any further.
    fn next_heading(&mut self, forward: bool) {
        let count = self.view.headings.len();
        if count == 0 {
            self.message = Some("No headings here (Markdown files have them)".into());
            return;
        }
        let top = self.view.vp.top;
        let target = match self.heading_cursor {
            Some((index, at)) if at == top => {
                if forward {
                    (index + 1 < count).then_some(index + 1)
                } else {
                    index.checked_sub(1)
                }
            }
            _ if forward => (0..count).find(|&i| self.heading_row(i) > top),
            _ => (0..count).rev().find(|&i| self.heading_row(i) < top),
        };
        match target {
            Some(index) => self.go_to_heading(index),
            None => self.message = Some("No more headings".into()),
        }
    }

    fn go_to_heading(&mut self, index: usize) {
        self.remember_place();
        let row = self.heading_row(index);
        let vp = &mut self.view.vp;
        vp.top = row.min(vp.max_top());
        self.heading_cursor = Some((index, vp.top));
    }

    /// `t`: open the table of contents, at the heading of the current section.
    fn open_contents(&mut self) {
        if self.view.headings.is_empty() {
            self.message = Some("No headings here (Markdown files have them)".into());
            return;
        }
        let anchor = self.view.anchor();
        let selected = self
            .view
            .headings
            .iter()
            .rposition(|h| h.source <= anchor)
            .unwrap_or(0);
        let mut toc = Toc { selected, top: 0 };
        keep_visible(&mut toc, self.content_height());
        self.toc = Some(toc);
    }

    fn toc_key(&mut self, key: KeyEvent, action: Action) -> Flow {
        let count = self.view.headings.len();
        let height = self.content_height();
        if key.code == KeyCode::Enter {
            if let Some(toc) = self.toc.take() {
                self.go_to_heading(toc.selected);
            }
            return Flow::Continue;
        }
        let Some(toc) = &mut self.toc else {
            return Flow::Continue;
        };
        let last = count.saturating_sub(1);
        let half = (height / 2).max(1);
        toc.selected = match action {
            Action::LineDown => (toc.selected + 1).min(last),
            Action::LineUp => toc.selected.saturating_sub(1),
            Action::PageDown => (toc.selected + height).min(last),
            Action::PageUp => toc.selected.saturating_sub(height),
            Action::HalfDown => (toc.selected + half).min(last),
            Action::HalfUp => toc.selected.saturating_sub(half),
            Action::Top => 0,
            Action::Bottom => last,
            Action::Suspend => return Flow::Suspend,
            Action::Quit | Action::Contents | Action::ClearSearch | Action::Help => {
                self.toc = None;
                return Flow::Continue;
            }
            _ => toc.selected,
        };
        keep_visible(toc, height);
        Flow::Continue
    }

    /// Go to source line `line` (counting from 1), as `120g` does.
    /// Go where the command line asked (`+120`, `+G`, `+/pattern`).
    fn start_at(&mut self, start: &Start) {
        match start {
            Start::Line(line) => self.go_to_line(*line),
            Start::End => self.view.vp.apply(Action::Bottom),
            Start::Search { pattern, backward } => {
                // A backward search starts from the end.
                if *backward {
                    self.view.vp.apply(Action::Bottom);
                    self.view.vp.top = self.view.wrapped.rows.len();
                }
                self.start_search(pattern, *backward);
                let vp = &mut self.view.vp;
                vp.top = vp.top.min(vp.max_top());
            }
        }
        // `''` goes back to the top.
        self.last_place = Some(Place {
            file: self.file,
            source: 0,
            rows_in: 0,
        });
    }

    fn go_to_line(&mut self, line: usize) {
        let row = self.view.wrapped.first_row_of(line.saturating_sub(1));
        let vp = &mut self.view.vp;
        vp.top = row.min(vp.max_top());
    }

    /// Go to `percent` of the way through the file, as `50%` does.
    fn go_to_percent(&mut self, percent: usize) {
        let vp = &mut self.view.vp;
        let row = vp.total.saturating_mul(percent.min(100)) / 100;
        vp.top = row.min(vp.max_top());
    }

    /// `&pattern`: show only matching lines. An empty pattern shows all again.
    fn set_filter(&mut self, input: &str) {
        let anchor = self.view.anchor();
        if input.is_empty() {
            if self.filter.take().is_some() {
                self.message = Some("Showing all lines".into());
            }
            self.layout(anchor);
            return;
        }
        let literal = std::mem::take(&mut self.prompt_modifiers).literal;
        match Search::with(input, false, literal) {
            Ok(filter) => {
                self.filter = Some(filter);
                self.layout(anchor);
            }
            Err(e) => self.message = Some(e),
        }
    }

    /// The file to open in the editor, and the line at the top of the screen.
    fn edit_target(&self) -> Result<(std::path::PathBuf, usize), String> {
        let file = &self.doc.files[self.file];
        match &file.path {
            Some(path) if crate::compress::format_of(path).is_some() => Err(format!(
                "{} is compressed, so it cannot be edited (R reloads it)",
                file.name
            )),
            Some(path) if file.stream.is_none() => Ok((path.clone(), self.view.anchor() + 1)),
            _ => Err(format!("{} cannot be opened in an editor", file.name)),
        }
    }

    /// `--watch`: reload the current file if it changed on disk since it was
    /// loaded. Looks at most every `WATCH_POLL`. Returns whether it reloaded.
    fn check_watched(&mut self) -> bool {
        if !self.opts.watch || self.watched_at.elapsed() < WATCH_POLL {
            return false;
        }
        self.watched_at = std::time::Instant::now();
        let Some(path) = watchable(&self.doc.files[self.file]) else {
            return false;
        };
        let now = stamp(path);
        if now.is_none() || now == self.stamps[self.file] {
            return false;
        }
        self.reload();
        true
    }

    /// Load the current file again (after editing, `R`, or `--watch`), keeping the
    /// place; at the end it stays at the end, so a growing log keeps showing it.
    fn reload(&mut self) {
        let file = &self.doc.files[self.file];
        // A document from the web is fetched again.
        if file.url.is_some() {
            let name = file.name.clone();
            match self.opts.fetch.map(|fetch| fetch(&name)) {
                Some(Ok(file)) => self.replace_file(file),
                Some(Err(e)) => self.message = Some(format!("Could not fetch {name} again: {e}")),
                None => {}
            }
            return;
        }
        let Some(reload) = self.opts.reload else {
            return;
        };
        let Some(path) = file.path.clone() else {
            self.message = Some("Piped input cannot be reloaded".into());
            return;
        };
        // Opening a named pipe again would wait for a new writer.
        if std::fs::metadata(&path).is_ok_and(|m| !m.is_file()) {
            self.message = Some(format!("{} cannot be reloaded", path.display()));
            return;
        }
        self.stamps[self.file] = stamp(&path);
        match reload(&path) {
            Some(file) => self.replace_file(file),
            None => {
                self.message = Some(format!("Could not read {} again", path.display()));
            }
        }
    }

    /// Show `file` instead of the current one, keeping the place (or staying at
    /// the end).
    fn replace_file(&mut self, file: File) {
        let at_end = self.view.vp.at_end() && self.view.vp.top > 0;
        self.doc.files[self.file] = file;
        self.doc.start_highlighting_file(self.file);
        let anchor = self.view.anchor();
        self.layout(anchor);
        if at_end {
            self.view.vp.top = self.view.vp.max_top();
        }
    }

    fn start_search(&mut self, input: &str, backward: bool) {
        // An empty pattern repeats the previous one, as in less.
        // (With the same modifiers.)
        let modifiers = std::mem::take(&mut self.prompt_modifiers);
        let (pattern, literal) = match (input.is_empty(), &self.search) {
            (true, Some(previous)) => (previous.pattern.clone(), previous.literal),
            (true, None) => return,
            (false, _) => (input.to_string(), modifiers.literal),
        };
        match Search::with(&pattern, backward, literal) {
            Ok(search) => {
                self.view.matches = search.row_matches(&self.view.wrapped);
                self.search = Some(search);
                self.cursor = None;
                // Ctrl-K: only highlight.
                if modifiers.keep {
                    return;
                }
                self.remember_place();
                self.jump(!backward, true);
            }
            Err(e) => self.message = Some(e),
        }
    }

    /// `n` (or `N` with `reverse`) in the direction of the current search.
    fn next_match(&mut self, reverse: bool) {
        let Some(search) = &self.search else {
            self.message = Some("No previous search (/ to search)".into());
            return;
        };
        let forward = search.backward == reverse;
        self.remember_place();
        self.jump(forward, false);
    }

    fn jump(&mut self, forward: bool, new_search: bool) {
        let vp = &mut self.view.vp;
        // Continue from the last match unless the user scrolled away from it.
        let (from, inclusive) = match self.cursor {
            Some((row, top)) if top == vp.top && !new_search => (row, false),
            _ => (vp.top, forward),
        };
        match search::find(&self.view.matches.starts, from, forward, inclusive) {
            Some(row) => {
                vp.top = row.min(vp.max_top());
                self.cursor = Some((row, vp.top));
                self.reveal_match(row);
            }
            None => self.jump_to_other_file(forward),
        }
    }

    /// No more matches in this file: go on to the first match in the next files
    /// (or the last match in the previous ones, going backward).
    fn jump_to_other_file(&mut self, forward: bool) {
        let Some(search) = self.search.clone() else {
            return;
        };
        let others: Vec<usize> = if forward {
            (self.file + 1..self.doc.files.len()).collect()
        } else {
            (0..self.file).rev().collect()
        };
        let Some(file) = others
            .into_iter()
            .find(|&f| self.file_has_match(f, &search))
        else {
            self.message = Some("Pattern not found".into());
            return;
        };
        self.switch_file(file);
        let starts = &self.view.matches.starts;
        let found = if forward {
            search::find(starts, 0, true, true)
        } else {
            starts
                .len()
                .checked_sub(1)
                .and_then(|last| search::find(starts, last, false, true))
        };
        if let Some(row) = found {
            let vp = &mut self.view.vp;
            vp.top = row.min(vp.max_top());
            self.cursor = Some((row, vp.top));
            self.reveal_match(row);
        }
        self.message = Some(format!("Found in {}", self.doc.files[file].name));
    }

    /// Whether file `index`, as it would be shown now, has a match.
    fn file_has_match(&self, index: usize, search: &Search) -> bool {
        let gutter = self.doc.gutter_width(index);
        let rendered = self
            .doc
            .render_file(index, self.content_width(gutter), self.raw);
        rendered.lines.iter().any(|line| {
            let text = line.text();
            search.is_match(&text) && self.filter.as_ref().is_none_or(|f| f.is_match(&text))
        })
    }

    /// With long lines cut off, scroll sideways so the match in `row` is visible.
    fn reveal_match(&mut self, row: usize) {
        if !self.chop {
            return;
        }
        let Some(start) = self.view.matches.ranges.get(row).and_then(|r| r.first()) else {
            return;
        };
        let col = column_of(&self.view.wrapped.rows[row], start.start);
        let width = self.content_width(self.view.gutter);
        if col < self.hscroll || col >= self.hscroll + width {
            self.hscroll = col.saturating_sub(width / 4);
        }
    }

    /// The current file's lazy highlighter, when highlighted lines can be shown.
    fn lazy(&self) -> Option<&LazyHighlight> {
        if self.raw || self.help.is_some() {
            return None;
        }
        self.doc.code(self.file)
    }

    /// Wait (up to `limit`) until the lines on screen are highlighted.
    fn wait_for_first_screen(&self, limit: Duration) {
        let deadline = std::time::Instant::now() + limit;
        let wrapped = &self.view.wrapped;
        let bottom = (self.view.vp.top + self.view.vp.height).min(wrapped.rows.len());
        let last_line = wrapped
            .source
            .get(bottom.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        while self.highlighting()
            && self.highlight_progress() <= last_line
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The current file's input stream, if it is piped input.
    fn stream(&self) -> Option<&Stream> {
        self.doc.stream(self.file)
    }

    /// Whether the current file's input is still arriving.
    fn reading(&self) -> bool {
        self.stream().is_some_and(|s| !s.is_done())
    }

    /// Append lines that arrived since the last call. Returns whether anything
    /// on screen (rows or the status line) changed.
    fn pull_stream(&mut self) -> bool {
        let Some(stream) = self.doc.stream(self.file) else {
            return false;
        };
        let done = stream.is_done();
        let ended = done && !self.stream_done;
        self.stream_done = done;
        // The gutter widens past 99,999 lines.
        if self.doc.gutter_width(self.file) != self.view.gutter {
            let anchor = self.view.anchor();
            self.layout(anchor);
            return true;
        }
        let have = self.view.lines;
        let new = stream.lines_from(have);
        if new.is_empty() {
            return ended;
        }
        let count = new.len();
        let mut rendered = Rendered::default();
        for (i, line) in new.into_iter().enumerate() {
            rendered.push(line, have + i);
        }
        if let Some(filter) = &self.filter {
            rendered = keep_matching(rendered, filter);
        }
        if self.doc.squeeze {
            (rendered, self.view.ends_blank) = squeeze_blank(rendered, self.view.ends_blank);
        }
        let mut more = wrap_all(rendered, self.view.width);
        // Match on the new rows while their line numbers still start at 0.
        let matches = self.search.as_ref().map(|s| s.row_matches(&more));
        let shown = self.view.wrapped.line.last().map_or(0, |l| l + 1);
        for line in &mut more.line {
            *line += shown;
        }
        let view = &mut self.view;
        view.wrapped.rows.append(&mut more.rows);
        view.wrapped.source.append(&mut more.source);
        view.wrapped.line.append(&mut more.line);
        view.wrapped.offset.append(&mut more.offset);
        if let Some(mut m) = matches {
            view.matches.ranges.append(&mut m.ranges);
            view.matches.starts.append(&mut m.starts);
        }
        view.lines += count;
        let (height, top) = (view.vp.height, view.vp.top);
        view.vp.reset(view.wrapped.rows.len(), height, top);
        if self.follow {
            view.vp.top = view.vp.max_top();
        }
        true
    }

    /// Whether the current file is still being highlighted.
    fn highlighting(&self) -> bool {
        self.lazy().is_some_and(|lazy| !lazy.is_done())
    }

    fn highlight_progress(&self) -> usize {
        self.lazy().map_or(0, LazyHighlight::progress)
    }

    /// Whether lines that were plain at the last draw (when `drawn` lines were
    /// highlighted) are on screen and now have their highlighted version.
    fn newly_highlighted_on_screen(&self, drawn: usize) -> bool {
        let wrapped = &self.view.wrapped;
        let vp = &self.view.vp;
        let Some(&last_line) = wrapped.source.get(
            (vp.top + vp.height)
                .min(wrapped.rows.len())
                .saturating_sub(1),
        ) else {
            return false;
        };
        drawn <= last_line && self.highlight_progress() > drawn
    }

    /// Row `r` of the current view, highlighted if its line is ready. Highlighting
    /// does not change the text, so the highlighted line wraps into the same rows.
    fn row(&self, view: &View, r: usize) -> Line {
        let row = &view.wrapped.rows[r];
        let Some(lazy) = self.lazy() else {
            return row.clone();
        };
        // Code is shown line for line, so the source line is the highlighted line.
        let Some(highlighted) = lazy.line(view.wrapped.source[r]) else {
            return row.clone();
        };
        // Which of this line's rows `r` is.
        let line = view.wrapped.line[r];
        let first = (0..=r)
            .rev()
            .take_while(|&i| view.wrapped.line[i] == line)
            .last()
            .unwrap_or(r);
        wrap_line(&highlighted, view.width)
            .into_iter()
            .nth(r - first)
            .unwrap_or_else(|| row.clone())
    }

    /// "name (file 2 of 3), line 12 of 300", for `=` and `:f`.
    fn position(&self) -> String {
        let line = self.view.anchor() + 1;
        let file = &self.doc.files[self.file];
        let more = if self.reading() { "+" } else { "" };
        let total = format!("{}{more}", file.line_count());
        let name = &self.doc.files[self.file].name;
        let files = self.doc.files.len();
        if files > 1 {
            format!(
                "{name} (file {} of {files}), line {line} of {total}",
                self.file + 1
            )
        } else {
            format!("{name}, line {line} of {total}")
        }
    }

    fn status(&self) -> String {
        if let Some(help) = &self.help {
            let at = if help.vp.at_end() {
                "(END)"
            } else {
                "--More--"
            };
            return match help::close_keys(self.opts.keymap).first() {
                Some(key) => format!("Help {at}  {key} to close"),
                None => format!("Help {at}"),
            };
        }
        let vp = &self.view.vp;
        let mut status = if self.follow {
            "Following new input (any key to stop)".to_string()
        } else if self.reading() {
            // Percentages mean nothing while the end is unknown.
            if vp.at_end() {
                "(waiting for input)"
            } else {
                "--More--"
            }
            .to_string()
        } else if vp.at_end() {
            "(END)".to_string()
        } else {
            format!("--More--({}%)", vp.percent())
        };
        let files = self.doc.files.len();
        if files > 1 {
            let name = &self.doc.files[self.file].name;
            status.push_str(&format!("  [{}/{files}] {name}", self.file + 1));
            if vp.at_end() && self.file + 1 < files {
                let next = &self.doc.files[self.file + 1].name;
                status.push_str(&format!("  Next file: {next} (Space or :n)"));
            }
        }
        if self.raw {
            status.push_str("  [source]");
        }
        if let Some(filter) = &self.filter {
            let shown = self.view.wrapped.line.last().map_or(0, |l| l + 1);
            status.push_str(&format!("  [&{}: {shown} lines]", filter.pattern));
        }
        if self.chop && self.hscroll > 0 {
            status.push_str(&format!("  [from column {}]", self.hscroll + 1));
        }
        if self.opts.watch {
            status.push_str("  [watching]");
        }
        status
    }

    /// Row `r` of `view` as shown: with the line number gutter, search
    /// highlighting, the selected link, and cut to the screen with `-S`.
    fn screen_row(&self, view: &View, r: usize) -> Line {
        let row = self.row(view, r);
        let mut line = Line::default();
        if view.gutter > 0 {
            let source = view.wrapped.source[r];
            let first = r == 0 || view.wrapped.source[r - 1] != source;
            line.spans
                .push(gutter_span(first.then_some(source + 1), view.gutter));
        }
        let ranges = view.matches.ranges.get(r).map_or(&[][..], Vec::as_slice);
        let mut content = search::highlight(&row, ranges);
        if let Some(link) = self.selected_link.filter(|_| self.help.is_none()) {
            for span in &mut content.spans {
                if span.style.link == Some(link) {
                    span.style.reverse = !span.style.reverse;
                }
            }
        }
        if self.chop && self.help.is_none() {
            let width = self.content_width(view.gutter);
            line.append(slice_columns(&content, self.hscroll, width));
        } else {
            line.append(content);
        }
        line
    }

    /// The content rows on the screen now (not the status line), with escape
    /// sequences, for `--keep-screen`.
    fn screen_text(&self) -> Vec<String> {
        let view = self.help.as_ref().unwrap_or(&self.view);
        let mode = self.opts.color.max(ColorMode::Mono);
        let links = self
            .opts
            .hyperlinks
            .then_some(view.wrapped.links.as_slice());
        let end = (view.vp.top + view.vp.height).min(view.wrapped.rows.len());
        (view.vp.top..end)
            .map(|r| ansi::render(&self.screen_row(view, r), mode, links))
            .collect()
    }

    /// The table of contents, as screen lines.
    fn contents_lines(&self, toc: &Toc) -> Vec<Line> {
        let width = (self.cols as usize).saturating_sub(1);
        (toc.top..self.view.headings.len())
            .take(self.content_height())
            .map(|i| {
                let heading = &self.view.headings[i];
                let indent = "  ".repeat(usize::from(heading.level.saturating_sub(1)));
                let style = crate::style::Style {
                    bold: heading.level == 1,
                    reverse: i == toc.selected,
                    ..crate::style::Style::default()
                };
                let mut line = Line::default();
                line.push(&format!(" {indent}{} ", heading.text), style);
                slice_columns(&line, 0, width)
            })
            .collect()
    }

    fn draw(&self, out: &mut impl Write) -> io::Result<()> {
        if let Some(toc) = &self.toc {
            let mode = self.opts.color.max(ColorMode::Mono);
            let lines = self.contents_lines(toc);
            let height = self.content_height();
            for i in 0..height {
                queue!(out, MoveTo(0, i as u16), Clear(ClearType::CurrentLine))?;
                if let Some(line) = lines.get(i) {
                    queue!(out, Print(ansi::render(line, mode, None)))?;
                }
            }
            let max = (self.cols as usize).saturating_sub(1);
            let keymap = self.opts.keymap;
            let first = |actions: &[Action], defaults: &[(&str, KeyCode)]| {
                keymap.keys_for(actions, defaults).into_iter().next()
            };
            let down = first(
                &[Action::LineDown],
                &[("j", KeyCode::Char('j')), ("Down", KeyCode::Down)],
            );
            let up = first(
                &[Action::LineUp],
                &[("k", KeyCode::Char('k')), ("Up", KeyCode::Up)],
            );
            let close = first(
                &[
                    Action::Quit,
                    Action::Contents,
                    Action::ClearSearch,
                    Action::Help,
                ],
                &[
                    ("q", KeyCode::Char('q')),
                    ("t", KeyCode::Char('t')),
                    ("Esc", KeyCode::Esc),
                ],
            );
            let mut parts = vec!["Contents:".to_string()];
            match (down, up) {
                (Some(down), Some(up)) => parts.push(format!("{down}/{up} to choose,")),
                (Some(key), None) | (None, Some(key)) => parts.push(format!("{key} to choose,")),
                (None, None) => {}
            }
            parts.push("Enter to go there".into());
            if let Some(close) = close {
                parts.last_mut().expect("just pushed").push(',');
                parts.push(format!("{close} to close"));
            }
            let status = parts.join(" ");
            let status = status.as_str();
            queue!(
                out,
                MoveTo(0, height as u16),
                Clear(ClearType::CurrentLine),
                cursor::Hide,
                SetAttribute(Attribute::Reverse),
                Print(head(status, max)),
                SetAttribute(Attribute::Reset),
            )?;
            return out.flush();
        }
        let view = self.help.as_ref().unwrap_or(&self.view);
        let links = self
            .opts
            .hyperlinks
            .then_some(view.wrapped.links.as_slice());
        // Search highlighting needs reverse video even without colors.
        let mode = self.opts.color.max(ColorMode::Mono);
        let rows = &view.wrapped.rows;
        for i in 0..view.vp.height {
            // Clear before printing: clearing after a full-width row would erase its last cell.
            queue!(out, MoveTo(0, i as u16), Clear(ClearType::CurrentLine))?;
            let r = view.vp.top + i;
            if r >= rows.len() {
                continue;
            }
            let line = self.screen_row(view, r);
            queue!(out, Print(ansi::render(&line, mode, links)))?;
        }

        // Stay short of the last column so the terminal never scrolls.
        let max = (self.cols as usize).saturating_sub(1);
        let bottom = view.vp.height as u16;
        queue!(out, MoveTo(0, bottom), Clear(ClearType::CurrentLine))?;
        let typing = match (&self.prompt, self.count) {
            (Some((kind, input)), _) => {
                let prefix = match kind {
                    PromptKind::Search { backward: false } => "/",
                    PromptKind::Search { backward: true } => "?",
                    PromptKind::Command => ":",
                    PromptKind::Filter => "&",
                    PromptKind::Mark { set: true } => "m",
                    PromptKind::Mark { set: false } => "'",
                };
                let mut modifiers = String::new();
                if matches!(kind, PromptKind::Search { .. } | PromptKind::Filter) {
                    if self.prompt_modifiers.keep {
                        modifiers.push_str("Keep-pos ");
                    }
                    if self.prompt_modifiers.literal {
                        modifiers.push_str("Regex-off ");
                    }
                }
                Some(format!("{modifiers}{prefix}{input}"))
            }
            (None, Some(count)) => Some(count.to_string()),
            (None, None) => None,
        };
        if let Some(text) = typing {
            // Keep the end of long input visible.
            let shown = tail(&text, max);
            queue!(
                out,
                Print(shown),
                cursor::MoveTo(shown.width() as u16, bottom),
                cursor::Show
            )?;
        } else {
            let status = self.message.clone().unwrap_or_else(|| self.status());
            queue!(
                out,
                cursor::Hide,
                SetAttribute(Attribute::Reverse),
                Print(head(&status, max)),
                SetAttribute(Attribute::Reset),
            )?;
        }
        out.flush()
    }
}

/// Scroll the table of contents so the selected heading is visible.
fn keep_visible(toc: &mut Toc, height: usize) {
    if toc.selected < toc.top {
        toc.top = toc.selected;
    } else if toc.selected >= toc.top + height {
        toc.top = toc.selected + 1 - height;
    }
}

/// Only the rendered lines that match `filter`, keeping their source lines.
fn keep_matching(rendered: Rendered, filter: &Search) -> Rendered {
    let mut out = Rendered {
        links: rendered.links,
        headings: rendered.headings,
        ..Rendered::default()
    };
    for (line, source) in rendered.lines.into_iter().zip(rendered.source) {
        if filter.is_match(&line.text()) {
            out.push(line, source);
        }
    }
    out
}

/// Links in the order they appear: the first row each is on, and its index.
fn link_order(wrapped: &Wrapped) -> Vec<(usize, u32)> {
    let mut seen = std::collections::HashSet::new();
    let mut order = Vec::new();
    for (row, line) in wrapped.rows.iter().enumerate() {
        for span in &line.spans {
            if let Some(link) = span.style.link
                && seen.insert(link)
            {
                order.push((row, link));
            }
        }
    }
    order
}

/// Decode `%20` and the like in a link to a file.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Open a web or mail link with the system's handler. Other schemes are not
/// opened: `open` would also launch applications.
fn open_in_browser(url: &str, message: &mut Option<String>) {
    let lower = url.to_ascii_lowercase();
    if !["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        *message = Some(format!("Cannot open {url} from here"));
        return;
    }
    if cfg!(test) {
        *message = Some(format!("Opening {url}"));
        return;
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let spawned = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    *message = Some(match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
            format!("Opening {url}")
        }
        Err(e) => format!("Cannot run {program}: {e}"),
    });
}

fn empty_view() -> View {
    View {
        wrapped: Wrapped::default(),
        vp: Viewport::new(0, 1),
        gutter: 0,
        width: 1,
        lines: 0,
        headings: Vec::new(),
        matches: RowMatches::default(),
        ends_blank: false,
    }
}

/// The longest prefix of `s` that fits in `max` columns.
fn head(s: &str, max: usize) -> &str {
    let mut width = 0;
    for (i, ch) in s.char_indices() {
        width += ch.width().unwrap_or(0);
        if width > max {
            return &s[..i];
        }
    }
    s
}

/// The longest suffix of `s` that fits in `max` columns.
fn tail(s: &str, max: usize) -> &str {
    let mut width = 0;
    for (i, ch) in s.char_indices().rev() {
        width += ch.width().unwrap_or(0);
        if width > max {
            return &s[i + ch.len_utf8()..];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets;
    use crate::document::{File, Section};
    use crate::render::plain;

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    fn doc(files: &[(&str, &str)]) -> Document {
        Document {
            files: files
                .iter()
                .map(|(name, text)| File {
                    name: name.to_string(),
                    path: None,
                    url: None,
                    content: if name.ends_with(".md") {
                        Section::Markdown(text.to_string())
                    } else {
                        Section::Plain
                    },
                    raw: plain::render(text),
                    stream: None,
                })
                .collect(),
            width: None,
            max_width: 100,
            highlight: false,
            theme: assets::theme(assets::DEFAULT_DARK_THEME).unwrap(),
            ascii: false,
            front_matter: true,
            line_numbers: false,
            squeeze: false,
        }
    }

    const OPTS: Options<'static> = Options {
        exit_at_eof: false,
        color: ColorMode::None,
        hyperlinks: false,
        mouse: false,
        chop: false,
        watch: false,
        reload: None,
        keymap: &NO_KEYS,
        fetch: None,
        start: None,
        keep_screen: false,
    };

    static NO_KEYS: keys::Keymap = keys::Keymap::new();

    fn press(p: &mut Pager, keys: &str) -> Flow {
        let mut flow = Flow::Continue;
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\x1b' => KeyCode::Esc,
                '\x08' => KeyCode::Backspace,
                '\t' => KeyCode::Tab,
                '\u{2190}' => KeyCode::BackTab,
                c => KeyCode::Char(c),
            };
            flow = p.key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        flow
    }

    fn screen(p: &Pager) -> String {
        let mut buf = Vec::new();
        p.draw(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    /// The screen with SGR styling removed (cursor movement stays).
    fn plain_screen(p: &Pager) -> String {
        let raw = screen(p);
        let mut out = String::new();
        let mut rest = raw.as_str();
        while let Some(i) = rest.find("\x1b[") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            match after.find(|c: char| c.is_ascii_alphabetic()) {
                Some(end) if after.as_bytes()[end] == b'm' => rest = &after[end + 1..],
                Some(end) => {
                    out.push_str(&rest[i..i + 2 + end + 1]);
                    rest = &after[end + 1..];
                }
                None => break,
            }
        }
        out.push_str(rest);
        out
    }

    fn top_text(p: &Pager) -> String {
        p.view.wrapped.rows[p.view.vp.top].text()
    }

    #[test]
    fn search_jumps_and_highlights() {
        let d = doc(&[("a.txt", &numbered(100))]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, "/line 5\n");
        assert_eq!(top_text(&p), "line 5");
        // Matches are shown in reverse video.
        assert!(screen(&p).contains("\x1b[7mline 5\x1b[0m"));
        press(&mut p, "n");
        assert_eq!(top_text(&p), "line 50");
        press(&mut p, "n");
        assert_eq!(top_text(&p), "line 51");
        press(&mut p, "N");
        assert_eq!(top_text(&p), "line 50");
    }

    #[test]
    fn search_regex_smart_case_and_not_found() {
        let d = doc(&[("a.txt", "alpha\nBeta\nbeta\n")]);
        let mut p = Pager::new(d, &OPTS, 80, 2);
        press(&mut p, "/Beta\n");
        assert_eq!(top_text(&p), "Beta");
        press(&mut p, "n");
        assert_eq!(p.message.as_deref(), Some("Pattern not found"));
        press(&mut p, "g/b.ta\n");
        assert_eq!(top_text(&p), "Beta");
        press(&mut p, "/(\n");
        assert!(p.message.as_deref().unwrap().starts_with("Invalid pattern"));
    }

    #[test]
    fn search_continues_in_the_other_files() {
        let mut p = Pager::new(
            doc(&[
                ("a.txt", "apple\nneedle one\n"),
                ("b.txt", "nothing here\n"),
                ("c.txt", "x\ny\nneedle two\nneedle three\n"),
            ]),
            &OPTS,
            40,
            2,
        );
        press(&mut p, "/needle\n");
        assert_eq!(p.file, 0);
        press(&mut p, "n");
        assert_eq!(p.file, 2, "skips the file without a match");
        assert_eq!(top_text(&p), "needle two");
        assert_eq!(p.message.as_deref(), Some("Found in c.txt"));
        press(&mut p, "n");
        assert_eq!(top_text(&p), "needle three");
        press(&mut p, "n");
        assert_eq!(p.message.as_deref(), Some("Pattern not found"));
        // Backward goes to the last match of an earlier file.
        press(&mut p, "NN");
        assert_eq!(p.file, 0);
        assert_eq!(top_text(&p), "needle one");
        // A new search that only matches elsewhere goes there too.
        press(&mut p, "/nothing\n");
        assert_eq!(p.file, 1);
        press(&mut p, "''");
        assert_eq!(p.file, 0);
    }

    #[test]
    fn backward_search_and_repeat_with_empty_pattern() {
        let d = doc(&[("a.txt", &numbered(30))]);
        let mut p = Pager::new(d, &OPTS, 80, 5);
        press(&mut p, "G");
        assert_eq!(top_text(&p), "line 27");
        press(&mut p, "?line 1\n");
        assert_eq!(top_text(&p), "line 19");
        // `n` continues backward for a `?` search; `N` goes forward.
        press(&mut p, "n");
        assert_eq!(top_text(&p), "line 18");
        press(&mut p, "N");
        assert_eq!(top_text(&p), "line 19");
        // An empty pattern repeats the last one.
        press(&mut p, "g/\n");
        assert_eq!(top_text(&p), "line 1");
    }

    #[test]
    fn matches_at_the_end_still_advance() {
        // The last screen cannot scroll further, but n must still move on.
        let d = doc(&[("a.txt", "x\nx\nx\nhit a\nhit b\nhit c\n")]);
        let mut p = Pager::new(d, &OPTS, 80, 5);
        press(&mut p, "/hit\n");
        let first = p.cursor;
        press(&mut p, "n");
        assert_ne!(p.cursor, first);
        press(&mut p, "n");
        assert_eq!(p.cursor.map(|c| c.0), Some(5));
        press(&mut p, "n");
        assert_eq!(p.message.as_deref(), Some("Pattern not found"));
    }

    #[test]
    fn prompt_editing() {
        let d = doc(&[("a.txt", &numbered(20))]);
        let mut p = Pager::new(d, &OPTS, 80, 5);
        press(&mut p, "/line 9x\x08");
        assert!(screen(&p).contains("/line 9"));
        press(&mut p, "\n");
        assert_eq!(top_text(&p), "line 9");
        // Escape cancels, Backspace on an empty prompt cancels.
        press(&mut p, "/abc\x1b");
        assert!(p.prompt.is_none());
        press(&mut p, "/\x08");
        assert!(p.prompt.is_none());
    }

    #[test]
    fn switching_files_keeps_positions() {
        let d = doc(&[("a.txt", &numbered(50)), ("b.txt", &numbered(50))]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, " ");
        assert_eq!(top_text(&p), "line 10");
        assert!(screen(&p).contains("[1/2] a.txt"));
        press(&mut p, ":n");
        assert_eq!(p.file, 1);
        assert_eq!(top_text(&p), "line 1");
        press(&mut p, ":n");
        assert_eq!(p.message.as_deref(), Some("No next file"));
        press(&mut p, ":p");
        assert_eq!(top_text(&p), "line 10");
    }

    #[test]
    fn space_at_end_goes_to_next_file() {
        let d = doc(&[("a.txt", &numbered(12)), ("b.txt", "other\n")]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, "G");
        assert!(screen(&p).contains("Next file: b.txt"));
        // A line step at the end does not switch files; a page step does.
        press(&mut p, "j");
        assert_eq!(p.file, 0);
        press(&mut p, " ");
        assert_eq!(p.file, 1);
        assert_eq!(top_text(&p), "other");
    }

    #[test]
    fn exit_at_eof_quits_after_last_file() {
        let opts = Options {
            exit_at_eof: true,
            ..OPTS
        };
        let d = doc(&[("a.txt", &numbered(12))]);
        let mut p = Pager::new(d, &opts, 80, 10);
        assert_eq!(press(&mut p, " "), Flow::Continue);
        assert_eq!(press(&mut p, " "), Flow::Quit);
    }

    #[test]
    fn colon_commands() {
        let d = doc(&[("a.txt", &numbered(5))]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, ":f");
        assert_eq!(p.message.as_deref(), Some("a.txt, line 1 of 5"));
        press(&mut p, ":x");
        assert!(p.message.as_deref().unwrap().starts_with("Unknown command"));
        assert_eq!(press(&mut p, ":q"), Flow::Quit);
    }

    #[test]
    fn position_message() {
        let d = doc(&[("a.txt", &numbered(40)), ("b.txt", "x\n")]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, "d=");
        assert_eq!(
            p.message.as_deref(),
            Some("a.txt (file 1 of 2), line 5 of 40")
        );
        // Messages last until the next key.
        press(&mut p, "j");
        assert!(p.message.is_none());
    }

    #[test]
    fn raw_toggle_keeps_place() {
        let md = format!("# Title\n\n{}", "para\n\n".repeat(30));
        let d = doc(&[("a.md", &md)]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        assert_eq!(top_text(&p), "Title");
        press(&mut p, "r");
        assert_eq!(top_text(&p), "# Title");
        // The confirmation message shows first; the status line marks the mode.
        assert!(screen(&p).contains("Showing raw source"));
        assert!(p.status().contains("[source]"));
        press(&mut p, "dr");
        assert_eq!(p.view.anchor(), 4);
        assert!(!p.status().contains("[source]"));
    }

    #[test]
    fn help_overlay() {
        let d = doc(&[("a.txt", &numbered(50))]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, " h");
        assert!(screen(&p).contains("mmore keys"));
        // Keys move the help, not the file; q closes help instead of quitting.
        assert_eq!(press(&mut p, "jq"), Flow::Continue);
        assert!(p.help.is_none());
        assert_eq!(top_text(&p), "line 10");
    }

    #[test]
    fn line_number_gutter() {
        let mut d = doc(&[("a.txt", "short\nthis line is long enough to wrap\n")]);
        d.line_numbers = true;
        let p = Pager::new(d, &OPTS, 20, 10);
        let rows: Vec<String> = p.view.wrapped.rows.iter().map(Line::text).collect();
        // Content wraps (between words) at the width minus the 4-column gutter.
        assert_eq!(rows[1..], ["this line is ", "long enough to ", "wrap"]);
        let s = plain_screen(&p);
        assert!(s.contains("  1 short"));
        assert!(s.contains("  2 this line is"));
        assert!(s.contains("    long enough to"));
        assert!(s.contains("    wrap"));
    }

    #[test]
    fn resize_keeps_search_highlights() {
        let d = doc(&[("a.txt", &numbered(40))]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        press(&mut p, "/line 30\n");
        p.resize(40, 20);
        assert!(screen(&p).contains("\x1b[7mline 30\x1b[0m"));
    }

    #[test]
    fn code_is_highlighted_on_screen_once_ready() {
        use crate::document::LazyHighlight;
        use std::sync::Arc;
        let text = "fn main() {\n    let wrapped_long_identifier_name = 1;\n}\n";
        let syntax = assets::syntaxes().find_syntax_by_extension("rs").unwrap();
        let theme = assets::theme(assets::DEFAULT_DARK_THEME).unwrap();
        let lazy = Arc::new(LazyHighlight::new(text.into(), syntax, theme));
        let mut d = doc(&[]);
        d.files.push(File {
            name: "a.rs".into(),
            path: None,
            url: None,
            content: Section::Code(Arc::clone(&lazy)),
            raw: plain::render(text),
            stream: None,
        });
        let opts = Options {
            color: ColorMode::TrueColor,
            ..OPTS
        };
        let p = Pager::new(d, &opts, 20, 10);
        assert!(p.highlighting());
        assert!(!screen(&p).contains("38;2;"));
        assert!(!p.newly_highlighted_on_screen(0));
        lazy.run();
        assert!(p.newly_highlighted_on_screen(0));
        assert!(!p.highlighting());
        let s = screen(&p);
        assert!(s.contains("38;2;"));
        // The wrapped second line keeps its text across rows, highlighted or not:
        // between words, and inside a word longer than a row.
        let rows: Vec<String> = p.view.wrapped.rows.iter().map(Line::text).collect();
        assert_eq!(
            rows[1..4],
            ["    let ", "wrapped_long_identif", "ier_name = 1;"]
        );
        let s = plain_screen(&p);
        assert!(s.contains("wrapped_long_identif") && s.contains("ier_name = 1;"));
        // The raw view shows no highlighting.
        let mut p = p;
        press(&mut p, "r");
        assert!(!screen(&p).contains("38;2;"));
    }

    #[test]
    fn mouse_wheel_scrolls() {
        let d = doc(&[("a.txt", &numbered(50)), ("b.txt", "x\n")]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        assert!(p.mouse(MouseEventKind::ScrollDown, 0, 0));
        assert_eq!(top_text(&p), "line 4");
        assert!(p.mouse(MouseEventKind::ScrollUp, 0, 0));
        assert_eq!(top_text(&p), "line 1");
        // Nothing to do at the top, or for other mouse events.
        assert!(!p.mouse(MouseEventKind::ScrollUp, 0, 0));
        assert!(!p.mouse(MouseEventKind::Moved, 0, 0));
        // The wheel never switches files at the end.
        press(&mut p, "G");
        assert!(!p.mouse(MouseEventKind::ScrollDown, 0, 0));
        assert_eq!(p.file, 0);
        // It scrolls the help screen while that is open, and not a typed prompt.
        press(&mut p, "gh");
        p.mouse(MouseEventKind::ScrollDown, 0, 0);
        assert_eq!(p.help.as_ref().unwrap().vp.top, 3);
        assert_eq!(top_text(&p), "line 1");
        press(&mut p, "q/abc");
        assert!(!p.mouse(MouseEventKind::ScrollDown, 0, 0));
        assert_eq!(top_text(&p), "line 1");
    }

    /// A document with one streamed file fed through a pipe.
    fn streamed() -> (Document, std::io::PipeWriter) {
        let (reader, writer) = std::io::pipe().unwrap();
        let stream = Stream::spawn(reader, "(standard input)".into(), false, false, true);
        let mut d = doc(&[]);
        d.files.push(File {
            name: "(standard input)".into(),
            path: None,
            url: None,
            content: Section::Plain,
            raw: Vec::new(),
            stream: Some(stream),
        });
        (d, writer)
    }

    /// Wait until the stream has `n` lines, then pull them into the pager.
    fn pull(p: &mut Pager, n: usize) -> bool {
        p.doc
            .stream(0)
            .unwrap()
            .wait_for(n, Some(std::time::Duration::from_secs(5)));
        p.pull_stream()
    }

    #[test]
    fn streamed_input_is_shown_as_it_arrives() {
        use std::io::Write as _;
        let (d, mut w) = streamed();
        let mut p = Pager::new(d, &OPTS, 40, 5);
        assert!(p.reading());
        assert!(p.status().contains("(waiting for input)"));
        w.write_all(b"one\ntwo\n").unwrap();
        assert!(pull(&mut p, 2));
        assert_eq!(top_text(&p), "one");
        assert_eq!(p.view.wrapped.rows.len(), 2);
        for i in 3..=10 {
            writeln!(w, "line {i}").unwrap();
        }
        pull(&mut p, 10);
        // More than a screen now, and no percentage while the end is unknown.
        assert_eq!(p.status(), "--More--");
        press(&mut p, "=");
        assert_eq!(
            p.message.as_deref(),
            Some("(standard input), line 1 of 10+")
        );
        drop(w);
        p.doc.stream(0).unwrap().wait_for(usize::MAX, None);
        assert!(p.pull_stream(), "the end of input changes the status line");
        assert!(!p.reading());
        assert!(p.status().starts_with("--More--("));
    }

    #[test]
    fn input_that_ends_before_the_first_pull_is_still_pulled() {
        use std::io::Write as _;
        let (d, mut w) = streamed();
        for i in 1..=12 {
            writeln!(w, "line {i}").unwrap();
        }
        d.stream(0)
            .unwrap()
            .wait_for(12, Some(std::time::Duration::from_secs(5)));
        let mut p = Pager::new(d, &OPTS, 40, 5);
        assert_eq!(p.view.lines, 12);
        writeln!(w, "line 13").unwrap();
        drop(w);
        p.doc.stream(0).unwrap().wait_for(usize::MAX, None);
        // Not reading any more, but the last line and the end are still pending.
        assert!(!p.reading());
        assert!(p.pull_stream());
        assert_eq!(p.view.lines, 13);
        assert!(p.status().starts_with("--More--("));
        // Nothing left to pull: the loop can now wait for keys.
        assert!(!p.pull_stream());
    }

    #[test]
    fn follow_keeps_the_end_in_view() {
        use std::io::Write as _;
        let (d, mut w) = streamed();
        let mut p = Pager::new(d, &OPTS, 40, 5);
        for i in 1..=10 {
            writeln!(w, "line {i}").unwrap();
        }
        pull(&mut p, 10);
        press(&mut p, "F");
        assert!(p.status().starts_with("Following"));
        assert_eq!(top_text(&p), "line 7");
        for i in 11..=20 {
            writeln!(w, "line {i}").unwrap();
        }
        pull(&mut p, 20);
        assert_eq!(top_text(&p), "line 17");
        // Any key stops following.
        press(&mut p, "k");
        assert!(!p.follow);
        writeln!(w, "line 21").unwrap();
        pull(&mut p, 21);
        assert_eq!(top_text(&p), "line 16");
    }

    #[test]
    fn search_finds_streamed_lines() {
        use std::io::Write as _;
        let (d, mut w) = streamed();
        let mut p = Pager::new(d, &OPTS, 40, 5);
        w.write_all(b"a\nb\n").unwrap();
        pull(&mut p, 2);
        press(&mut p, "/needle\n");
        assert_eq!(p.message.as_deref(), Some("Pattern not found"));
        for _ in 0..10 {
            writeln!(w, "hay").unwrap();
        }
        writeln!(w, "a needle here").unwrap();
        pull(&mut p, 13);
        press(&mut p, "n");
        assert_eq!(top_text(&p), "hay");
        assert_eq!(p.cursor.map(|c| c.0), Some(12));
    }

    #[test]
    fn exit_at_eof_waits_for_the_input_to_end() {
        use std::io::Write as _;
        let opts = Options {
            exit_at_eof: true,
            ..OPTS
        };
        let (d, mut w) = streamed();
        let mut p = Pager::new(d, &opts, 40, 5);
        w.write_all(b"x\n").unwrap();
        pull(&mut p, 1);
        assert_eq!(press(&mut p, " "), Flow::Continue);
        drop(w);
        p.doc.stream(0).unwrap().wait_for(usize::MAX, None);
        p.pull_stream();
        assert_eq!(press(&mut p, " "), Flow::Quit);
    }

    #[test]
    fn ctrl_z_suspends() {
        let d = doc(&[("a.txt", &numbered(5))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        let ctrl_z = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(p.key(ctrl_z), Flow::Suspend);
        press(&mut p, "h");
        assert_eq!(p.key(ctrl_z), Flow::Suspend);
    }

    #[test]
    fn counts_go_to_lines_and_percentages() {
        let d = doc(&[("a.txt", &numbered(200))]);
        let mut p = Pager::new(d, &OPTS, 40, 11);
        press(&mut p, "120g");
        assert_eq!(top_text(&p), "line 120");
        press(&mut p, "3G");
        assert_eq!(top_text(&p), "line 3");
        press(&mut p, "50%");
        assert_eq!(top_text(&p), "line 101");
        press(&mut p, "0p");
        assert_eq!(top_text(&p), "line 1");
        press(&mut p, "5j");
        assert_eq!(top_text(&p), "line 6");
        press(&mut p, "2k");
        assert_eq!(top_text(&p), "line 4");
        // Past the end goes to the last screen; plain g and G still work.
        press(&mut p, "999g");
        assert!(p.view.vp.at_end());
        press(&mut p, "g");
        assert_eq!(top_text(&p), "line 1");
    }

    #[test]
    fn count_is_shown_while_typing_and_esc_cancels_it() {
        let d = doc(&[("a.txt", &numbered(50))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "12");
        assert!(plain_screen(&p).contains("12"));
        press(&mut p, "\x1b");
        assert!(p.count.is_none());
        press(&mut p, "j");
        assert_eq!(top_text(&p), "line 2");
    }

    #[test]
    fn chopped_lines_scroll_sideways() {
        let long = format!("start{}end", "-".repeat(60));
        let d = doc(&[("a.txt", &format!("{long}\nshort\n"))]);
        let opts = Options { chop: true, ..OPTS };
        let mut p = Pager::new(d, &opts, 20, 5);
        // One row per line, cut at the screen edge.
        assert_eq!(p.view.wrapped.rows.len(), 2);
        let s = plain_screen(&p);
        assert!(
            s.contains("start---------------") && !s.contains("end"),
            "{s}"
        );
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        for _ in 0..10 {
            p.key(right);
        }
        // Scrolled to the end of the longest line, and no further.
        assert_eq!(p.hscroll, 68 - 20);
        assert!(plain_screen(&p).contains("-end"));
        assert!(p.status().contains("[from column 49]"));
        press(&mut p, "w");
        assert_eq!(p.hscroll, 0);
        assert!(p.view.wrapped.rows.len() > 2);
        // Left/Right while wrapping explain how to cut lines off.
        p.key(right);
        assert!(p.message.as_deref().unwrap().contains("press w"));
    }

    #[test]
    fn search_scrolls_sideways_to_the_match() {
        let line = format!("{}needle", "x".repeat(100));
        let d = doc(&[("a.txt", &format!("{line}\n"))]);
        let opts = Options { chop: true, ..OPTS };
        let mut p = Pager::new(d, &opts, 40, 5);
        press(&mut p, "/needle\n");
        assert!(p.hscroll > 60 && p.hscroll <= 100, "hscroll {}", p.hscroll);
        assert!(screen(&p).contains("\x1b[7mneedle"));
    }

    #[test]
    fn filter_shows_only_matching_lines() {
        let text = "ok 1\nerror 2\nok 3\nERROR 4\nok 5\n";
        let mut d = doc(&[("a.log", text)]);
        d.line_numbers = true;
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "&error\n");
        let rows: Vec<String> = p.view.wrapped.rows.iter().map(Line::text).collect();
        // Smart case, like search; line numbers stay those of the file.
        assert_eq!(rows, vec!["error 2", "ERROR 4"]);
        assert!(plain_screen(&p).contains("  2 error 2"));
        assert!(p.status().contains("[&error: 2 lines]"));
        // Search works within the filtered lines.
        press(&mut p, "/4\n");
        assert_eq!(p.cursor.map(|c| c.0), Some(1));
        press(&mut p, "&\n");
        assert_eq!(p.view.wrapped.rows.len(), 5);
        assert_eq!(p.message.as_deref(), Some("Showing all lines"));
        press(&mut p, "&(\n");
        assert!(p.message.as_deref().unwrap().starts_with("Invalid pattern"));
    }

    #[test]
    fn filter_applies_to_streamed_input() {
        use std::io::Write as _;
        let (d, mut w) = streamed();
        let mut p = Pager::new(d, &OPTS, 40, 5);
        w.write_all(b"GET /a 200\nGET /b 500\n").unwrap();
        pull(&mut p, 2);
        press(&mut p, "&500\n");
        w.write_all(b"GET /c 200\nGET /d 500\n").unwrap();
        pull(&mut p, 4);
        let rows: Vec<String> = p.view.wrapped.rows.iter().map(Line::text).collect();
        assert_eq!(rows, vec!["GET /b 500", "GET /d 500"]);
        assert_eq!(p.view.wrapped.line, vec![0, 1]);
    }

    #[test]
    fn filtered_code_is_still_highlighted() {
        use crate::document::LazyHighlight;
        use std::sync::Arc;
        let text = "fn a() {}\nlet x = 1;\nfn b() {}\n";
        let syntax = assets::syntaxes().find_syntax_by_extension("rs").unwrap();
        let theme = assets::theme(assets::DEFAULT_DARK_THEME).unwrap();
        let lazy = Arc::new(LazyHighlight::new(text.into(), syntax, theme));
        lazy.run();
        let mut d = doc(&[]);
        d.files.push(File {
            name: "a.rs".into(),
            path: None,
            url: None,
            content: Section::Code(Arc::clone(&lazy)),
            raw: plain::render(text),
            stream: None,
        });
        let opts = Options {
            color: ColorMode::TrueColor,
            ..OPTS
        };
        let mut p = Pager::new(d, &opts, 40, 10);
        press(&mut p, "&let\n");
        // The one shown line is source line 2, highlighted as that line.
        assert_eq!(p.row(&p.view, 0).text(), "let x = 1;");
        assert!(p.row(&p.view, 0).spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn edit_targets_files_and_reload_replaces_content() {
        let mut d = doc(&[("a.txt", &numbered(30))]);
        d.files[0].path = Some("a.txt".into());
        let reload = |_: &Path| -> Option<File> {
            Some(File {
                name: "a.txt".into(),
                path: Some("a.txt".into()),
                url: None,
                content: Section::Plain,
                raw: plain::render("edited 1\nedited 2\n"),
                stream: None,
            })
        };
        let opts = Options {
            reload: Some(&reload),
            ..OPTS
        };
        let mut p = Pager::new(d, &opts, 40, 10);
        press(&mut p, "5g");
        assert_eq!(press(&mut p, "v"), Flow::Edit);
        assert_eq!(p.edit_target(), Ok(("a.txt".into(), 5)));
        p.reload();
        assert_eq!(top_text(&p), "edited 1");
    }

    #[test]
    fn piped_input_cannot_be_edited() {
        let (d, _w) = streamed();
        let p = Pager::new(d, &OPTS, 40, 5);
        assert_eq!(
            p.edit_target(),
            Err("(standard input) cannot be opened in an editor".into())
        );
    }

    fn sections(n: usize) -> String {
        (1..=n)
            .map(|i| format!("## Section {i}\n\n{}\n", "text\n\n".repeat(5)))
            .collect()
    }

    #[test]
    fn brackets_jump_between_headings() {
        let d = doc(&[("a.md", &sections(6))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        assert_eq!(top_text(&p), "Section 1");
        press(&mut p, "]");
        assert_eq!(top_text(&p), "Section 2");
        press(&mut p, "]]");
        assert_eq!(top_text(&p), "Section 4");
        press(&mut p, "[");
        assert_eq!(top_text(&p), "Section 3");
        // Near the end the screen cannot scroll, but ] still moves on.
        press(&mut p, "]]");
        press(&mut p, "]");
        assert_eq!(p.heading_cursor.map(|c| c.0), Some(5));
        press(&mut p, "]");
        assert_eq!(p.message.as_deref(), Some("No more headings"));
    }

    #[test]
    fn plain_files_have_no_headings() {
        let d = doc(&[("a.txt", "# not a heading\n")]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "]");
        assert!(p.message.as_deref().unwrap().starts_with("No headings"));
        press(&mut p, "t");
        assert!(p.toc.is_none());
    }

    #[test]
    fn table_of_contents_chooses_and_jumps() {
        let d = doc(&[("a.md", &format!("# Title\n\n{}", sections(30)))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "]]]");
        assert_eq!(top_text(&p), "Section 3");
        press(&mut p, "t");
        // Opens at the current section; indented by level; selection reversed.
        let toc = p.toc.as_ref().unwrap();
        assert_eq!(toc.selected, 3);
        let s = screen(&p);
        assert!(s.contains("\x1b[7m   Section 3 \x1b[0m"), "{s}");
        assert!(s.contains("Contents: j/k"));
        // Scrolls to keep the selection visible.
        press(&mut p, "G");
        assert_eq!(p.toc.as_ref().unwrap().selected, 30);
        assert!(p.toc.as_ref().unwrap().top > 0);
        press(&mut p, "kk\n");
        assert!(p.toc.is_none());
        assert_eq!(p.heading_cursor.map(|c| c.0), Some(28));
        // q closes the contents without quitting.
        press(&mut p, "t");
        assert_eq!(press(&mut p, "q"), Flow::Continue);
        assert!(p.toc.is_none());
    }

    /// A file on disk, and a reload function that reads it as plain text.
    fn on_disk(name: &str, text: &str) -> (std::path::PathBuf, Document) {
        let dir = std::env::temp_dir().join(format!("mmore-watch-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        let mut d = doc(&[(name, text)]);
        d.files[0].path = Some(path.clone());
        (path, d)
    }

    fn read_plain(path: &Path) -> Option<File> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(File {
            name: path.display().to_string(),
            path: Some(path.to_path_buf()),
            url: None,
            content: Section::Plain,
            raw: plain::render(&text),
            stream: None,
        })
    }

    /// Change a file's content and push its modification time forward, so the
    /// change is noticed even on file systems with coarse timestamps.
    fn rewrite(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    fn read_markdown(path: &Path) -> Option<File> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(File {
            name: path.display().to_string(),
            path: Some(path.to_path_buf()),
            url: None,
            content: Section::Markdown(text.clone()),
            raw: plain::render(&text),
            stream: None,
        })
    }

    /// Paragraphs "para 1" to "para n", one line each.
    fn paras(n: usize) -> String {
        (1..=n).map(|i| format!("para {i}\n\n")).collect()
    }

    #[test]
    fn links_are_selected_followed_and_left_again() {
        let dir = std::env::temp_dir().join(format!("mmore-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        let readme = format!(
            "# Top\n\n[guide](docs/guide%20one.md#second-part)\n\n[down](#later)\n\n\
             [web](https://example.com)\n\n[gone](missing.md)\n\n{0}## Later\n\n{0}",
            paras(20)
        );
        let guide = format!(
            "# Guide\n\n{0}## Second part\n\n[back](../README.md)\n\n{0}",
            paras(20)
        );
        std::fs::write(dir.join("README.md"), &readme).unwrap();
        std::fs::write(dir.join("docs/guide one.md"), &guide).unwrap();
        let mut d = doc(&[("README.md", &readme)]);
        d.files[0].path = Some(dir.join("README.md"));
        let opts = Options {
            reload: Some(&read_markdown),
            ..OPTS
        };
        let mut p = Pager::new(d, &opts, 60, 6);

        press(&mut p, "\t");
        let message = p.message.clone().unwrap();
        assert!(
            message.starts_with("Link: docs/guide%20one.md#second-part"),
            "{message}"
        );
        press(&mut p, "\t\t\t\t");
        assert_eq!(p.message.as_deref(), Some("No more links"));
        press(&mut p, "\u{2190}");
        assert!(
            p.message
                .as_deref()
                .unwrap()
                .starts_with("Link: https://example.com")
        );
        press(&mut p, "\n");
        assert_eq!(p.message.as_deref(), Some("Opening https://example.com"));

        // An anchor in the same file, and back.
        press(&mut p, "g\t\t");
        let at_link = p.view.vp.top;
        press(&mut p, "\n");
        assert!(top_text(&p).contains("Later"), "{}", top_text(&p));
        press(&mut p, "\x08");
        assert_eq!((p.file, p.view.vp.top), (0, at_link));
        press(&mut p, "g");

        // Another file, at a heading; its link back reuses the open file.
        press(&mut p, "\t\n");
        assert_eq!(p.file, 1);
        assert!(top_text(&p).contains("Second part"), "{}", top_text(&p));
        press(&mut p, "\t\n");
        assert_eq!((p.file, p.doc.files.len()), (0, 2));
        press(&mut p, "\x08");
        assert_eq!(p.file, 1);
        press(&mut p, "\x08");
        assert_eq!((p.file, p.view.vp.top), (0, 0));
        press(&mut p, "\x08");
        assert_eq!(p.message.as_deref(), Some("No link to go back from"));

        // A missing file; and Enter without a selected link scrolls as usual.
        press(&mut p, "\t\t\t\t\n");
        assert!(
            p.message.as_deref().unwrap().starts_with("Cannot open"),
            "{:?}",
            p.message
        );
        assert_eq!(p.file, 0);
        let top = p.view.vp.top;
        press(&mut p, "\n");
        assert_eq!(p.view.vp.top, top + 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn links_in_web_documents_are_fetched_relative_to_them() {
        let text = "[next](b.md)\n\n[local](file:///etc/hosts)\n";
        let mut d = doc(&[("https://h.io/docs/a.md", text)]);
        d.files[0].url = Some("https://h.io/docs/a.md".into());
        let fetched = std::cell::RefCell::new(Vec::new());
        let fetch = |url: &str| -> Result<File, String> {
            fetched.borrow_mut().push(url.to_string());
            let mut file = doc(&[(url, "# Fetched\n")]).files.remove(0);
            file.url = Some(url.to_string());
            Ok(file)
        };
        let opts = Options {
            fetch: Some(&fetch),
            ..OPTS
        };
        let mut p = Pager::new(d, &opts, 40, 10);
        press(&mut p, "\t\n");
        assert_eq!(p.doc.files[p.file].name, "https://h.io/docs/b.md");
        press(&mut p, "\x08\t\n");
        assert_eq!(
            *fetched.borrow(),
            vec!["https://h.io/docs/b.md"],
            "fetched once"
        );
        press(&mut p, "\x08\t\t\n");
        assert_eq!(p.file, 0);
        assert_eq!(
            p.message.as_deref(),
            Some("Cannot open file:///etc/hosts from a web page")
        );
    }

    #[test]
    fn clicking_a_link_follows_it() {
        let text = format!(
            "# Top\n\nGo [down](#later) now.\n\n{}## Later\n\n{}",
            paras(20),
            paras(10)
        );
        let mut p = Pager::new(doc(&[("a.md", &text)]), &OPTS, 40, 8);
        // Row 3 is "Go down now."; "down" is at columns 3 to 6.
        assert_eq!(top_text(&p), "Top");
        assert!(
            !p.mouse(MouseEventKind::Down(MouseButton::Left), 1, 3),
            "not on the link"
        );
        assert!(p.mouse(MouseEventKind::Down(MouseButton::Left), 4, 3));
        assert!(top_text(&p).contains("Later"), "{}", top_text(&p));
        press(&mut p, "\x08");
        assert_eq!(top_text(&p), "Top");
        // A click below the text, or on the status line, does nothing.
        assert!(!p.mouse(MouseEventKind::Down(MouseButton::Left), 4, 7));
    }

    #[test]
    fn links_and_headings_work_in_the_raw_view() {
        let text = format!(
            "# Top\n\nGo [down](#later) now.\n\n{}## Later\n\n{}",
            paras(20),
            paras(10)
        );
        let mut p = Pager::new(doc(&[("a.md", &text)]), &OPTS, 40, 8);
        press(&mut p, "r");
        assert_eq!(top_text(&p), "# Top");
        press(&mut p, "\t");
        assert!(p.message.as_deref().unwrap().starts_with("Link: #later"));
        press(&mut p, "\n");
        assert_eq!(top_text(&p), "## Later");
        press(&mut p, "g]");
        assert_eq!(
            top_text(&p),
            "## Later",
            "] uses the headings in the raw view too"
        );
    }

    #[test]
    fn watch_reloads_when_the_file_changes() {
        let (path, d) = on_disk("doc.txt", &numbered(30));
        let reload = read_plain;
        let opts = Options {
            watch: true,
            reload: Some(&reload),
            ..OPTS
        };
        let mut p = Pager::new(d, &opts, 40, 10);
        press(&mut p, "5g");
        p.watched_at -= WATCH_POLL;
        assert!(!p.check_watched(), "nothing changed yet");
        rewrite(&path, &numbered(40));
        p.watched_at -= WATCH_POLL;
        assert!(p.check_watched());
        assert_eq!(top_text(&p), "line 5");
        assert_eq!(p.doc.files[0].raw.len(), 40);
        assert!(p.status().contains("[watching]"));
        // At the end, a growing file keeps its end in view.
        press(&mut p, "G");
        rewrite(&path, &numbered(50));
        p.watched_at -= WATCH_POLL;
        p.check_watched();
        assert_eq!(top_text(&p), "line 42");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn capital_r_reloads() {
        let (path, d) = on_disk("r.txt", "old\n");
        let reload = read_plain;
        let opts = Options {
            reload: Some(&reload),
            ..OPTS
        };
        let mut p = Pager::new(d, &opts, 40, 10);
        std::fs::write(&path, "new\n").unwrap();
        press(&mut p, "R");
        assert_eq!(top_text(&p), "new");
        assert!(p.message.as_deref().unwrap().starts_with("Reloaded"));
        // Without --watch nothing is checked.
        assert!(!p.check_watched());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn help_and_contents_name_the_keys_that_work() {
        let text = sections(3);
        let mut p = Pager::new(doc(&[("a.md", &text)]), &OPTS, 80, 70);
        press(&mut p, "h");
        assert!(p.status().ends_with("q to close"), "{}", p.status());
        assert!(screen(&p).contains("Press q or h to return."));
        press(&mut p, "qt");
        assert!(screen(&p).contains("Contents: j/k to choose, Enter to go there, q to close"));

        let mut keys = keys::Keymap::new();
        keys.bind("q", "nothing").unwrap();
        keys.bind("j", "nothing").unwrap();
        keys.bind("x", "quit").unwrap();
        let opts = Options {
            keymap: &keys,
            ..OPTS
        };
        let mut p = Pager::new(doc(&[("a.md", &text)]), &opts, 80, 70);
        press(&mut p, "h");
        assert!(p.status().ends_with("h to close"), "{}", p.status());
        assert!(screen(&p).contains("Press h or Esc to return."));
        press(&mut p, "ht");
        assert!(
            screen(&p).contains("Contents: Down/k to choose, Enter to go there, t to close"),
            "{}",
            screen(&p)
        );
    }

    fn ctrl(p: &mut Pager, c: char) {
        p.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    #[test]
    fn search_modifiers() {
        let text = numbered(30);
        // !pattern: lines that do not match.
        let mut p = Pager::new(doc(&[("a.txt", &text)]), &OPTS, 40, 5);
        press(&mut p, "/!line 1\n");
        assert_eq!(top_text(&p), "line 2");
        press(&mut p, "nnnnnnnn");
        assert_eq!(top_text(&p), "line 20", "line 10 to 19 match 'line 1'");
        press(&mut p, "g&!line 1\n");
        assert!(
            p.status().contains("[&!line 1: 19 lines]"),
            "{}",
            p.status()
        );

        // Ctrl-K: highlight without moving; n goes to the match afterwards.
        let mut p = Pager::new(doc(&[("a.txt", &text)]), &OPTS, 40, 5);
        press(&mut p, "/");
        ctrl(&mut p, 'k');
        press(&mut p, "line 20");
        assert!(screen(&p).contains("Keep-pos /line 20"));
        assert_eq!(top_text(&p), "line 1", "no move while typing either");
        press(&mut p, "\n");
        assert_eq!(top_text(&p), "line 1");
        assert!(p.view.matches.starts[19]);
        press(&mut p, "n");
        assert_eq!(top_text(&p), "line 20");

        // Ctrl-R: plain text, not a regular expression (also for &).
        let dots = "a.b\naxb\na.b again\n";
        let mut p = Pager::new(doc(&[("a.txt", dots)]), &OPTS, 40, 10);
        press(&mut p, "/");
        ctrl(&mut p, 'r');
        assert!(screen(&p).contains("Regex-off /"));
        press(&mut p, "a.b\n");
        let found = p.view.matches.starts.iter().filter(|s| **s).count();
        assert_eq!(found, 2, "axb does not match");
        // An empty pattern repeats the search the same way.
        press(&mut p, "g/\n");
        assert_eq!(p.view.matches.starts.iter().filter(|s| **s).count(), 2);
        press(&mut p, "&");
        ctrl(&mut p, 'r');
        press(&mut p, "a.b\n");
        assert!(p.status().contains("2 lines"), "{}", p.status());
        // The modifiers do not carry over to the next prompt.
        press(&mut p, "&\n/a.b\n");
        assert_eq!(p.view.matches.starts.iter().filter(|s| **s).count(), 3);
    }

    #[test]
    fn inverted_search_finds_blank_lines() {
        let mut p = Pager::new(doc(&[("a.txt", "x\nx\n\nx\n")]), &OPTS, 40, 10);
        press(&mut p, "/!x\n");
        assert!(p.view.matches.starts[2]);
        assert_eq!(p.view.matches.starts.iter().filter(|s| **s).count(), 1);
    }

    #[test]
    fn squeeze_shows_blank_runs_as_one_line() {
        let mut d = doc(&[("a.txt", "a\n\n\n\nb\n\n\nc\n")]);
        d.squeeze = true;
        d.line_numbers = true;
        let p = Pager::new(d, &OPTS, 40, 10);
        let rows: Vec<String> = p.view.wrapped.rows.iter().map(Line::text).collect();
        assert_eq!(rows, ["a", "", "b", "", "c"]);
        // Line numbers stay those of the file.
        assert!(plain_screen(&p).contains("  5 b"));

        // Piped input arriving in pieces is squeezed across the pieces.
        let (mut d, mut writer) = streamed();
        d.squeeze = true;
        let mut p = Pager::new(d, &OPTS, 40, 10);
        use std::io::Write;
        writer.write_all(b"x\n\n").unwrap();
        pull(&mut p, 2);
        writer.write_all(b"\n\ny\n").unwrap();
        drop(writer);
        pull(&mut p, 5);
        let rows: Vec<String> = p.view.wrapped.rows.iter().map(Line::text).collect();
        assert_eq!(rows, ["x", "", "y"]);
    }

    #[test]
    fn screen_text_is_the_content_rows_shown() {
        let mut p = Pager::new(doc(&[("a.txt", &numbered(100))]), &OPTS, 40, 5);
        press(&mut p, "jj/line 5\n");
        let text = p.screen_text();
        assert_eq!(text.len(), 4, "the status line is left out");
        assert_eq!(text[0], "\x1b[7mline 5\x1b[0m");
        assert_eq!(text[3], "line 8");
    }

    #[test]
    fn start_commands() {
        assert_eq!(Start::parse("120"), Ok(Start::Line(120)));
        assert_eq!(Start::parse("G"), Ok(Start::End));
        assert_eq!(
            Start::parse("/err or"),
            Ok(Start::Search {
                pattern: "err or".into(),
                backward: false
            })
        );
        assert!(matches!(
            Start::parse("?x"),
            Ok(Start::Search { backward: true, .. })
        ));
        assert!(Start::parse("/").is_err());
        assert!(Start::parse("q").unwrap_err().contains("+LINE"));

        let text = numbered(100);
        let at = |start: Start| {
            let mut p = Pager::new(doc(&[("a.txt", &text)]), &OPTS, 40, 10);
            p.start_at(&start);
            let top = top_text(&p);
            // '' goes back to the top.
            press(&mut p, "''");
            (top, top_text(&p))
        };
        assert_eq!(at(Start::Line(50)).0, "line 50");
        assert_eq!(at(Start::End).0, "line 92");
        let search = |pattern: &str, backward| Start::Search {
            pattern: pattern.into(),
            backward,
        };
        assert_eq!(
            at(search("line 7", false)),
            ("line 7".into(), "line 1".into())
        );
        // Backward from the end: the last match.
        assert_eq!(at(search("line 7", true)).0, "line 79");
        assert_eq!(at(search("nothing", true)).0, "line 92");
    }

    #[test]
    fn marks_keep_the_row_within_a_long_line() {
        // A line that wraps into five rows at width 10, between short lines.
        let text = format!("{}{}\n{}", numbered(3), "x".repeat(50), numbered(20));
        let mut p = Pager::new(doc(&[("a.txt", &text)]), &OPTS, 10, 4);
        press(&mut p, "jjjjj");
        assert_eq!(p.view.vp.top, 5, "two rows into the long line");
        press(&mut p, "ma");
        press(&mut p, "G'a");
        assert_eq!(p.view.vp.top, 5);
        press(&mut p, "''");
        assert_eq!(
            p.view.vp.top,
            p.view.vp.max_top(),
            "'' goes back to the end"
        );
        press(&mut p, "''");
        assert_eq!(p.view.vp.top, 5);
    }

    #[test]
    fn marks_and_going_back() {
        let d = doc(&[("a.txt", &numbered(100)), ("b.txt", &numbered(100))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "20gma");
        assert_eq!(p.message.as_deref(), Some("Mark a set"));
        press(&mut p, "70g");
        press(&mut p, "'a");
        assert_eq!(top_text(&p), "line 20");
        // '' goes back to before the last jump, and again goes forward.
        press(&mut p, "''");
        assert_eq!(top_text(&p), "line 70");
        press(&mut p, "''");
        assert_eq!(top_text(&p), "line 20");
        // A mark can be in another file.
        press(&mut p, ":n");
        press(&mut p, "5g'a");
        assert_eq!(p.file, 0);
        assert_eq!(top_text(&p), "line 20");
        // Search and G count as jumps too.
        press(&mut p, "/line 50\n''");
        assert_eq!(top_text(&p), "line 20");
        press(&mut p, "G''");
        assert_eq!(top_text(&p), "line 20");
        press(&mut p, "'z");
        assert_eq!(p.message.as_deref(), Some("No mark z"));
        press(&mut p, "m1");
        assert!(
            p.message
                .as_deref()
                .unwrap()
                .starts_with("Marks are letters")
        );
        // The mark prompt shows what it waits for.
        press(&mut p, "m");
        assert!(plain_screen(&p).contains("m"));
    }

    #[test]
    fn search_as_you_type_moves_and_esc_goes_back() {
        let d = doc(&[("a.txt", &numbered(100))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "20g/line 6");
        assert_eq!(top_text(&p), "line 60");
        assert!(screen(&p).contains("\x1b[7mline 6\x1b[0m"));
        press(&mut p, "5");
        assert_eq!(top_text(&p), "line 65");
        // Backspace goes back to the earlier match.
        press(&mut p, "\x08");
        assert_eq!(top_text(&p), "line 60");
        // Esc returns to where the search started, without highlights.
        press(&mut p, "\x1b");
        assert_eq!(top_text(&p), "line 20");
        assert!(p.view.matches.ranges.iter().all(Vec::is_empty));
        assert!(p.search.is_none());
    }

    #[test]
    fn enter_commits_the_previewed_search() {
        let d = doc(&[("a.txt", &numbered(100))]);
        let mut p = Pager::new(d, &OPTS, 40, 10);
        press(&mut p, "/line 3\n");
        assert_eq!(top_text(&p), "line 3");
        press(&mut p, "n");
        assert_eq!(top_text(&p), "line 30");
        // A new search that is cancelled keeps the old one and its highlights.
        press(&mut p, "/line 9\x1b");
        assert_eq!(top_text(&p), "line 30");
        assert_eq!(
            p.search.as_ref().map(|s| s.pattern.as_str()),
            Some("line 3")
        );
        assert!(screen(&p).contains("\x1b[7mline 3\x1b[0m"));
        // Not found while typing: stays put.
        press(&mut p, "/nothing");
        assert_eq!(top_text(&p), "line 30");
        // An unfinished pattern keeps the last preview.
        press(&mut p, "\x1b/line 7(");
        assert_eq!(top_text(&p), "line 70");
        // Backward search previews backward.
        press(&mut p, "\x1b?line 2");
        assert_eq!(top_text(&p), "line 29");
    }

    #[test]
    fn status_line_fits_and_prompt_shows_tail() {
        assert_eq!(head("--More--(25%)", 8), "--More--");
        assert_eq!(head("日本語", 5), "日本");
        assert_eq!(tail("/abcdef", 3), "def");
        assert_eq!(tail("ok", 10), "ok");
    }
}
