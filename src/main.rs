mod cli;
mod pager;
mod render;
mod source;
mod wrap;

use std::io::{self, BufWriter, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use cli::Cli;
use source::{Origin, Source};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let origins: Vec<Origin> = if cli.files.is_empty() {
        if io::stdin().is_terminal() {
            // Nothing piped in and no files: same as `more`, show usage.
            let _ = Cli::command().print_help();
            return ExitCode::FAILURE;
        }
        vec![Origin::Stdin]
    } else {
        cli.files.iter().map(|p| Origin::from_arg(p)).collect()
    };

    let (lines, all_ok) = collect(&cli, &origins);

    match output(&cli, &lines) {
        Ok(()) if all_ok => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        // The reader went away (for example `mmore file | head`); that is not an error.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mmore: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Load and render every input into one list of lines. Errors are reported on
/// stderr as they happen; the flag is false if any input could not be shown.
fn collect(cli: &Cli, origins: &[Origin]) -> (Vec<String>, bool) {
    let multiple = origins.len() > 1;
    let mut lines = Vec::new();
    let mut all_ok = true;

    for origin in origins {
        let source = match Source::load(origin.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("mmore: {e:#}");
                all_ok = false;
                continue;
            }
        };
        let name = source.origin.display_name();

        if multiple {
            // Separator between files, in the same style as `more`.
            let bar = ":".repeat(14);
            lines.extend([bar.clone(), name.clone(), bar]);
        }

        if source.is_binary() && !cli.force {
            lines.extend([
                String::new(),
                format!("******** {name}: Not a text file ********"),
                String::new(),
            ]);
            continue;
        }

        lines.extend(render::plain::render(&source.text()));
    }

    (lines, all_ok)
}

/// Page the lines on a terminal, or print them when paging is off or pointless.
fn output(cli: &Cli, lines: &[String]) -> io::Result<()> {
    if cli.no_pager || !io::stdout().is_terminal() {
        return dump(lines);
    }
    let Ok((cols, rows)) = crossterm::terminal::size() else {
        return dump(lines);
    };
    // Leave a row free so the shell prompt does not push the first line off screen.
    if !cli.always_page && wrap::row_count(lines, cols as usize) < rows as usize {
        return dump(lines);
    }
    pager::run(
        lines,
        &pager::Options {
            exit_at_eof: cli.exit_at_eof,
        },
    )
}

fn dump(lines: &[String]) -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    for line in lines {
        writeln!(out, "{line}")?;
    }
    out.flush()
}
