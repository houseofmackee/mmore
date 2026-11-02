use assert_cmd::Command;
use predicates::prelude::*;

fn mmore() -> Command {
    Command::cargo_bin("mmore").unwrap()
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
