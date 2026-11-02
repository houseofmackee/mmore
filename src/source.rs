//! Reading input from files or stdin.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// How many leading bytes to inspect when deciding whether input is binary.
const BINARY_SNIFF_LEN: usize = 8 * 1024;

/// Where an input comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Stdin,
    File(PathBuf),
}

impl Origin {
    pub fn from_arg(arg: &Path) -> Self {
        if arg.as_os_str() == "-" {
            Origin::Stdin
        } else {
            Origin::File(arg.to_path_buf())
        }
    }

    /// Name shown to the user in headers and messages.
    pub fn display_name(&self) -> String {
        match self {
            Origin::Stdin => "(standard input)".to_string(),
            Origin::File(path) => path.display().to_string(),
        }
    }
}

/// A fully loaded input.
#[derive(Debug)]
pub struct Source {
    pub origin: Origin,
    pub bytes: Vec<u8>,
}

impl Source {
    pub fn load(origin: Origin) -> Result<Self> {
        let bytes = match &origin {
            Origin::Stdin => {
                let mut buf = Vec::new();
                io::stdin()
                    .lock()
                    .read_to_end(&mut buf)
                    .context("failed to read standard input")?;
                buf
            }
            Origin::File(path) => {
                if path.is_dir() {
                    anyhow::bail!("{}: is a directory", path.display());
                }
                fs::read(path).with_context(|| format!("{}", path.display()))?
            }
        };
        Ok(Source { origin, bytes })
    }

    pub fn is_binary(&self) -> bool {
        looks_binary(&self.bytes)
    }

    /// Input decoded as UTF-8, replacing invalid sequences.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Heuristic: text files do not contain NUL bytes near the start.
pub fn looks_binary(bytes: &[u8]) -> bool {
    let len = bytes.len().min(BINARY_SNIFF_LEN);
    bytes[..len].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_means_stdin() {
        assert_eq!(Origin::from_arg(Path::new("-")), Origin::Stdin);
        assert_eq!(
            Origin::from_arg(Path::new("a.md")),
            Origin::File(PathBuf::from("a.md"))
        );
    }

    #[test]
    fn binary_detection() {
        assert!(!looks_binary(b"hello\nworld\n"));
        assert!(!looks_binary(b""));
        assert!(looks_binary(b"ELF\0\x01\x02"));
    }

    #[test]
    fn nul_after_sniff_window_is_ignored() {
        let mut bytes = vec![b'a'; BINARY_SNIFF_LEN];
        bytes.push(0);
        assert!(!looks_binary(&bytes));
    }
}
