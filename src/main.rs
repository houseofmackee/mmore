mod ansi;
mod assets;
mod background;
mod cli;
mod compress;
mod detect;
mod document;
mod pager;
mod render;
mod source;
mod stream;
mod style;
mod wrap;

use std::io::{self, BufWriter, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use syntect::highlighting::Theme;

use ansi::{ColorMode, When};
use cli::Cli;
use detect::Kind;
use document::{Document, File, LazyHighlight, Section};
use source::{Origin, Source};
use stream::Stream;
use style::Line;

fn main() -> ExitCode {
    match cli::Packaging::from_args() {
        Some(Ok(cli::Packaging::Completions(shell))) => {
            cli::completions(shell, &mut io::stdout());
            return ExitCode::SUCCESS;
        }
        Some(Ok(cli::Packaging::Man)) => return finish(cli::man_page(&mut io::stdout())),
        Some(Err(e)) => {
            eprintln!("mmore: {e}");
            return ExitCode::from(2);
        }
        None => {}
    }
    let cli = Cli::parse_from(cli::args());

    if cli.list_themes {
        return finish(list_themes());
    }
    if cli.list_languages {
        return finish(list_languages());
    }

    let stdout_is_tty = io::stdout().is_terminal();
    let color = ColorMode::resolve(cli.color, stdout_is_tty);
    let theme_name = match &cli.theme {
        Some(name) => name.as_str(),
        None => assets::default_theme_name(light_background(color, stdout_is_tty)),
    };
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

    let settings = Settings {
        // Piped output stays raw (like `more` and `cat`) unless colors are forced.
        render: !cli.plain && (stdout_is_tty || cli.color == When::Always),
        highlight: color.enabled(),
        theme,
    };
    let (document, all_ok) = collect(&cli, &origins, &settings);
    let hyperlinks = match cli.hyperlinks {
        When::Always => true,
        When::Never => false,
        When::Auto => stdout_is_tty && color.enabled() && ansi::hyperlinks_supported(),
    };

    match output(&cli, &settings, document, color, hyperlinks, stdout_is_tty) {
        Ok(()) if all_ok => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        Err(e) => {
            report(e);
            ExitCode::FAILURE
        }
    }
}

/// Whether the terminal background is light. Asking the terminal (OSC 11) only
/// makes sense when writing to it with colors; otherwise only `COLORFGBG` counts.
fn light_background(color: ColorMode, stdout_is_tty: bool) -> bool {
    let light = if stdout_is_tty && color.enabled() {
        background::is_light()
    } else {
        background::colorfgbg_is_light(std::env::var("COLORFGBG").ok().as_deref())
    };
    light.unwrap_or(false)
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
    let mut files = Vec::new();
    let mut all_ok = true;

    for origin in origins {
        // Compressed files are decompressed as they are read.
        if let Origin::File(path) = origin
            && path.is_file()
            && let Some(format) = compress::format_of(path)
        {
            match compress::open(path, format) {
                Ok(pipe) => {
                    let naming = Naming {
                        name: origin.display_name(),
                        detect: Origin::File(compress::inner_name(path, format)),
                        path: None,
                    };
                    let language = cli.language.as_deref();
                    files.push(stream_input(cli, settings, &naming, pipe, language));
                }
                Err(e) => {
                    eprintln!(
                        "mmore: {}: needs the {} command to decompress it ({e})",
                        path.display(),
                        format.tool
                    );
                    all_ok = false;
                }
            }
            continue;
        }
        // Pipes are shown while their input is still arriving, when they can be.
        if cli.language.is_none() {
            let naming = Naming::of(origin);
            match streamable(origin) {
                Some(Input::Stdin) => {
                    files.push(stream_input(cli, settings, &naming, io::stdin(), None));
                    continue;
                }
                Some(Input::File(file)) => {
                    files.push(stream_input(cli, settings, &naming, file, None));
                    continue;
                }
                None => {}
            }
        }
        let source = match Source::load(origin.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("mmore: {e:#}");
                all_ok = false;
                continue;
            }
        };
        let name = source.origin.display_name();

        if source.is_binary() && !cli.force {
            let message = vec![
                Line::default(),
                Line::plain(format!("******** {name}: Not a text file ********")),
                Line::default(),
            ];
            files.push(File {
                name,
                path: None,
                content: Section::Plain,
                raw: message,
                stream: None,
            });
            continue;
        }

        files.push(text_file(
            cli,
            &Naming::of(&source.origin),
            source.text(),
            settings,
        ));
    }

    let document = Document {
        files,
        width: cli.width.map(usize::from),
        max_width: cli.max_width.into(),
        highlight: settings.highlight,
        theme: settings.theme,
        ascii: cli.ascii,
        front_matter: !cli.no_front_matter,
        line_numbers: cli.line_numbers,
    };
    (document, all_ok)
}

