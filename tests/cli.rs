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
