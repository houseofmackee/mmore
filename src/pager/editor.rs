//! Opening the current file in an editor (`v`), as `less` does.

use std::env;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::cli::split_words;

/// Editors that take `+LINE` before the file name to start at that line.
const PLUS_LINE: &[&str] = &[
    "vi",
    "vim",
    "nvim",
    "view",
    "vim.basic",
    "vim.tiny",
    "nano",
    "pico",
    "emacs",
    "emacsclient",
    "micro",
    "kak",
    "joe",
    "jed",
    "ne",
    "mg",
    "mcedit",
];

/// The editor command: `$VISUAL`, then `$EDITOR`, then `vi`. May include
/// arguments, as in "code --wait".
fn command() -> Vec<String> {
    for name in ["VISUAL", "EDITOR"] {
        if let Ok(value) = env::var(name) {
            let words = split_words(&value);
            if !words.is_empty() {
                return words;
            }
        }
    }
    vec!["vi".to_string()]
}

/// Arguments for opening `path` at `line` with the editor `program`.
pub fn arguments(program: &str, path: &Path, line: usize) -> Vec<OsString> {
    let name = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    match name {
        n if PLUS_LINE.contains(&n) => vec![format!("+{line}").into(), path.into()],
        // Helix takes FILE:LINE.
        "hx" | "helix" => vec![format!("{}:{line}", path.display()).into()],
        _ => vec![path.into()],
    }
}

/// Run the editor on `path` at `line` and wait for it. The editor talks to the
/// terminal directly, even when mmore's own input is a pipe.
pub fn open(path: &Path, line: usize) -> io::Result<()> {
    let words = command();
    let (program, extra) = words.split_first().expect("command() is never empty");
    let mut cmd = Command::new(program);
    cmd.args(extra).args(arguments(program, path, line));
    if let Ok(tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") {
        cmd.stdin(Stdio::from(tty));
    }
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_arguments_by_editor() {
        let path = Path::new("notes.md");
        let args = |p: &str| -> Vec<String> {
            arguments(p, path, 12)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(args("vim"), vec!["+12", "notes.md"]);
        assert_eq!(args("/usr/bin/nano"), vec!["+12", "notes.md"]);
        assert_eq!(args("hx"), vec!["notes.md:12"]);
        assert_eq!(args("code"), vec!["notes.md"]);
    }
}
