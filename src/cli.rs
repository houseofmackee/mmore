use std::path::PathBuf;

use clap::Parser;

/// A pager like `more`, with Markdown rendering and syntax highlighting.
#[derive(Debug, Parser)]
#[command(name = "mmore", version, about)]
pub struct Cli {
    /// Files to display. Use `-` for stdin. Reads stdin when no files are given.
    #[arg(value_name = "FILE")]
    pub files: Vec<PathBuf>,

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
