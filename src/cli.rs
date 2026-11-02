use std::path::PathBuf;

use clap::Parser;

use std::env;
use std::ffi::OsString;

use crate::ansi::When;

const AFTER_HELP: &str = "\
Environment:
  MMORE_OPTS   Default options, read before the command line (e.g. \"-n --theme InspiredGitHub\")
  NO_COLOR     Disable colors (text attributes such as bold are kept)
  COLORFGBG    Terminal colors; a light background selects a light theme

Press h while paging for the list of keys.";

/// Command-line arguments with the options from `MMORE_OPTS` inserted first.
pub fn args() -> Vec<OsString> {
    let mut args: Vec<OsString> = env::args_os().collect();
    if let Some(opts) = env::var_os("MMORE_OPTS") {
        let extra = split_words(&opts.to_string_lossy());
        let at = args.len().min(1);
        args.splice(at..at, extra.into_iter().map(OsString::from));
    }
    args
}

/// Split like a shell: whitespace separates words, quotes group them, and a
/// backslash escapes the next character (except inside single quotes).
pub fn split_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// A pager like `more`, with Markdown rendering and syntax highlighting.
#[derive(Debug, Parser)]
#[command(
    name = "mmore",
    version,
    about,
    // MMORE_OPTS may set an option that the command line sets again: the last wins.
    args_override_self = true,
    after_help = AFTER_HELP
)]
pub struct Cli {
    /// Files to display. Use `-` for stdin. Reads stdin when no files are given.
    #[arg(value_name = "FILE")]
    pub files: Vec<PathBuf>,

    /// Disable Markdown rendering and syntax highlighting. (Output that is piped
    /// elsewhere is always shown as is, unless --color=always is given.)
    #[arg(short, long, conflicts_with = "language")]
    pub plain: bool,

    /// Force a language, by name or extension (e.g. rust, py, markdown).
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Show source line numbers.
    #[arg(short = 'n', long)]
    pub line_numbers: bool,

    /// Syntax highlighting theme (see --list-themes).
    #[arg(long, value_name = "NAME")]
    pub theme: Option<String>,

    /// List available themes and exit.
    #[arg(long)]
    pub list_themes: bool,

    /// List supported languages and exit.
    #[arg(long)]
    pub list_languages: bool,

    /// Render width, instead of the terminal width.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(10..))]
    pub width: Option<u16>,

    /// Maximum width of rendered Markdown.
    #[arg(long, value_name = "N", default_value_t = 100, value_parser = clap::value_parser!(u16).range(10..))]
    pub max_width: u16,

    /// When to use colors.
    #[arg(long, value_enum, value_name = "WHEN", default_value_t = When::Auto)]
    pub color: When,

    /// When to make links clickable (OSC 8 hyperlinks).
    #[arg(long, value_enum, value_name = "WHEN", default_value_t = When::Auto)]
    pub hyperlinks: When,

    /// Draw Markdown with ASCII characters instead of Unicode symbols.
    #[arg(long)]
    pub ascii: bool,

    /// Hide YAML/TOML front matter in Markdown files.
    #[arg(long)]
    pub no_front_matter: bool,

    /// Scroll with the mouse wheel. The terminal then sends clicks to mmore; to
    /// select text, drag while holding Shift in most terminals (Option in iTerm2).
    #[arg(long)]
    pub mouse: bool,

    /// Display binary files anyway.
    #[arg(long)]
    pub force: bool,

    /// Exit when paging forward past the end, like classic `more`.
    #[arg(short = 'e', long)]
    pub exit_at_eof: bool,

    /// Print to stdout without paging.
    #[arg(long, conflicts_with = "always_page")]
    pub no_pager: bool,

    /// Page even if the content fits on one screen.
    #[arg(long)]
    pub always_page: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_words_like_a_shell() {
        assert_eq!(split_words("  -n   --theme x "), vec!["-n", "--theme", "x"]);
        assert_eq!(
            split_words(r#"--theme "Solarized (dark)""#),
            vec!["--theme", "Solarized (dark)"]
        );
        assert_eq!(split_words("--theme 'a b' c"), vec!["--theme", "a b", "c"]);
        assert_eq!(split_words(r"a\ b"), vec!["a b"]);
        assert_eq!(split_words(r#"'it\s' "q\"q""#), vec![r"it\s", "q\"q"]);
        assert_eq!(split_words("''"), vec![""]);
        assert!(split_words("   ").is_empty());
        // An unterminated quote takes the rest of the string.
        assert_eq!(split_words("'a b"), vec!["a b"]);
    }

    #[test]
    fn later_options_override_earlier_ones() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["mmore", "--theme", "a", "-n", "--theme", "b", "-n", "f"])
            .unwrap();
        assert_eq!(cli.theme.as_deref(), Some("b"));
        assert!(cli.line_numbers);
    }
}
