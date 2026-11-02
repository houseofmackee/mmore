//! Entering and leaving the interactive terminal mode safely.

use std::io::{self, Write};
use std::sync::Once;

use crossterm::{cursor, execute, terminal};

/// Puts the terminal into raw mode on the alternate screen. Restores it on drop,
/// and a panic hook restores it if the program panics while paging.
pub struct Screen;

impl Screen {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        // Construct the guard before anything else can fail, so Drop always runs.
        let screen = Screen;
        execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
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
