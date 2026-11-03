use std::path::PathBuf;

use clap::Parser;

use std::env;
use std::ffi::OsString;

use crate::ansi::When;
use crate::pager::keys::Keymap;

/// Environment variables, for the manual page (`AFTER_HELP` lists them too).
const ENVIRONMENT: &[(&str, &str)] = &[
    (
        "MMORE_OPTS",
        "Default options, read before the command line (e.g. \"-n --theme InspiredGitHub\")",
    ),
    (
        "NO_COLOR",
        "Disable colors (text attributes such as bold are kept)",
    ),
    (
        "COLORFGBG",
        "Terminal colors; a light background selects a light theme",
    ),
    ("VISUAL, EDITOR", "The editor that v opens"),
    (
        "MMORE_CONFIG",
        "The config file (empty for none); by default $XDG_CONFIG_HOME/mmore/config or ~/.config/mmore/config",
    ),
];

const AFTER_HELP: &str = concat!(
    "Environment:\n",
    "  MMORE_OPTS   Default options, read before the command line (e.g. \"-n --theme InspiredGitHub\")\n",
    "  NO_COLOR     Disable colors (text attributes such as bold are kept)\n",
    "  COLORFGBG    Terminal colors; a light background selects a light theme\n",
    "  VISUAL, EDITOR   The editor that v opens\n",
    "  MMORE_CONFIG Config file (empty for none; default ~/.config/mmore/config)\n",
    "\n",
    "Config file: options as on the command line, one or more per line, and\n",
    "\"bind KEY ACTION\" lines (for example \"bind x quit\"); # starts a comment.\n",
    "\n",
    "Press h while paging for the list of keys."
);

/// Command-line arguments with the options from the config file and then
/// `MMORE_OPTS` in front, so that later ones win: the config file, then
/// `MMORE_OPTS`, then the command line.
pub fn args(config: &[String]) -> Vec<OsString> {
    let mut args: Vec<OsString> = env::args_os().collect();
    let mut extra: Vec<String> = config.to_vec();
    if let Some(opts) = env::var_os("MMORE_OPTS") {
        extra.extend(split_words(&opts.to_string_lossy()));
    }
    let at = args.len().min(1);
    args.splice(at..at, extra.into_iter().map(OsString::from));
    args
}

/// What the config file sets.
#[derive(Debug, Default)]
pub struct Config {
    /// Options, as on the command line.
    pub args: Vec<String>,
    pub keymap: Keymap,
}

/// Where the config file is: `MMORE_CONFIG` if set (empty for none), otherwise
/// `mmore/config` in `$XDG_CONFIG_HOME` or `~/.config`. Also whether the
/// location was given explicitly (then it must exist).
pub fn config_path() -> Option<(PathBuf, bool)> {
    if let Some(path) = env::var_os("MMORE_CONFIG") {
        return (!path.is_empty()).then(|| (PathBuf::from(path), true));
    }
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some((base.join("mmore").join("config"), false))
}

