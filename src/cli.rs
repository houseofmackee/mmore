use std::path::PathBuf;

use clap::Parser;

use crate::ansi::When;

/// A pager like `more`, with Markdown rendering and syntax highlighting.
#[derive(Debug, Parser)]
#[command(name = "mmore", version, about)]
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
