//! Entering and leaving the interactive terminal mode safely.

use std::io::{self, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::{cursor, execute, terminal};

/// Mouse button reporting (1000) in SGR encoding (1006). Wheel turns arrive as
/// buttons 64 and 65. Motion tracking stays off, so moving the mouse sends nothing.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1000l";

/// Whether this process turned mouse reporting on (and must turn it off again).
static MOUSE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Puts the terminal into raw mode on the alternate screen (optionally with mouse
/// reporting). Restores it on drop, and a panic hook restores it if the program
/// panics while paging.
pub struct Screen;

impl Screen {
    pub fn enter(mouse: bool) -> io::Result<Self> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        // Construct the guard before anything else can fail, so Drop always runs.
        let screen = Screen;
        let mut out = io::stdout();
        execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
        if mouse {
            MOUSE_ENABLED.store(true, Ordering::SeqCst);
            out.write_all(MOUSE_ON.as_bytes())?;
            out.flush()?;
        }
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let mut out = io::stdout();
    if MOUSE_ENABLED.swap(false, Ordering::SeqCst) {
        let _ = out.write_all(MOUSE_OFF.as_bytes());
    }
    let _ = execute!(out, cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Leave raw mode first so the panic message is readable.
            restore();
            previous(info);
        }));
    });
}
