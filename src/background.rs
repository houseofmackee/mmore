//! Detecting whether the terminal has a light background.
//!
//! First `COLORFGBG` (set by some terminals), then an OSC 11 query: the terminal
//! is asked for its background color, followed by a Device Attributes (DA1)
//! request that practically every terminal answers. The DA1 reply marks the end,
//! so terminals that ignore OSC 11 cost one round trip instead of a full timeout.

use std::env;
use std::sync::Mutex;
use std::time::Duration;

/// How long to wait for the terminal to answer.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(100);

/// `Some(true)` for a light background, `Some(false)` for dark, `None` if unknown.
pub fn is_light() -> Option<bool> {
    if let Some(light) = colorfgbg_is_light(env::var("COLORFGBG").ok().as_deref()) {
        return Some(light);
    }
    if !query_allowed() {
        return None;
    }
    let reply = query::background(QUERY_TIMEOUT)?;
    let (answers, typed) = split_reply(&reply);
    TYPEAHEAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend_from_slice(&typed);
    parse_osc11(&answers).map(|(r, g, b)| luminance(r, g, b) > 0.5)
}

/// Keys pressed while the terminal was being asked about its background: they
/// were read along with its answer and must not be lost.
static TYPEAHEAD: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Take the keys that were typed during the background query (see `TYPEAHEAD`).
pub fn take_typeahead() -> Vec<u8> {
    std::mem::take(
        &mut *TYPEAHEAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Separate the terminal's answers (the OSC 11 reply and the DA1 reply) from
/// anything else that was read: keys the user pressed meanwhile.
pub fn split_reply(reply: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut answers = Vec::new();
    let mut typed = Vec::new();
    let mut i = 0;
    while i < reply.len() {
        let rest = &reply[i..];
        let end = if rest.starts_with(b"\x1b]11;") {
            // Ends with BEL or ST (ESC \).
            rest.iter()
                .position(|&b| b == 0x07)
                .map(|p| p + 1)
                .into_iter()
                .chain(rest.windows(2).position(|w| w == b"\x1b\\").map(|p| p + 2))
                .min()
        } else if rest.starts_with(b"\x1b[?") {
            rest.iter().position(|&b| b == b'c').map(|p| p + 1)
        } else {
            None
        };
        match end {
            Some(end) => {
                answers.extend_from_slice(&rest[..end]);
                i += end;
            }
            None => {
                typed.push(reply[i]);
                i += 1;
            }
        }
    }
    (answers, typed)
}

/// `COLORFGBG` is "fg;bg" or "fg;default;bg" with palette indexes. Background 7
/// or 15 is light, other numbers are dark.
pub fn colorfgbg_is_light(value: Option<&str>) -> Option<bool> {
    let bg = value?.rsplit(';').next()?.trim().parse::<u8>().ok()?;
    Some(bg == 7 || bg == 15)
}

/// Over SSH the reply can be slow enough to arrive after the pager starts, where
/// it would be read as key presses; dumb terminals do not answer at all.
fn query_allowed() -> bool {
    let term = env::var("TERM").unwrap_or_default();
    env::var_os("SSH_CONNECTION").is_none()
        && env::var_os("SSH_TTY").is_none()
        && !term.is_empty()
        && term != "dumb"
}

/// Parse an OSC 11 reply: `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` ending in BEL or ST.
/// Each component may have 1 to 4 hex digits.
pub fn parse_osc11(reply: &[u8]) -> Option<(f64, f64, f64)> {
    let text = String::from_utf8_lossy(reply);
    let start = text.find("]11;")? + 4;
    let body = &text[start..];
    let end = body.find(['\x07', '\x1b']).unwrap_or(body.len());
    let spec = body[..end].trim();
    let rgb = spec
        .strip_prefix("rgb:")
        .or_else(|| spec.strip_prefix("rgba:"))?;
    let mut parts = rgb.split('/').map(|component| {
        let digits = component.len();
        if digits == 0 || digits > 4 {
            return None;
        }
        let value = u32::from_str_radix(component, 16).ok()?;
        let max = (1u32 << (4 * digits)) - 1;
        Some(f64::from(value) / f64::from(max))
    });
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Relative luminance (Rec. 709 weights) of a color with 0 to 1 components.
pub fn luminance(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(unix)]
mod query {
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    use crossterm::terminal;

    /// Ask the terminal for its background color. Returns the raw reply bytes.
    pub fn background(timeout: Duration) -> Option<Vec<u8>> {
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        // Raw mode: the reply must not be echoed or wait for Enter.
        let was_raw = terminal::is_raw_mode_enabled().unwrap_or(false);
        if !was_raw {
            terminal::enable_raw_mode().ok()?;
        }
        let reply = exchange(&mut tty, timeout);
        if !was_raw {
            let _ = terminal::disable_raw_mode();
        }
        reply
    }

    fn exchange(tty: &mut std::fs::File, timeout: Duration) -> Option<Vec<u8>> {
        // OSC 11 query, then DA1.
        tty.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
        tty.flush().ok()?;
        let deadline = Instant::now() + timeout;
        let mut reply = Vec::new();
        let mut buf = [0u8; 256];
        while !has_da1_reply(&reply) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !readable(tty, left) {
                break;
            }
            match tty.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => reply.extend_from_slice(&buf[..n]),
            }
        }
        Some(reply)
    }

    /// Wait until `tty` has input, for at most `timeout`. Uses `select`, because
    /// `poll` does not work on `/dev/tty` on macOS.
    fn readable(tty: &std::fs::File, timeout: Duration) -> bool {
        let fd = tty.as_raw_fd();
        if fd >= libc::FD_SETSIZE as i32 {
            return false;
        }
        // SAFETY: `set` is a zeroed fd_set on the stack, `fd` is an open descriptor
        // below FD_SETSIZE, and `tv` outlives the call.
        unsafe {
            let mut set: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut set);
            libc::FD_SET(fd, &mut set);
            let mut tv = libc::timeval {
                tv_sec: timeout.as_secs() as libc::time_t,
                tv_usec: timeout.subsec_micros() as libc::suseconds_t,
            };
            let ready = libc::select(
                fd + 1,
                &mut set,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            );
            ready > 0 && libc::FD_ISSET(fd, &set)
        }
    }

    /// The DA1 reply is `ESC [ ? ... c`.
    pub fn has_da1_reply(reply: &[u8]) -> bool {
        reply
            .windows(3)
            .position(|w| w == b"\x1b[?")
            .is_some_and(|start| reply[start + 3..].contains(&b'c'))
    }
}

