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
}
