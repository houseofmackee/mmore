//! Entering and leaving the interactive terminal mode safely.

use std::io::{self, Write};
use std::sync::Once;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::{cursor, execute, terminal};

/// Mouse button reporting (1000) in SGR encoding (1006). Wheel turns arrive as
/// buttons 64 and 65. Motion tracking stays off, so moving the mouse sends nothing.
const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1000l";

/// Whether this process turned mouse reporting on (and must turn it off again).
static MOUSE_ENABLED: AtomicBool = AtomicBool::new(false);

/// The terminal size before the alternate screen, as columns and rows.
static SIZE_BEFORE: std::sync::Mutex<Option<(u16, u16)>> = std::sync::Mutex::new(None);

/// Longest to wait, after leaving the alternate screen, for the terminal to give
/// back its earlier size.
const SIZE_BACK_WAIT: Duration = Duration::from_millis(300);

/// Puts the terminal into raw mode on the alternate screen (optionally with mouse
/// reporting). Restores it on drop, and a panic hook restores it if the program
/// panics while paging.
pub struct Screen {
    mouse: bool,
}

impl Screen {
    pub fn enter(mouse: bool) -> io::Result<Self> {
        install_panic_hook();
        // Construct the guard before anything else can fail, so Drop always runs.
        let screen = Screen { mouse };
        screen.setup()?;
        Ok(screen)
    }

    fn setup(&self) -> io::Result<()> {
        // Back to the settings from before a slow background query, so raw mode
        // and leaving it start from those; a late answer is read as input now.
        crate::background::end_quiet(false);
        *SIZE_BEFORE.lock().unwrap_or_else(PoisonError::into_inner) = terminal::size().ok();
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
        if self.mouse {
            MOUSE_ENABLED.store(true, Ordering::SeqCst);
            out.write_all(MOUSE_ON.as_bytes())?;
            out.flush()?;
        }
        Ok(())
    }

    /// Give the terminal back while `run` runs (an editor), then take it again.
    pub fn outside<T>(&self, run: impl FnOnce() -> T) -> io::Result<T> {
        restore();
        let result = run();
        self.setup()?;
        Ok(result)
    }

    /// Ctrl-Z: give the terminal back and stop, like any job; continue after `fg`.
    /// Raw mode turns off the terminal's own Ctrl-Z handling, so this does it: it
    /// stops the whole process group, as the terminal would.
    pub fn suspend(&self) -> io::Result<()> {
        restore();
        #[cfg(unix)]
        // SAFETY: kill with pid 0 signals our own process group; no memory involved.
        unsafe {
            libc::kill(0, libc::SIGTSTP);
        }
        // Running again (or the stop was ignored, without job control).
        self.setup()
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
    // Back at the start of the line. Terminals put the cursor back where it was
    // before the alternate screen, which is there already; one that does not
    // would leave it mid-line, and zsh would then show its `%` marker.
    let _ = out.write_all(b"\r");
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
    wait_for_size_back();
}

/// Some terminals (Warp) give full-screen programs a different size and switch
/// back when they leave the alternate screen, a moment later. A shell that looks
/// at the size before then gets the full-screen width, and zsh's `%` mark for
/// output without a newline is then left on the screen. So if the size changed,
/// wait until it changes again (or for `SIZE_BACK_WAIT`) before going on. When
/// it did not change, there is nothing to wait for.
fn wait_for_size_back() {
    let before = SIZE_BEFORE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let Some(before) = before else {
        return;
    };
    let Ok(during) = terminal::size() else {
        return;
    };
    if during == before {
        return;
    }
    let deadline = Instant::now() + SIZE_BACK_WAIT;
    while Instant::now() < deadline {
        match terminal::size() {
            Ok(now) if now != during => return,
            Ok(_) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => return,
        }
    }
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
