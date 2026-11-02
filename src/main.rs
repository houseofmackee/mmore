mod cli;
mod render;
mod source;

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

    match run(&cli, &origins) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        // The reader went away (for example `mmore file | head`); that is not an error.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mmore: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Writes every input to stdout. Returns `Ok(false)` if any input could not be shown.
fn run(cli: &Cli, origins: &[Origin]) -> io::Result<bool> {
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let multiple = origins.len() > 1;
    let mut all_ok = true;

    for origin in origins {
        let source = match Source::load(origin.clone()) {
            Ok(s) => s,
            Err(e) => {
                out.flush()?;
                eprintln!("mmore: {e:#}");
                all_ok = false;
                continue;
            }
        };

        if multiple {
            write_file_header(&mut out, &source.origin.display_name())?;
        }

        if source.is_binary() && !cli.force {
            writeln!(
                out,
                "\n******** {}: Not a text file ********\n",
                source.origin.display_name()
            )?;
            continue;
        }

        for line in render::plain::render(&source.text()) {
            writeln!(out, "{line}")?;
        }
    }

    out.flush()?;
    Ok(all_ok)
}

/// Separator between files, in the same style as `more`.
fn write_file_header(out: &mut impl Write, name: &str) -> io::Result<()> {
    let bar = ":".repeat(14);
    writeln!(out, "{bar}\n{name}\n{bar}")
}
