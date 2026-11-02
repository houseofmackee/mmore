use assert_cmd::Command;
use predicates::prelude::*;

fn mmore() -> Command {
    let mut cmd = Command::cargo_bin("mmore").unwrap();
    // Keep the user's own defaults out of the tests.
    cmd.env_remove("MMORE_OPTS");
    cmd
}

#[test]
fn prints_single_file_without_header() {
    mmore()
        .arg("tests/fixtures/hello.txt")
        .assert()
        .success()
        .stdout("hello\nworld\n");
}

#[test]
fn reads_stdin_when_no_files() {
    mmore()
        .write_stdin("a\tb\n")
        .assert()
        .success()
        .stdout("a       b\n");
}

#[test]
fn dash_reads_stdin() {
    mmore()
        .arg("-")
        .write_stdin("x\n")
        .assert()
        .success()
        .stdout("x\n");
}

#[test]
fn multiple_files_get_headers() {
    mmore()
        .args(["tests/fixtures/hello.txt", "tests/fixtures/sample.md"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "::::::::::::::\ntests/fixtures/hello.txt\n::::::::::::::\nhello\n",
        ))
        .stdout(predicate::str::contains(
            "::::::::::::::\ntests/fixtures/sample.md\n::::::::::::::\n# Title\n",
        ));
}

#[test]
fn missing_file_reports_error_and_continues() {
    mmore()
        .args(["does-not-exist.txt", "tests/fixtures/hello.txt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mmore: does-not-exist.txt"))
        .stdout(predicate::str::contains("hello\nworld\n"));
}

#[test]
fn directory_is_an_error() {
    mmore()
        .arg("tests")
        .assert()
        .failure()
        .stderr(predicate::str::contains("is a directory"));
}

#[test]
fn binary_file_is_skipped() {
    mmore()
        .arg("tests/fixtures/binary.bin")
        .assert()
        .success()
        .stdout(predicate::str::contains("Not a text file"));
}

#[test]
fn force_shows_binary_file() {
    mmore()
        .args(["--force", "tests/fixtures/binary.bin"])
        .assert()
        .success()
        .stdout("BIN^@^A^Bdata\n");
}

const ESC: &str = "\x1b[";

/// Run with a predictable terminal environment.
fn mmore_env() -> Command {
    let mut cmd = mmore();
    cmd.env_remove("NO_COLOR")
        .env_remove("MMORE_OPTS")
        .env_remove("COLORTERM")
        .env_remove("COLORFGBG")
        .env("TERM", "xterm-256color");
    cmd
}

#[test]
fn piped_output_has_no_colors_by_default() {
    mmore_env()
        .arg("tests/fixtures/hello.rs")
        .assert()
        .success()
        .stdout(predicate::str::contains(ESC).not())
        .stdout(predicate::str::contains("let greeting = \"hello\";"));
}

