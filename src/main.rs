mod ansi;
mod assets;
mod cli;
mod detect;
mod document;
mod pager;
mod render;
mod source;
mod style;
mod wrap;

use std::io::{self, BufWriter, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use syntect::highlighting::Theme;

use ansi::{ColorMode, ColorWhen};
use cli::Cli;
use detect::Kind;
use document::{Document, Section};
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

    let stdout_is_tty = io::stdout().is_terminal();
    let color = ColorMode::resolve(cli.color, stdout_is_tty);
    let settings = Settings {
        // Piped output stays raw (like `more` and `cat`) unless colors are forced.
        render: !cli.plain && (stdout_is_tty || cli.color == ColorWhen::Always),
        highlight: color.enabled(),
        theme,
    };
    let (document, all_ok) = collect(&cli, &origins, &settings);

    match output(&cli, &document, color, stdout_is_tty) {
        Ok(()) if all_ok => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        Err(e) => {
            report(e);
            ExitCode::FAILURE
        }
    }
}

/// How inputs are turned into output.
struct Settings {
    /// Render Markdown and highlight code (otherwise show the source as is).
    render: bool,
    /// Colors are available, so highlighting is worthwhile.
    highlight: bool,
    theme: &'static Theme,
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

/// Load every input into a document. Errors are reported on stderr as they
/// happen; the flag is false if any input could not be shown.
fn collect(cli: &Cli, origins: &[Origin], settings: &Settings) -> (Document, bool) {
    let multiple = origins.len() > 1;
    let mut sections = Vec::new();
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
            sections.push(Section::Fixed(vec![
                Line::plain(&bar),
                Line::plain(&name),
                Line::plain(bar),
            ]));
        }

        if source.is_binary() && !cli.force {
            sections.push(Section::Fixed(vec![
                Line::default(),
                Line::plain(format!("******** {name}: Not a text file ********")),
                Line::default(),
            ]));
            continue;
        }

        sections.push(section(cli, &source.origin, source.text(), settings));
    }

    let document = Document {
        sections,
        width: cli.width.map(usize::from),
        max_width: cli.max_width.into(),
        highlight: settings.highlight,
        theme: settings.theme,
    };
    (document, all_ok)
}

fn section(cli: &Cli, origin: &Origin, text: String, settings: &Settings) -> Section {
    if !settings.render {
        return Section::Fixed(render::plain::render(&text));
    }
    let first_line = text.lines().next().unwrap_or("");
    let kind = detect::detect(
        assets::syntaxes(),
        origin,
        first_line,
        cli.language.as_deref(),
    );
    match kind {
        Kind::Markdown => Section::Markdown(text),
        Kind::Code(syntax) if settings.highlight && render::code::should_highlight(&text) => {
            Section::Fixed(render::code::render(&text, syntax, settings.theme))
        }
        Kind::Code(_) | Kind::Plain => Section::Fixed(render::plain::render(&text)),
    }
}

/// Page the document on a terminal, or print it when paging is off or pointless.
fn output(cli: &Cli, document: &Document, color: ColorMode, stdout_is_tty: bool) -> io::Result<()> {
    let size = crossterm::terminal::size().ok().filter(|_| stdout_is_tty);
    let Some((cols, rows)) = size.filter(|_| !cli.no_pager) else {
        let width = size.map_or(DEFAULT_WIDTH, |(cols, _)| cols as usize);
        return dump(&document.render(width).lines, color);
    };
    let rendered = document.render(cols as usize);
    // Leave a row free so the shell prompt does not push the first line off screen.
    if !cli.always_page && wrap::row_count(&rendered.lines, cols as usize) < rows as usize {
        return dump(&rendered.lines, color);
    }
    pager::run(
        document,
        &pager::Options {
            exit_at_eof: cli.exit_at_eof,
            color,
        },
    )
}

/// Render width when there is no terminal to measure.
const DEFAULT_WIDTH: usize = 80;

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