/// Read the config file, if there is one.
pub fn load_config() -> Result<Config, String> {
    let Some((path, explicit)) = config_path() else {
        return Ok(Config::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_config(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if !explicit && e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Each line is an option as on the command line (`--theme InspiredGitHub`,
/// `-n`), or `bind KEY ACTION`. `#` starts a comment line.
pub fn parse_config(text: &str) -> Result<Config, String> {
    let mut config = Config::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words = split_words(line);
        let fail = |e: String| format!("line {}: {e}", n + 1);
        match words.first().map(String::as_str) {
            Some("bind") => match &words[1..] {
                [key, action] => config.keymap.bind(key, action).map_err(fail)?,
                _ => return Err(fail("expected bind KEY ACTION".into())),
            },
            Some(word) if word.starts_with('-') => config.args.extend(words),
            _ => {
                return Err(fail(format!(
                    "expected an option (like --theme NAME) or bind KEY ACTION, not '{line}'"
                )));
            }
        }
    }
    Ok(config)
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
    /// Files to display: names, web addresses (http or https), or `-` for stdin.
    /// Reads stdin when no files are given.
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

    /// Cut long lines off at the screen edge instead of wrapping them (scroll
    /// sideways with Left and Right; w toggles while paging).
    #[arg(short = 'S', long)]
    pub chop_long_lines: bool,

    /// Columns between tab stops.
    #[arg(short = 'x', long, value_name = "N", default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=32))]
    pub tabs: u8,

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

    /// Scroll with the mouse wheel and follow links by clicking. The terminal then
    /// sends clicks to mmore; to select text, drag while holding Shift in most
    /// terminals (Option in iTerm2).
    #[arg(long)]
    pub mouse: bool,

    /// Show escape sequences in plain text as codes (^[[31m) instead of colors.
    #[arg(long)]
    pub show_escapes: bool,

    /// Reload the file when it changes on disk (to preview a document while
    /// editing it, or to keep up with a log file).
    #[arg(long)]
    pub watch: bool,

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

/// What packagers run instead of the pager: `mmore --completions SHELL` and
/// `mmore --man`. These are not regular options, so they stay out of the help,
/// the completions, and the manual page.
pub enum Packaging {
    Completions(clap_complete::Shell),
    Man,
}

impl Packaging {
    /// From the real command line (`MMORE_OPTS` does not apply). An unknown
    /// shell is an error.
    pub fn from_args() -> Option<Result<Packaging, String>> {
        let args: Vec<String> = env::args().skip(1).collect();
        match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["--man"] => Some(Ok(Packaging::Man)),
            ["--completions", shell] => {
                Some(shell.parse().map(Packaging::Completions).map_err(|_| {
                    format!("unknown shell '{shell}' (bash, elvish, fish, powershell, zsh)")
                }))
            }
            _ => None,
        }
    }
}

/// Write the completion script for `shell`.
pub fn completions(shell: clap_complete::Shell, out: &mut impl std::io::Write) {
    use clap::CommandFactory;
    clap_complete::generate(shell, &mut Cli::command(), "mmore", out);
}

/// Write the manual page, with the keys and environment as extra sections.
pub fn man_page(out: &mut impl std::io::Write) -> std::io::Result<()> {
    use clap::CommandFactory;
    let man = clap_mangen::Man::new(Cli::command());
    man.render_title(out)?;
    man.render_name_section(out)?;
    man.render_synopsis_section(out)?;
    man.render_description_section(out)?;
    man.render_options_section(out)?;
    // Tagged paragraphs: the key (or variable), then what it does.
    writeln!(out, ".SH KEYS")?;
    for (keys, what) in crate::pager::help::KEYS {
        let keys = keys.split("  ").collect::<Vec<_>>().join(", ");
        writeln!(out, ".TP\n\\fB{}\\fR\n{}", roff(&keys), roff(what))?;
    }
    writeln!(out, ".SH ENVIRONMENT")?;
    for (name, what) in ENVIRONMENT {
        writeln!(out, ".TP\n\\fB{}\\fR\n{}", roff(name), roff(what))?;
    }
    writeln!(out, ".SH FILES")?;
    writeln!(out, ".TP\n\\fB~/.config/mmore/config\\fR")?;
    writeln!(
        out,
        "{}",
        roff(
            "Defaults and key bindings (in $XDG_CONFIG_HOME/mmore/config if that is set, or the file in MMORE_CONFIG). \
             Each line is options as on the command line, or bind KEY ACTION; # starts a comment line. \
             The actions are: "
        )
    )?;
    let actions: Vec<&str> = crate::pager::keys::ACTIONS
        .iter()
        .map(|(n, _)| *n)
        .collect();
    writeln!(out, "{}.", roff(&actions.join(", ")))?;
    writeln!(out, ".SH SEE ALSO")?;
    writeln!(out, "less(1), more(1)")?;
    man.render_version_section(out)
}

/// Escape text for roff: backslashes, and a leading `.` or `'` (control lines).
fn roff(s: &str) -> String {
    let s = s.replace('\\', "\\e").replace('-', "\\-");
    if s.starts_with('.') || s.starts_with('\'') {
        format!("\\&{s}")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_lines() {
        let config = parse_config(
            "# defaults\n\n-n\n--theme \"Solarized (dark)\"\n  bind x quit\nbind Ctrl-d page-down\n",
        )
        .unwrap();
        assert_eq!(config.args, vec!["-n", "--theme", "Solarized (dark)"]);
        let bound: Vec<_> = config.keymap.bindings().collect();
        assert_eq!(bound, vec![("x", "quit"), ("Ctrl-d", "page-down")]);
    }

    #[test]
    fn config_errors_name_the_line() {
        assert_eq!(
            parse_config("-n\ntheme x\n").unwrap_err(),
            "line 2: expected an option (like --theme NAME) or bind KEY ACTION, not 'theme x'"
        );
        assert_eq!(
            parse_config("bind x\n").unwrap_err(),
            "line 1: expected bind KEY ACTION"
        );
        assert_eq!(
            parse_config("\n\nbind Hyper-x quit").unwrap_err(),
            "line 3: unknown key 'Hyper-x'"
        );
    }

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
