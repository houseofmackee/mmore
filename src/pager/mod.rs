//! Interactive pager.

mod keys;
mod screen;
mod viewport;

use std::io::{self, BufWriter, Write};

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType};
use unicode_width::UnicodeWidthStr;

use crate::wrap::{Wrapped, wrap_all};
use keys::Action;
use screen::Screen;
use viewport::Viewport;

pub struct Options {
    /// Quit when moving forward while the last page is already shown (classic `more`).
    pub exit_at_eof: bool,
}

/// Page `lines` interactively until the user quits.
pub fn run(lines: &[String], opts: &Options) -> io::Result<()> {
    let _screen = Screen::enter()?;
    let mut out = BufWriter::new(io::stdout());

    let (mut cols, rows) = terminal::size()?;
    let mut wrapped = wrap_all(lines, cols as usize);
    let mut vp = Viewport::new(wrapped.rows.len(), content_height(rows));

    loop {
        draw(&mut out, &wrapped, &vp, cols)?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let action = keys::map(key);
                if action == Action::Quit
                    || (opts.exit_at_eof && vp.at_end() && action.moves_forward())
                {
                    break;
                }
                vp.apply(action);
            }
            Event::Resize(..) => {
                // Query again: the size in the event can be stale during a drag.
                let (new_cols, new_rows) = terminal::size()?;
                let anchor = wrapped.source.get(vp.top).copied().unwrap_or(0);
                if new_cols != cols {
                    cols = new_cols;
                    wrapped = wrap_all(lines, cols as usize);
                }
                let top = wrapped.first_row_of(anchor);
                vp.reset(wrapped.rows.len(), content_height(new_rows), top);
            }
            _ => {}
        }
    }
    Ok(())
}

fn content_height(rows: u16) -> usize {
    (rows as usize).saturating_sub(1).max(1)
}

fn draw(out: &mut impl Write, wrapped: &Wrapped, vp: &Viewport, cols: u16) -> io::Result<()> {
    for i in 0..vp.height {
        // Clear before printing: clearing after a full-width row would erase its last cell.
        queue!(out, MoveTo(0, i as u16), Clear(ClearType::CurrentLine))?;
        if let Some(row) = wrapped.rows.get(vp.top + i) {
            queue!(out, Print(row))?;
        }
    }

    let status = status_text(vp);
    // Stay short of the last column so the terminal never scrolls.
    let status = truncate(&status, (cols as usize).saturating_sub(1));
    queue!(
        out,
        MoveTo(0, vp.height as u16),
        Clear(ClearType::CurrentLine),
        SetAttribute(Attribute::Reverse),
        Print(status),
        SetAttribute(Attribute::Reset),
    )?;
    out.flush()
}

fn status_text(vp: &Viewport) -> String {
    if vp.at_end() {
        "(END)".to_string()
    } else {
        format!("--More--({}%)", vp.percent())
    }
}

fn truncate(s: &str, max_width: usize) -> &str {
    if s.width() <= max_width {
        return s;
    }
    let mut width = 0;
    for (i, ch) in s.char_indices() {
        width += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width > max_width {
            return &s[..i];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn status_shows_progress_then_end() {
        let mut vp = Viewport::new(40, 10);
        assert_eq!(status_text(&vp), "--More--(25%)");
        vp.apply(Action::Bottom);
        assert_eq!(status_text(&vp), "(END)");
    }

    #[test]
    fn truncate_respects_width() {
        assert_eq!(truncate("--More--(25%)", 8), "--More--");
        assert_eq!(truncate("日本語", 5), "日本");
        assert_eq!(truncate("ok", 10), "ok");
    }

    #[test]
    fn draw_shows_visible_rows_and_status() {
        let wrapped = wrap_all(&lines(30), 80);
        let mut vp = Viewport::new(wrapped.rows.len(), 10);
        vp.apply(Action::PageDown);
        let mut buf = Vec::new();
        draw(&mut buf, &wrapped, &vp, 80).unwrap();
        let screen = String::from_utf8(buf).unwrap();
        assert!(screen.contains("line 11"));
        assert!(screen.contains("line 20"));
        assert!(!screen.contains("line 10\u{1b}"));
        assert!(!screen.contains("line 21"));
        assert!(screen.contains("--More--(66%)"));
    }
}
