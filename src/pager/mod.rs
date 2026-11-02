//! Interactive pager.
//!
//! `Pager` holds all state and handles keys; `run` connects it to the terminal.
//! Files are shown one at a time (`:n` and `:p` switch), each remembering its
//! scroll position.

mod editor;
mod help;
mod keys;
mod screen;
mod search;
mod viewport;

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::cursor::{self, MoveTo};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ansi::{self, ColorMode};
use crate::document::{Document, File, LazyHighlight, gutter_span};
use crate::render::Rendered;
use crate::stream::Stream;
use crate::style::Line;
use crate::wrap::{Wrapped, column_of, slice_columns, wrap_all, wrap_line};
use keys::Action;
use screen::Screen;
use search::{RowMatches, Search};
use viewport::Viewport;

/// Loads a file again after it was edited (see `v`).
pub type Reload<'a> = &'a dyn Fn(&Path) -> Option<File>;

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
    /// How to load a file again after editing it.
    pub reload: Option<Reload<'a>>,
}

/// Page the document interactively until the user quits.
pub fn run(document: Document, opts: &Options) -> io::Result<()> {
    if document.files.is_empty() {
        return Ok(());
    }
    document.start_highlighting();
    let screen = Screen::enter(opts.mouse)?;
    let mut out = BufWriter::new(io::stdout());
    let (cols, rows) = terminal::size()?;
    let mut pager = Pager::new(document, opts, cols, rows);
    pager.wait_for_first_screen(FIRST_SCREEN_WAIT);
    // Keys pressed while the terminal was asked about its background come first.
    for key in keys::from_bytes(&crate::background::take_typeahead()) {
        match pager.key(key) {
            Flow::Quit => return Ok(()),
            // Suspending or editing before the first screen would be odd.
            Flow::Suspend | Flow::Edit | Flow::Continue => {}
        }
    }
    let mut dirty = true;
    // Highlighted lines available at the last draw.
    let mut drawn = 0;
    loop {
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
        // While highlighting runs or input arrives, wake up now and then.
        let event = if pager.highlighting() || pager.reading() {
            event::poll(HIGHLIGHT_POLL)?.then(event::read).transpose()?
        } else {
            Some(event::read()?)
        };
        match event {
            None => {
                let highlighted = pager.newly_highlighted_on_screen(drawn);
                dirty = pager.pull_stream() || highlighted;
            }
            Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
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
            Some(Event::Mouse(mouse)) => dirty = pager.mouse(mouse.kind),
            Some(Event::Resize(..)) => {
                // Query again: the size in the event can be stale during a drag.
                let (cols, rows) = terminal::size()?;
                pager.resize(cols, rows);
                dirty = true;
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// How long to wait for the first screen to be highlighted before showing it
/// plain. Small files are done by then; big files do not keep you waiting.
const FIRST_SCREEN_WAIT: Duration = Duration::from_millis(50);
/// Rows scrolled per wheel notch.
const WHEEL_ROWS: isize = 3;
/// How often to check for newly highlighted lines while highlighting runs.
const HIGHLIGHT_POLL: Duration = Duration::from_millis(25);

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
    Search { backward: bool },
    Command,
    Filter,
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
    matches: RowMatches,
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
}

impl<'a> Pager<'a> {
    fn new(doc: Document, opts: &'a Options<'a>, cols: u16, rows: u16) -> Self {
        let files = doc.files.len();
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
        let rendered = self.doc.render_file(self.file, width, self.raw);
        let lines = rendered.lines.len();
        let rendered = match &self.filter {
            Some(filter) => keep_matching(rendered, filter),
            None => rendered,
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
            matches,
        };
        if self.follow {
            self.view.vp.top = self.view.vp.max_top();
        }
        self.stream_done = self.stream().is_none_or(Stream::is_done);
        self.cursor = None;
    }

    fn layout_help(&mut self) {
        let mut rendered = Rendered::default();
        for (i, line) in help::lines().into_iter().enumerate() {
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
            matches: RowMatches::default(),
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
        let action = keys::map(key);

        if let Some(help) = &mut self.help {
            match action {
                Action::Suspend => return Flow::Suspend,
                Action::Quit | Action::Help | Action::ClearSearch => self.help = None,
                _ => help.vp.apply(action),
            }
            return Flow::Continue;
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
                self.go_to_line(line);
                return Flow::Continue;
            }
            (Action::Percent, count) => {
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
            Action::SearchForward => {
                self.prompt = Some((PromptKind::Search { backward: false }, String::new()));
            }
            Action::SearchBackward => {
                self.prompt = Some((PromptKind::Search { backward: true }, String::new()));
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
            Action::Filter => self.prompt = Some((PromptKind::Filter, String::new())),
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
                self.view.vp.apply(movement);
            }
        }
        Flow::Continue
    }

    /// Scroll on mouse wheel turns. Returns whether the screen needs a redraw.
    fn mouse(&mut self, kind: MouseEventKind) -> bool {
        let delta = match kind {
            MouseEventKind::ScrollDown => WHEEL_ROWS,
            MouseEventKind::ScrollUp => -WHEEL_ROWS,
            _ => return false,
        };
        // Leave a prompt being typed alone.
        if self.prompt.is_some() {
            return false;
        }
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
            KeyCode::Esc => {}
            KeyCode::Char('c' | 'g') if ctrl => {}
            KeyCode::Char('u') if ctrl => self.prompt = Some((kind, String::new())),
            KeyCode::Backspace => {
                // Backspace on an empty prompt cancels it, as in less.
                if input.pop().is_some() {
                    self.prompt = Some((kind, input));
                }
            }
            KeyCode::Enter => match kind {
                PromptKind::Search { backward } => self.start_search(&input, backward),
                PromptKind::Filter => self.set_filter(&input),
                PromptKind::Command => {}
            },
            KeyCode::Char(c) if !ctrl => match kind {
                PromptKind::Command => return self.command(c),
                PromptKind::Search { .. } | PromptKind::Filter => {
                    input.push(c);
                    self.prompt = Some((kind, input));
                }
            },
            _ => self.prompt = Some((kind, input)),
        }
        Flow::Continue
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

    /// Go to source line `line` (counting from 1), as `120g` does.
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
        match Search::new(input, false) {
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
            Some(path) if file.stream.is_none() => Ok((path.clone(), self.view.anchor() + 1)),
            _ => Err(format!("{} cannot be opened in an editor", file.name)),
        }
    }

    /// Load the current file again (after editing), keeping the place.
    fn reload(&mut self) {
        let Some(reload) = self.opts.reload else {
            return;
        };
        let Some(path) = self.doc.files[self.file].path.clone() else {
            return;
        };
        match reload(&path) {
            Some(file) => {
                self.doc.files[self.file] = file;
                self.doc.start_highlighting_file(self.file);
                let anchor = self.view.anchor();
                self.layout(anchor);
            }
            None => {
                self.message = Some(format!("Could not read {} again", path.display()));
            }
        }
    }

    fn start_search(&mut self, input: &str, backward: bool) {
        // An empty pattern repeats the previous one, as in less.
        let pattern = match (input.is_empty(), &self.search) {
            (true, Some(previous)) => previous.pattern.clone(),
            (true, None) => return,
            (false, _) => input.to_string(),
        };
        match Search::new(&pattern, backward) {
            Ok(search) => {
                self.view.matches = search.row_matches(&self.view.wrapped);
                self.search = Some(search);
                self.cursor = None;
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
            None => self.message = Some("Pattern not found".into()),
        }
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
            return format!("Help {at}  q to close");
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
        status
    }

    fn draw(&self, out: &mut impl Write) -> io::Result<()> {
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
            let row = self.row(view, r);
            let mut line = Line::default();
            if view.gutter > 0 {
                let source = view.wrapped.source[r];
                let first = r == 0 || view.wrapped.source[r - 1] != source;
                line.spans
                    .push(gutter_span(first.then_some(source + 1), view.gutter));
            }
            let ranges = view.matches.ranges.get(r).map_or(&[][..], Vec::as_slice);
            let content = search::highlight(&row, ranges);
            if self.chop && self.help.is_none() {
                let width = self.content_width(view.gutter);
                line.append(slice_columns(&content, self.hscroll, width));
            } else {
                line.append(content);
            }
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
                };
                Some(format!("{prefix}{input}"))
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

/// Only the rendered lines that match `filter`, keeping their source lines.
fn keep_matching(rendered: Rendered, filter: &Search) -> Rendered {
    let mut out = Rendered {
        links: rendered.links,
        ..Rendered::default()
    };
    for (line, source) in rendered.lines.into_iter().zip(rendered.source) {
        if filter.is_match(&line.text()) {
            out.push(line, source);
        }
    }
    out
}

fn empty_view() -> View {
    View {
        wrapped: Wrapped::default(),
        vp: Viewport::new(0, 1),
        gutter: 0,
        width: 1,
        lines: 0,
        matches: RowMatches::default(),
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
        }
    }

    const OPTS: Options<'static> = Options {
        exit_at_eof: false,
        color: ColorMode::None,
        hyperlinks: false,
        mouse: false,
        chop: false,
        reload: None,
    };

    fn press(p: &mut Pager, keys: &str) -> Flow {
        let mut flow = Flow::Continue;
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\x1b' => KeyCode::Esc,
                '\x08' => KeyCode::Backspace,
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
        press(&mut p, "m");
        assert_eq!(top_text(&p), "# Title");
        // The confirmation message shows first; the status line marks the mode.
        assert!(screen(&p).contains("Showing raw source"));
        assert!(p.status().contains("[source]"));
        press(&mut p, "dm");
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
        // Content wraps at the width minus the 4-column gutter.
        assert_eq!(rows[1], "this line is lon");
        let s = plain_screen(&p);
        assert!(s.contains("  1 short"));
        assert!(s.contains("  2 this line is lon"));
        assert!(s.contains("    g enough to wrap"));
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
        // The wrapped second line keeps its text across rows.
        assert!(plain_screen(&p).contains("    let wrapped_long"));
        assert!(plain_screen(&p).contains("_identifier_name = 1"));
        // The raw view shows no highlighting.
        let mut p = p;
        press(&mut p, "m");
        assert!(!screen(&p).contains("38;2;"));
    }

    #[test]
    fn mouse_wheel_scrolls() {
        let d = doc(&[("a.txt", &numbered(50)), ("b.txt", "x\n")]);
        let mut p = Pager::new(d, &OPTS, 80, 10);
        assert!(p.mouse(MouseEventKind::ScrollDown));
        assert_eq!(top_text(&p), "line 4");
        assert!(p.mouse(MouseEventKind::ScrollUp));
        assert_eq!(top_text(&p), "line 1");
        // Nothing to do at the top, or for other mouse events.
        assert!(!p.mouse(MouseEventKind::ScrollUp));
        assert!(!p.mouse(MouseEventKind::Moved));
        // The wheel never switches files at the end.
        press(&mut p, "G");
        assert!(!p.mouse(MouseEventKind::ScrollDown));
        assert_eq!(p.file, 0);
        // It scrolls the help screen while that is open, and not a typed prompt.
        press(&mut p, "gh");
        p.mouse(MouseEventKind::ScrollDown);
        assert_eq!(p.help.as_ref().unwrap().vp.top, 3);
        assert_eq!(top_text(&p), "line 1");
        press(&mut p, "q/abc");
        assert!(!p.mouse(MouseEventKind::ScrollDown));
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

    #[test]
    fn status_line_fits_and_prompt_shows_tail() {
        assert_eq!(head("--More--(25%)", 8), "--More--");
        assert_eq!(head("日本語", 5), "日本");
        assert_eq!(tail("/abcdef", 3), "def");
        assert_eq!(tail("ok", 10), "ok");
    }
}