/// What an input is called, what name decides how it is shown, and where it can
/// be edited. Usually all the same file; a compressed `notes.md.gz` is shown
/// under its own name, rendered as `notes.md`, and cannot be edited.
struct Naming {
    name: String,
    detect: Origin,
    path: Option<std::path::PathBuf>,
}

impl Naming {
    fn of(origin: &Origin) -> Self {
        Naming {
            name: origin.display_name(),
            detect: origin.clone(),
            path: match origin {
                Origin::File(path) => Some(path.clone()),
                Origin::Stdin => None,
            },
        }
    }
}

fn text_file(cli: &Cli, naming: &Naming, text: String, settings: &Settings) -> File {
    let kind = content_kind(cli, &naming.detect, &text, settings);
    // Plain text shows the colors in it (as from `git log --color`); the raw view
    // of Markdown and code shows their source as written.
    let raw = if matches!(kind, Kind::Plain) && !cli.show_escapes {
        render::plain::render_colored(&text)
    } else {
        render::plain::render(&text)
    };
    let content = match kind {
        Kind::Plain => Section::Plain,
        Kind::Markdown => Section::Markdown(text),
        Kind::Code(syntax) => {
            Section::Code(Arc::new(LazyHighlight::new(text, syntax, settings.theme)))
        }
    };
    File {
        name: naming.name.clone(),
        path: naming.path.clone(),
        raw,
        content,
        stream: None,
    }
}

/// How long to wait for the first line of piped input, which may name a language
/// (a `#!` line). Silent producers get a plain view after this.
const FIRST_LINE_WAIT: Duration = Duration::from_millis(300);

/// An input that can be shown while it is still being read.
enum Input {
    Stdin,
    File(std::fs::File),
}

/// Whether `origin` is a pipe or similar (stdin, a FIFO, `<(command)`) that is
/// best read in the background. Regular files are read at once; terminals are
/// left alone, since the pager reads keys from the terminal.
fn streamable(origin: &Origin) -> Option<Input> {
    match origin {
        Origin::Stdin => (!io::stdin().is_terminal()).then_some(Input::Stdin),
        Origin::File(path) => {
            let meta = std::fs::metadata(path).ok()?;
            if meta.is_file() || meta.is_dir() {
                return None;
            }
            let file = std::fs::File::open(path).ok()?;
            (!file.is_terminal()).then_some(Input::File(file))
        }
    }
}

/// Read an input in the background so it can be shown as it arrives. If the
/// first line names a language (a `#!` line), highlighting needs the whole
/// input, so wait for it and render it like a file.
fn stream_input(
    cli: &Cli,
    settings: &Settings,
    naming: &Naming,
    reader: impl std::io::Read + Send + 'static,
    language: Option<&str>,
) -> File {
    let name = naming.name.clone();
    let stream = Stream::spawn(
        reader,
        name.clone(),
        cli.force,
        settings.render,
        !cli.show_escapes,
    );
    stream.wait_for(1, Some(FIRST_LINE_WAIT));
    if settings.render && !stream.is_binary() {
        let first = stream.line(0).map(|l| l.text()).unwrap_or_default();
        let kind = detect::detect(assets::syntaxes(), &naming.detect, &first, language);
        if !matches!(kind, Kind::Plain) {
            // Rendering or highlighting needs the whole input.
            let text = stream.take_text().unwrap_or_default();
            return text_file(cli, naming, text, settings);
        }
    }
    stream.drop_text();
    File {
        name,
        path: None,
        content: Section::Plain,
        raw: Vec::new(),
        stream: Some(stream),
    }
}

/// How a file's content will be shown: rendered Markdown, highlighted code, or
/// plain text (when rendering is off, highlighting is pointless, or nothing fits).
fn content_kind(cli: &Cli, origin: &Origin, text: &str, settings: &Settings) -> Kind {
    if !settings.render {
        return Kind::Plain;
    }
    let first_line = text.lines().next().unwrap_or("");
    let kind = detect::detect(
        assets::syntaxes(),
        origin,
        first_line,
        cli.language.as_deref(),
    );
    match kind {
        Kind::Code(_) if !settings.highlight => Kind::Plain,
        kind => kind,
    }
}

