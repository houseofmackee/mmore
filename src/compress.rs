//! Compressed files (gzip, bzip2, xz, zstd), recognized by their first bytes and
//! decompressed by the usual command line tool, whose output is read as it
//! comes (like piped input).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::thread;

pub struct Format {
    /// The command that decompresses it to stdout.
    pub tool: &'static str,
    args: &'static [&'static str],
    magic: &'static [u8],
    /// File name extensions, stripped to see what is inside (`notes.md.gz`).
    extensions: &'static [&'static str],
}

const FORMATS: &[Format] = &[
    Format {
        tool: "gzip",
        args: &["-dc"],
        magic: &[0x1f, 0x8b],
        extensions: &["gz", "z"],
    },
    Format {
        tool: "bzip2",
        args: &["-dc"],
        magic: b"BZh",
        extensions: &["bz2", "bz"],
    },
    Format {
        tool: "xz",
        args: &["-dc"],
        magic: &[0xfd, b'7', b'z', b'X', b'Z', 0x00],
        extensions: &["xz"],
    },
    Format {
        tool: "zstd",
        args: &["-dcq"],
        magic: &[0x28, 0xb5, 0x2f, 0xfd],
        extensions: &["zst", "zstd"],
    },
];

/// The compression format of the file at `path`, judged by its first bytes.
/// Only regular files: opening a named pipe to look would wait for a writer.
pub fn format_of(path: &Path) -> Option<&'static Format> {
    if !path.is_file() {
        return None;
    }
    let mut head = [0u8; 6];
    let mut file = File::open(path).ok()?;
    let mut read = 0;
    while read < head.len() {
        match file.read(&mut head[read..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => read += n,
        }
    }
    let head = &head[..read];
    FORMATS.iter().find(|f| head.starts_with(f.magic))
}

/// The name of what is inside: `notes.md.gz` holds `notes.md`, and a `.tgz`
/// holds a `.tar`. Names without a known extension are kept.
pub fn inner_name(path: &Path, format: &Format) -> PathBuf {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("tgz") => path.with_extension("tar"),
        Some(ext) if format.extensions.contains(&ext) => path.with_extension(""),
        _ => path.to_path_buf(),
    }
}

/// Start decompressing `path`; read the result from the returned pipe. The
/// tool's error messages are discarded so they cannot garble the pager.
pub fn open(path: &Path, format: &Format) -> io::Result<ChildStdout> {
    let mut child = Command::new(format.tool)
        .args(format.args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    // Reap the process when it ends, so it does not linger as a zombie.
    thread::spawn(move || child.wait());
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_names() {
        let gz = &FORMATS[0];
        assert_eq!(
            inner_name(Path::new("notes.md.gz"), gz),
            Path::new("notes.md")
        );
        assert_eq!(
            inner_name(Path::new("a/app.log.GZ"), gz),
            Path::new("a/app.log")
        );
        assert_eq!(inner_name(Path::new("src.tgz"), gz), Path::new("src.tar"));
        assert_eq!(
            inner_name(Path::new("weird.dat"), gz),
            Path::new("weird.dat")
        );
        assert_eq!(
            inner_name(Path::new("x.rs.zst"), &FORMATS[3]),
            Path::new("x.rs")
        );
    }

    #[test]
    fn formats_by_first_bytes() {
        let dir = std::env::temp_dir().join(format!("mmore-magic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: &[(&[u8], Option<&str>)] = &[
            (&[0x1f, 0x8b, 8, 0], Some("gzip")),
            (b"BZh91AY", Some("bzip2")),
            (&[0xfd, b'7', b'z', b'X', b'Z', 0x00, 0], Some("xz")),
            (&[0x28, 0xb5, 0x2f, 0xfd, 0], Some("zstd")),
            (b"plain text", None),
            (b"", None),
        ];
        for (i, (bytes, tool)) in cases.iter().enumerate() {
            let path = dir.join(format!("f{i}"));
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(format_of(&path).map(|f| f.tool), *tool, "case {i}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
