//! Input that arrives over time (a pipe), shown while it is still being read.
//!
//! A background thread reads lines and appends them; readers take snapshots or
//! wait for more. Lines get the same treatment as `plain::render` (tabs expanded,
//! control characters made visible), so they match what a whole file would show.

use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::render::plain;
use crate::source::looks_binary;
use crate::style::Line;

/// Leading bytes checked for NUL, as for files (see `source::looks_binary`).
const SNIFF_LEN: usize = 8 * 1024;
/// Lines are published at least this often when input arrives quickly.
const BATCH: usize = 1024;

pub struct Stream {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    lines: Vec<Line>,
    done: bool,
    binary: bool,
    /// The raw text, kept only until the caller knows whether it needs it.
    text: Option<String>,
}

impl Stream {
    /// Start reading `reader` in the background. With `keep_text`, the raw text is
    /// also kept (see `take_text`). Unless `force` is set, input that looks binary
    /// is replaced by a short message, as for files.
    pub fn spawn(
        reader: impl Read + Send + 'static,
        name: String,
        force: bool,
        keep_text: bool,
    ) -> Arc<Stream> {
        let stream = Arc::new(Stream {
            state: Mutex::new(State {
                lines: Vec::new(),
                done: false,
                binary: false,
                text: keep_text.then(String::new),
            }),
            changed: Condvar::new(),
        });
        let worker = Arc::clone(&stream);
        thread::spawn(move || worker.read_all(reader, &name, force));
        stream
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read_all(&self, reader: impl Read, name: &str, force: bool) {
        let mut reader = BufReader::with_capacity(64 * 1024, reader);
        let mut buf = Vec::new();
        let mut sniffed = 0;
        let mut batch = Vec::new();
        let mut text = String::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if !force && sniffed < SNIFF_LEN {
                let end = buf.len().min(SNIFF_LEN - sniffed);
                sniffed += buf.len();
                if looks_binary(&buf[..end]) {
                    self.binary(name);
                    return;
                }
            }
            let decoded = String::from_utf8_lossy(&buf);
            text.push_str(&decoded);
            // Same line endings as `str::lines`: "\n", or "\r\n".
            let body = decoded
                .strip_suffix('\n')
                .map_or(&*decoded, |l| l.strip_suffix('\r').unwrap_or(l));
            batch.push(plain::sanitize(&Line::plain(body)));
            // Publish when the next read would wait for input, or the batch is big.
            if reader.buffer().is_empty() || batch.len() >= BATCH {
                self.publish(&mut batch, &mut text, false);
            }
        }
        self.publish(&mut batch, &mut text, true);
    }

    fn publish(&self, batch: &mut Vec<Line>, text: &mut String, done: bool) {
        let mut state = self.state();
        state.lines.append(batch);
        if let Some(kept) = &mut state.text {
            kept.push_str(text);
        }
        text.clear();
        state.done |= done;
        drop(state);
        self.changed.notify_all();
    }

    fn binary(&self, name: &str) {
        let mut state = self.state();
        state.lines = vec![
            Line::default(),
            Line::plain(format!("******** {name}: Not a text file ********")),
            Line::default(),
        ];
        state.binary = true;
        state.done = true;
        state.text = None;
        drop(state);
        self.changed.notify_all();
    }

    pub fn len(&self) -> usize {
        self.state().lines.len()
    }

    pub fn is_done(&self) -> bool {
        self.state().done
    }

    pub fn is_binary(&self) -> bool {
        self.state().binary
    }

    pub fn line(&self, index: usize) -> Option<Line> {
        self.state().lines.get(index).cloned()
    }

    /// Lines from `start` on, as read so far.
    pub fn lines_from(&self, start: usize) -> Vec<Line> {
        let state = self.state();
        state
            .lines
            .get(start..)
            .map(<[Line]>::to_vec)
            .unwrap_or_default()
    }

    /// Wait until there are at least `count` lines or the input has ended, for at
    /// most `timeout` (forever with `None`). Returns the line count and whether the
    /// input has ended.
    pub fn wait_for(&self, count: usize, timeout: Option<Duration>) -> (usize, bool) {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut state = self.state();
        while state.lines.len() < count && !state.done {
            match deadline {
                None => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    state = self
                        .changed
                        .wait_timeout(state, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
        (state.lines.len(), state.done)
    }

    /// The raw text, once the input has ended (waits for it). `None` if it was not
    /// kept, was dropped, or the input was binary.
    pub fn take_text(&self) -> Option<String> {
        self.wait_for(usize::MAX, None);
        self.state().text.take()
    }

    /// Stop keeping the raw text (it is only needed to render whole input).
    pub fn drop_text(&self) {
        self.state().text = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};
    use std::sync::mpsc;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn reads_lines_like_plain_render() {
        let input = "a\r\nb\tc\n\x1b[31m\nlast";
        let s = Stream::spawn(Cursor::new(input), "x".into(), false, true);
        assert_eq!(s.wait_for(usize::MAX, None), (4, true));
        assert_eq!(texts(&s.lines_from(0)), texts(&plain::render(input)));
        assert_eq!(s.take_text().as_deref(), Some(input));
        assert_eq!(texts(&s.lines_from(2)), vec!["^[[31m", "last"]);
        assert!(s.lines_from(10).is_empty());
    }

    #[test]
    fn binary_input_becomes_a_message() {
        let s = Stream::spawn(
            Cursor::new(b"ab\0cd\n".to_vec()),
            "(standard input)".into(),
            false,
            true,
        );
        s.wait_for(usize::MAX, None);
        assert!(s.is_binary());
        assert_eq!(
            s.line(1).unwrap().text(),
            "******** (standard input): Not a text file ********"
        );
        assert!(s.take_text().is_none());
        let forced = Stream::spawn(Cursor::new(b"ab\0cd\n".to_vec()), "x".into(), true, false);
        forced.wait_for(usize::MAX, None);
        assert_eq!(forced.line(0).unwrap().text(), "ab^@cd");
    }

    /// A reader fed from a channel, so the test controls when input arrives.
    struct Feed(mpsc::Receiver<Vec<u8>>, Vec<u8>);

    impl Read for Feed {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(chunk) => self.1 = chunk,
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.1.len());
            out[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn lines_appear_while_input_is_still_open() {
        let (tx, rx) = mpsc::channel();
        let s = Stream::spawn(Feed(rx, Vec::new()), "x".into(), false, false);
        tx.send(b"one\ntwo\n".to_vec()).unwrap();
        assert_eq!(s.wait_for(2, Some(Duration::from_secs(5))), (2, false));
        // No more input yet: waiting times out without the input ending.
        assert_eq!(s.wait_for(3, Some(Duration::from_millis(50))), (2, false));
        tx.send(b"three\n".to_vec()).unwrap();
        drop(tx);
        assert_eq!(s.wait_for(usize::MAX, None), (3, true));
        assert!(s.take_text().is_none());
    }
}