#[test]
fn color_always_highlights_code_in_256_colors() {
    mmore_env()
        .args(["--color", "always", "tests/fixtures/hello.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("38;5;"));
}

#[test]
fn truecolor_when_colorterm_says_so() {
    mmore_env()
        .env("COLORTERM", "truecolor")
        .args(["--color", "always", "tests/fixtures/hello.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("38;2;"));
}

#[test]
fn explicit_color_always_overrides_no_color() {
    mmore_env()
        .env("NO_COLOR", "1")
        .args(["--color", "always", "tests/fixtures/hello.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains(ESC));
}

#[test]
fn plain_flag_disables_highlighting() {
    mmore_env()
        .args(["--plain", "--color", "always", "tests/fixtures/hello.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains(ESC).not());
}

#[test]
fn language_flag_highlights_stdin() {
    mmore_env()
        .args(["-l", "python", "--color", "always"])
        .write_stdin("def f():\n    return 1\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(ESC));
}

#[test]
fn shebang_detects_language_on_stdin() {
    mmore_env()
        .args(["--color", "always"])
        .write_stdin("#!/usr/bin/env python3\nimport os\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(ESC));
}

#[test]
fn unknown_text_stays_plain_even_with_color() {
    mmore_env()
        .args(["--color", "always", "tests/fixtures/hello.txt"])
        .assert()
        .success()
        .stdout("hello\nworld\n");
}

#[test]
fn unknown_theme_is_an_error() {
    mmore()
        .args(["--theme", "nope", "tests/fixtures/hello.rs"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown theme 'nope'"));
}

#[test]
fn unknown_language_is_an_error() {
    mmore()
        .args(["-l", "klingon", "tests/fixtures/hello.rs"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown language 'klingon'"));
}

#[test]
fn list_themes_and_languages() {
    mmore_env()
        .arg("--list-themes")
        .assert()
        .success()
        .stdout(predicate::str::contains("base16-ocean.dark (default)"))
        .stdout(predicate::str::contains("InspiredGitHub"));
    mmore()
        .arg("--list-languages")
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust: rs"))
        .stdout(predicate::str::contains("Python: py"));
}

#[test]
fn other_theme_changes_colors() {
    let dark = mmore_env()
        .args(["--color", "always", "tests/fixtures/hello.rs"])
        .output()
        .unwrap()
        .stdout;
    let light = mmore_env()
        .args([
            "--color",
            "always",
            "--theme",
            "InspiredGitHub",
            "tests/fixtures/hello.rs",
        ])
        .output()
        .unwrap()
        .stdout;
    assert_ne!(dark, light);
}

#[test]
fn piped_markdown_stays_raw() {
    mmore_env()
        .arg("tests/fixtures/sample.md")
        .assert()
        .success()
        .stdout("# Title\n\nSome *markdown*.\n");
}

#[test]
fn markdown_renders_with_color_always() {
    mmore_env()
        .args([
            "--color",
            "always",
            "--width",
            "20",
            "tests/fixtures/sample.md",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Title").not())
        .stdout(predicate::str::contains(format!(
            "{}\x1b[0m\n",
            "═".repeat(20)
        )))
        .stdout(predicate::str::contains("\x1b[3mmarkdown\x1b[0m"));
}

#[test]
fn max_width_caps_markdown() {
    mmore_env()
        .args([
            "--color",
            "always",
            "--max-width",
            "12",
            "tests/fixtures/sample.md",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("{}\x1b", "═".repeat(12))));
}

#[test]
fn plain_flag_shows_markdown_source() {
    mmore_env()
        .args(["--plain", "--color", "always", "tests/fixtures/sample.md"])
        .assert()
        .success()
        .stdout("# Title\n\nSome *markdown*.\n");
}

#[test]
fn language_flag_renders_markdown_from_stdin() {
    mmore_env()
        .args(["-l", "md", "--color", "always", "--width", "30"])
        .write_stdin("## Hi\n\n- item\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("  • ").and(predicate::str::contains("## Hi").not()));
}

#[test]
fn width_must_be_reasonable() {
    mmore().args(["--width", "3", "x"]).assert().code(2);
}

const MD_EXT: &str = "tests/fixtures/markdown/extended.md";
const OSC8: &str = "\x1b]8;";

#[test]
fn hyperlinks_always_emits_osc8_for_absolute_urls() {
    mmore_env()
        .args(["--color", "always", "--hyperlinks", "always", MD_EXT])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\x1b]8;id=mmore0;https://ci.example.com\x1b\\",
        ))
        // Relative image paths are not clickable.
        .stdout(predicate::str::contains("docs/logo.png\x1b\\").not());
}

#[test]
fn hyperlinks_auto_is_off_when_piped() {
    mmore_env()
        .env("TERM_PROGRAM", "iTerm.app")
        .args(["--color", "always", MD_EXT])
        .assert()
        .success()
        .stdout(predicate::str::contains(OSC8).not());
}

#[test]
fn hyperlinks_never_overrides_force_env() {
    mmore_env()
        .env("FORCE_HYPERLINK", "1")
        .args(["--color", "always", "--hyperlinks", "never", MD_EXT])
        .assert()
        .success()
        .stdout(predicate::str::contains(OSC8).not());
}

/// Remove SGR color sequences.
fn strip_sgr(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn ascii_flag_avoids_unicode_symbols() {
    let out = mmore_env()
        .args(["--color", "always", "--ascii", "--width", "60", MD_EXT])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = strip_sgr(&String::from_utf8(out.stdout).unwrap());
    assert!(text.contains("  [x] Render tables"), "{text}");
    assert!(text.contains("+-------+"), "{text}");
    assert!(
        !text.contains('☑') && !text.contains('┌') && !text.contains('•'),
        "{text}"
    );
}

#[test]
fn front_matter_can_be_hidden() {
    mmore_env()
        .args(["--color", "always", MD_EXT])
        .assert()
        .success()
        .stdout(predicate::str::contains("front matter (yaml)"));
    mmore_env()
        .args(["--color", "always", "--no-front-matter", MD_EXT])
        .assert()
        .success()
        .stdout(predicate::str::contains("front matter").not())
        .stdout(predicate::str::contains("title: Extended").not());
}

#[test]
fn line_numbers_when_printing() {
    mmore_env()
        .args(["-n", "tests/fixtures/hello.txt"])
        .assert()
        .success()
        .stdout("  1 hello\n  2 world\n");
}

#[test]
fn line_numbers_with_several_files_skip_headers() {
    mmore_env()
        .args(["-n", "tests/fixtures/hello.txt", "tests/fixtures/sample.md"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "::::::::::::::\ntests/fixtures/sample.md\n::::::::::::::\n  1 # Title\n",
        ));
}

#[test]
fn mmore_opts_sets_defaults() {
    mmore_env()
        .env("MMORE_OPTS", "-n")
        .arg("tests/fixtures/hello.txt")
        .assert()
        .success()
        .stdout("  1 hello\n  2 world\n");
}

#[test]
fn command_line_overrides_mmore_opts() {
    let with = |opts: &str, args: &[&str]| {
        mmore_env()
            .env("MMORE_OPTS", opts)
            .args(args)
            .output()
            .unwrap()
            .stdout
    };
    let base = ["--color", "always", "tests/fixtures/hello.rs"];
    let light = with(
        "",
        &["--theme", "InspiredGitHub", base[0], base[1], base[2]],
    );
    let dark = with("", &base);
    // MMORE_OPTS picks the light theme; the command line switches back to dark.
    assert_eq!(with("--theme InspiredGitHub", &base), light);
    assert_eq!(
        with(
            "--theme InspiredGitHub",
            &["--theme", "base16-ocean.dark", base[0], base[1], base[2]]
        ),
        dark
    );
}

#[test]
fn mmore_opts_with_quotes() {
    mmore_env()
        .env("MMORE_OPTS", r#"--theme "Solarized (dark)""#)
        .args(["--color", "always", "tests/fixtures/hello.rs"])
        .assert()
        .success();
}

#[test]
fn help_mentions_environment() {
    mmore()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("MMORE_OPTS"));
}

#[test]
fn piped_input_is_copied_before_it_ends() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("mmore"))
        .env_remove("MMORE_OPTS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            tx.send(line.unwrap()).unwrap();
        }
    });
    // The first line comes out while stdin is still open.
    writeln!(stdin, "first").unwrap();
    stdin.flush().unwrap();
    let first = rx.recv_timeout(Duration::from_secs(5));
    assert_eq!(first.as_deref(), Ok("first"));
    writeln!(stdin, "second\tline").unwrap();
    drop(stdin);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).as_deref(),
        Ok("second  line")
    );
    assert!(child.wait().unwrap().success());
}

#[test]
fn piped_binary_input_is_detected() {
    mmore()
        .write_stdin(b"PNG\0\x01\x02".to_vec())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "(standard input): Not a text file",
        ));
}

#[test]
fn piped_input_with_line_numbers() {
    mmore()
        .arg("-n")
        .write_stdin("a\nb\n")
        .assert()
        .success()
        // Piped input always gets room for 5 digits (its length is not known).
        .stdout("    1 a\n    2 b\n");
}

/// A named pipe in a fresh temporary directory (Unix only).
#[cfg(unix)]
fn fifo(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mmore-fifo-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join(name);
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo failed");
    (dir, path)
}

#[cfg(unix)]
#[test]
fn named_pipes_are_shown_as_they_arrive() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let (dir, path) = fifo("log");
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("mmore"))
        .env_remove("MMORE_OPTS")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            tx.send(line.unwrap()).unwrap();
        }
    });
    // Opening the FIFO for writing waits until mmore has opened it for reading.
    let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    writeln!(writer, "first").unwrap();
    writer.flush().unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).as_deref(),
        Ok("first")
    );
    writeln!(writer, "second").unwrap();
    drop(writer);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).as_deref(),
        Ok("second")
    );
    assert!(child.wait().unwrap().success());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn named_pipes_with_language_are_read_whole() {
    use std::io::Write;
    let (dir, path) = fifo("code");
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut w = std::fs::OpenOptions::new()
            .write(true)
            .open(writer_path)
            .unwrap();
        w.write_all(b"fn main() {}\n").unwrap();
    });
    mmore_env()
        .args(["-l", "rust", "--color", "always"])
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("38;5;"));
    writer.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn regular_files_are_not_streamed() {
    // A regular file keeps its full rendering (here: Markdown with headers).
    mmore_env()
        .args([
            "--color",
            "always",
            "--width",
            "20",
            "tests/fixtures/sample.md",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("═".repeat(20)));
}

#[test]
fn colors_in_piped_input_are_shown_as_styles() {
    mmore_env()
        .args(["--color", "always"])
        .write_stdin("\x1b[31mred\x1b[0m \x1b[1mbold\x1b[22m\n")
        .assert()
        .success()
        .stdout("\x1b[31mred\x1b[0m \x1b[1mbold\x1b[0m\n");
}

#[test]
fn colors_are_removed_when_output_is_plain() {
    // Piped output: the text without codes, ready for grep.
    mmore_env()
        .write_stdin("\x1b[31mred\x1b[0m\x1b[K done\n")
        .assert()
        .success()
        .stdout("red done\n");
}

#[test]
fn show_escapes_keeps_codes_visible() {
    mmore_env()
        .arg("--show-escapes")
        .write_stdin("\x1b[31mred\x1b[0m\n")
        .assert()
        .success()
        .stdout("^[[31mred^[[0m\n");
}

#[test]
fn man_page_overstriking_becomes_bold_and_underline() {
    mmore_env()
        .write_stdin("N\x08NA\x08AM\x08ME\x08E _\x08f\n")
        .assert()
        .success()
        .stdout("NAME f\n");
    mmore_env()
        .args(["--color", "always"])
        .write_stdin("N\x08NA\x08AM\x08ME\x08E _\x08f\n")
        .assert()
        .success()
        .stdout("\x1b[1mNAME\x1b[0m \x1b[4mf\x1b[0m\n");
}

#[test]
fn colors_in_plain_files_but_not_in_code() {
    let dir = std::env::temp_dir().join(format!("mmore-colors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("build.log");
    std::fs::write(&log, "\x1b[32mok\x1b[0m\n").unwrap();
    mmore_env()
        .args(["--color", "always"])
        .arg(&log)
        .assert()
        .success()
        .stdout("\x1b[32mok\x1b[0m\n");
    // In a code file an escape character is part of the source: shown, not run.
    let code = dir.join("x.rs");
    std::fs::write(&code, "let s = \"\x1b[31m\";\n").unwrap();
    mmore_env()
        .args(["--color", "always"])
        .arg(&code)
        .assert()
        .success()
        .stdout(predicate::str::contains("^[[31m"));
    std::fs::remove_dir_all(dir).unwrap();
}