#[cfg(not(unix))]
mod query {
    use std::time::Duration;

    pub fn background(_timeout: Duration) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg() {
        assert_eq!(colorfgbg_is_light(Some("0;15")), Some(true));
        assert_eq!(colorfgbg_is_light(Some("0;default;7")), Some(true));
        assert_eq!(colorfgbg_is_light(Some("15;0")), Some(false));
        assert_eq!(colorfgbg_is_light(Some("garbage")), None);
        assert_eq!(colorfgbg_is_light(None), None);
    }

    #[test]
    fn osc11_replies() {
        let white = parse_osc11(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;22c").unwrap();
        assert_eq!(white, (1.0, 1.0, 1.0));
        let (r, g, b) = parse_osc11(b"\x1b]11;rgb:28/2c/34\x07").unwrap();
        assert!((r - 0x28 as f64 / 255.0).abs() < 1e-9);
        assert!(luminance(r, g, b) < 0.5);
        assert!(parse_osc11(b"\x1b]11;rgba:f/f/f/f\x07").is_some());
        // Only the DA1 reply: the terminal ignored OSC 11.
        assert_eq!(parse_osc11(b"\x1b[?1;2c"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zz/00/00\x07"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:12345/0/0\x07"), None);
    }

    #[test]
    fn keys_typed_during_the_query_are_kept() {
        let reply = b" \x1b]11;rgb:ffff/ffff/ffff\x1b\\j\x1b[?62;22cq";
        let (answers, typed) = split_reply(reply);
        assert_eq!(typed, b" jq");
        assert_eq!(parse_osc11(&answers), Some((1.0, 1.0, 1.0)));
        assert!(query_has_da1(&answers));
        // BEL-terminated reply, and an arrow key typed before it.
        let (answers, typed) = split_reply(b"\x1b[A\x1b]11;rgb:0/0/0\x07\x1b[?1c");
        assert_eq!(typed, b"\x1b[A");
        assert!(parse_osc11(&answers).is_some());
        // Nothing but keys (the terminal did not answer at all).
        assert_eq!(split_reply(b"\r").1, b"\r");
    }

    #[cfg(unix)]
    fn query_has_da1(bytes: &[u8]) -> bool {
        query::has_da1_reply(bytes)
    }

    #[cfg(not(unix))]
    fn query_has_da1(_bytes: &[u8]) -> bool {
        true
    }

    #[test]
    fn light_and_dark_backgrounds() {
        assert!(luminance(1.0, 1.0, 1.0) > 0.5);
        // Solarized light base3 #fdf6e3 and dark base03 #002b36.
        assert!(luminance(0.99, 0.965, 0.89) > 0.5);
        assert!(luminance(0.0, 0.17, 0.21) < 0.5);
    }

    #[cfg(unix)]
    #[test]
    fn da1_detection() {
        assert!(query::has_da1_reply(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;22c"));
        assert!(!query::has_da1_reply(b"\x1b]11;rgb:0/0/0\x07\x1b[?62;22"));
        assert!(!query::has_da1_reply(b""));
    }
}