/// Page the document on a terminal, or print it when paging is off or pointless.
fn output(
    cli: &Cli,
    settings: &Settings,
    document: Document,
    color: ColorMode,
    hyperlinks: bool,
    stdout_is_tty: bool,
) -> io::Result<()> {
    let size = crossterm::terminal::size().ok().filter(|_| stdout_is_tty);
    let Some((cols, rows)) = size.filter(|_| !cli.no_pager) else {
        let width = size.map_or(DEFAULT_WIDTH, |(cols, _)| cols as usize);
        return print(&document, width, color, hyperlinks);
    };
    // Short piped input is printed directly too: give it a moment to end, or to
    // fill the screen.
    let mut streams_done = true;
    for index in 0..document.files.len() {
        if let Some(stream) = document.stream(index) {
            let (_, done) = stream.wait_for(rows as usize, Some(FIT_WAIT));
            streams_done &= done;
        }
    }
    // Leave a row free so the shell prompt does not push the first line off screen.
    // Counting rows needs no highlighting, so big files go straight to the pager.
    if !cli.always_page && streams_done && document.fits(cols as usize, rows as usize) {
        return print(&document, cols as usize, color, hyperlinks);
    }
    // After `v`, the edited file is loaded again the same way.
    let reload = |path: &std::path::Path| -> Option<File> {
        let origin = Origin::File(path.to_path_buf());
        let source = Source::load(origin.clone()).ok()?;
        if source.is_binary() && !cli.force {
            return None;
        }
        Some(text_file(
            cli,
            &Naming::of(&origin),
            source.text(),
            settings,
        ))
    };
    pager::run(
        document,
        &pager::Options {
            exit_at_eof: cli.exit_at_eof,
            color,
            hyperlinks,
            mouse: cli.mouse,
            chop: cli.chop_long_lines,
            watch: cli.watch,
            reload: Some(&reload),
        },
    )
}

/// Render width when there is no terminal to measure.
const DEFAULT_WIDTH: usize = 80;

/// How long to wait for piped input to end before deciding whether it fits.
const FIT_WAIT: Duration = Duration::from_millis(300);

/// Print every file without the pager. Streamed input is copied as it arrives
/// (flushed after each batch), so `mmore` works in the middle of a pipeline.
fn print(document: &Document, width: usize, color: ColorMode, hyperlinks: bool) -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    let several = document.files.len() > 1;
    for (index, file) in document.files.iter().enumerate() {
        if several {
            for line in document::header(&file.name) {
                writeln!(out, "{}", ansi::render(&line, color, None))?;
            }
        }
        if let Some(stream) = &file.stream {
            let gutter = document.gutter_width(index);
            let mut printed = 0;
            loop {
                let (_, done) = stream.wait_for(printed + 1, None);
                for line in stream.lines_from(printed) {
                    printed += 1;
                    let line = if gutter > 0 {
                        document::numbered(line, Some(printed), gutter)
                    } else {
                        line
                    };
                    writeln!(out, "{}", ansi::render(&line, color, None))?;
                }
                out.flush()?;
                if done && printed >= stream.len() {
                    break;
                }
            }
            continue;
        }
        if let Section::Code(lazy) = &file.content {
            // Write each line as soon as it is highlighted, instead of first
            // highlighting the whole file.
            let gutter = document.gutter_width(index);
            let mut number = 0;
            let mut result = Ok(());
            lazy.for_each_line(|line| {
                number += 1;
                let line = if gutter > 0 {
                    document::numbered(line, Some(number), gutter)
                } else {
                    line
                };
                result = writeln!(out, "{}", ansi::render(&line, color, None));
                result.is_ok()
            });
            result?;
            continue;
        }
        let rendered = document.render_one(index, width);
        let links = hyperlinks.then_some(rendered.links.as_slice());
        for line in &rendered.lines {
            writeln!(out, "{}", ansi::render(line, color, links))?;
        }
    }
    out.flush()
}

fn list_themes() -> io::Result<()> {
    let mut out = BufWriter::new(io::stdout().lock());
    let stdout_is_tty = io::stdout().is_terminal();
    let color = ColorMode::resolve(When::Auto, stdout_is_tty);
    let default = assets::default_theme_name(light_background(color, stdout_is_tty));
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
