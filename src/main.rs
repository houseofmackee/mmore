mod ansi;
mod assets;
mod cli;
mod detect;
mod pager;
mod render;
mod source;
mod style;
mod wrap;

use std::io::{self, BufWriter, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use syntect::highlighting::Theme;

use ansi::ColorMode;
use cli::Cli;
use detect::Kind;
use source::{Origin, Source};
use style::Line;

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.list_themes {
        return finish(list_themes());
    }
    if cli.list_languages {
        return finish(list_languages());
    }

    let theme_name = cli.theme.as_deref().unwrap_or(assets::default_theme_name());
    let Some(theme) = assets::theme(theme_name) else {
        eprintln!("mmore: unknown theme '{theme_name}' (see --list-themes)");
        return ExitCode::from(2);
    };
    if let Some(lang) = &cli.language
        && detect::by_language(assets::syntaxes(), lang).is_none()
    {
        eprintln!("mmore: unknown language '{lang}' (see --list-languages)");
        return ExitCode::from(2);
    }

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

    let color = ColorMode::resolve(cli.color, io::stdout().is_terminal());
    let (lines, all_ok) = collect(&cli, &origins, color, theme);

    match output(&cli, &lines, color) {
        Ok(()) if all_ok => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        Err(e) => {
            report(e);
            ExitCode::FAILURE
        }
    }
}

fn finish(result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report(e);
            ExitCode::FAILURE
        }
    }
}

fn report(e: io::Error) {
    // The reader went away (for example `mmore file | head`); that is not worth a message.
    if e.kind() != io::ErrorKind::BrokenPipe {
        eprintln!("mmore: {e}");
    }
}

/// Load and render every input into one list of lines. Errors are reported on
/// stderr as they happen; the flag is false if any input could not be shown.
fn collect(cli: &Cli, origins: &[Origin], color: ColorMode, theme: &Theme) -> (Vec<Line>, bool) {
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
            lines.extend([Line::plain(&bar), Line::plain(&name), Line::plain(bar)]);
        }

        if source.is_binary() && !cli.force {
            lines.extend([
                Line::default(),
                Line::plain(format!("******** {name}: Not a text file ********")),
                Line::default(),
            ]);
            continue;
        }

        let text = source.text();
        lines.extend(render(cli, &source.origin, &text, color, theme));
    }

    (lines, all_ok)
}

fn render(cli: &Cli, origin: &Origin, text: &str, color: ColorMode, theme: &Theme) -> Vec<Line> {
    // Highlighting is pointless without color.
    if cli.plain || !color.enabled() || !render::code::should_highlight(text) {
        return render::plain::render(text);
    }
    let syntaxes = assets::syntaxes();
    let first_line = text.lines().next().unwrap_or("");
    match detect::detect(syntaxes, origin, first_line, cli.language.as_deref()) {
        Kind::Plain => render::plain::render(text),
        Kind::Code(syntax) => render::code::render(text, syntax, theme),
        // Until the Markdown renderer lands, highlight Markdown source instead.
        Kind::Markdown => match syntaxes.find_syntax_by_extension("md") {
            Some(syntax) => render::code::render(text, syntax, theme),
            None => render::plain::render(text),
        },
    }
}

/// Page the lines on a terminal, or print them when paging is off or pointless.
fn output(cli: &Cli, lines: &[Line], color: ColorMode) -> io::Result<()> {
    if cli.no_pager || !io::stdout().is_terminal() {
        return dump(lines, color);
    }
    let Ok((cols, rows)) = crossterm::terminal::size() else {
        return dump(lines, color);
    };
    // Leave a row free so the shell prompt does not push the first line off screen.
    if !cli.always_page && wrap::row_count(lines, cols as usize) < rows as usize {
        return dump(lines, color);
    }
    pager::run(
        lines,
        &pager::Options {
            exit_at_eof: cli.exit_at_eof,
            color,
        },
    )
}

fn dump(lines: &[Line], color: ColorMode) -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    for line in lines {
        writeln!(out, "{}", ansi::render(line, color))?;
    }
    out.flush()
}

fn list_themes() -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    let default = assets::default_theme_name();
    for name in assets::themes().themes.keys() {
        let marker = if name == default { " (default)" } else { "" };
        writeln!(out, "{name}{marker}")?;
    }
    out.flush()
}

fn list_languages() -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    let mut syntaxes: Vec<_> = assets::syntaxes()
        .syntaxes()
        .iter()
        .filter(|s| !s.hidden && !s.file_extensions.is_empty())
        .collect();
    syntaxes.sort_by_key(|s| s.name.to_lowercase());
    for s in syntaxes {
        writeln!(out, "{}: {}", s.name, s.file_extensions.join(", "))?;
    }
    out.flush()
}
