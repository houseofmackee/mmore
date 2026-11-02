# mmore

[![CI](https://github.com/houseofmackee/mmore/actions/workflows/ci.yml/badge.svg)](https://github.com/houseofmackee/mmore/actions/workflows/ci.yml)

`mmore` is a terminal pager in the spirit of `more`. It pages plain text like
`more` does, renders Markdown files as formatted text, and syntax-highlights source
code.

```sh
mmore README.md          # rendered Markdown
mmore src/main.rs        # highlighted code
git log | mmore          # paging piped input
mmore -n a.txt b.txt     # several files, with line numbers
```

## Install

With [Homebrew](https://brew.sh), on macOS or Linux:

```sh
brew install houseofmackee/tap/mmore
```

Or download the archive for your platform from the
[latest release](https://github.com/houseofmackee/mmore/releases/latest), check it
against `SHA256SUMS.txt`, and put `mmore` somewhere on your `PATH`. There are
builds for macOS (Apple silicon and Intel) and Linux (x86_64 and ARM); the Linux
ones are statically linked and run on any distribution.

To build it yourself, you need a Rust toolchain (`cargo`). From a checkout of this
repository:

```sh
cargo install --path .
```

This builds an optimized binary and puts it in `~/.cargo/bin`. To install it
somewhere else, add `--root DIR` (the binary goes in `DIR/bin`).

## Features

- **Pager**: `more`-style keys plus a few from `less`, search, several files, a
  help screen (press `h`), and a status line with your position.
- **Markdown**: headings, emphasis, lists, task lists, block quotes, GitHub alerts,
  code blocks with highlighting, tables that fit the terminal width, footnotes,
  images, front matter, and clickable links.
- **Syntax highlighting** for the languages listed by `mmore --list-languages`,
  including Rust, Python, JavaScript, TypeScript, Go, C, C++, Java, Ruby, shell,
  SQL, HTML, CSS, JSON, YAML, and TOML. Large files open at once; colors fill in
  from the top while you read.
- **Fits the terminal**: everything re-flows when the window is resized, and
  output that fits on one screen is printed without starting the pager.
- **Colors in the input**: output from `git log --color`, `ls --color`, compilers,
  and `man` keeps its colors, bold, and underline, like `less -R`.
- **Plays well with pipes**: piped input is shown while it is still arriving
  (`F` follows new input, like `less`), and when the output is not a terminal,
  `mmore` prints plain text, like `cat`.

## Keys

| Key | Action |
|-----|--------|
| `Space`, `f`, `PgDn` | Next page (at the end of a file: next file) |
| `b`, `PgUp` | Previous page |
| `Enter`, `j`, `Down` | Next line |
| `k`, `Up` | Previous line |
| `d` / `u` | Half a page down / up |
| `g`, `Home` / `G`, `End` | Top / bottom |
| `120g`, `50%` | Go to line 120, or halfway (type the number first; `5j` moves 5 lines) |
| `w` | Wrap long lines, or cut them off at the screen edge |
| `Left` / `Right` | Scroll sideways when long lines are cut off |
| Mouse wheel | Scroll 3 lines (with `--mouse`) |
| `F` | Follow new input at the end, as with `tail -f` (any key stops) |
| `/pattern`, `?pattern` | Search forward / backward |
| `&pattern` | Show only lines that match (`&` and Enter shows all again) |
| `n` / `N` | Next / previous match |
| `Esc` | Clear search highlighting |
| `:n` / `:p` | Next / previous file |
| `=`, `:f`, `Ctrl-G` | Show the file name and line number |
| `m` | Switch between the rendered view and the raw source |
| `v` | Edit the file in `$VISUAL` or `$EDITOR`, at the current line, then reload it |
| `h` | Help |
| `Ctrl-Z` | Suspend (`fg` resumes) |
| `q`, `:q`, `Ctrl-C` | Quit |

Searches and filters are regular expressions. They ignore case unless the pattern
contains an uppercase letter. An empty search pattern repeats the previous search.
A filter also applies to piped input as it arrives, so `tail -f app.log | mmore`
followed by `&error` shows new errors only.

## Options

| Option | Effect |
|--------|--------|
| `-n`, `--line-numbers` | Show source line numbers (in rendered Markdown, the line where each block starts) |
| `-S`, `--chop-long-lines` | Cut long lines off at the screen edge instead of wrapping them |
| `-p`, `--plain` | Show files as plain text, without rendering or highlighting |
| `-l`, `--language LANG` | Treat the input as `LANG`, by name or extension (`rust`, `py`, `md`) |
| `--theme NAME` | Highlighting theme; `--list-themes` shows the choices |
| `--color WHEN` | `auto`, `always`, or `never` |
| `--hyperlinks WHEN` | Clickable links: `auto`, `always`, or `never` |
| `--width N`, `--max-width N` | Render width, and the cap for Markdown (default 100) |
| `--ascii` | Draw Markdown structure with ASCII characters only |
| `--no-front-matter` | Hide YAML or TOML front matter |
| `-e`, `--exit-at-eof` | Quit when paging past the end, like classic `more` |
| `--no-pager`, `--always-page` | Never page, or page even short input |
| `--mouse` | Scroll with the mouse wheel (see below) |
| `--show-escapes` | Show escape codes in plain text (`^[[31m`) instead of their colors |
| `--force` | Show binary files anyway |

Run `mmore --help` for the full list.

### Defaults with `MMORE_OPTS`

Options in the `MMORE_OPTS` environment variable apply to every run. Options given
on the command line take precedence:

```sh
export MMORE_OPTS='-n --theme "Solarized (dark)"'
mmore notes.md                    # line numbers, Solarized
mmore --theme InspiredGitHub x.rs # line numbers, InspiredGitHub
```

### Mouse wheel

With `--mouse`, the wheel scrolls three lines at a time. The terminal then sends
mouse clicks to `mmore` instead of selecting text, so it is off by default. To
select text while it is on, drag while holding Shift in most terminals (Option in
iTerm2). To turn it on for good, add it to `MMORE_OPTS`.

Some terminals scroll pagers with the wheel even without `--mouse`, by turning
wheel turns into arrow keys. In those, text selection keeps working as usual.

## Colors and themes

`mmore` uses 24-bit color when the terminal advertises it (`COLORTERM=truecolor`),
otherwise 256 or 16 colors. Markdown uses the terminal's own palette, so it matches
your color scheme.

Without `--theme`, code is highlighted with `base16-ocean.dark`, or `InspiredGitHub`
on a light background. To tell which it is, `mmore` checks `COLORFGBG`, then asks
the terminal for its background color. If neither works, or over SSH, it assumes a
dark background; set `--theme` (for example in `MMORE_OPTS`) to choose.

Set `NO_COLOR` to turn colors off. Bold, underline, and reverse video are kept, so
headings and search matches stay visible.

## Clickable links

Links in Markdown become clickable (OSC 8 hyperlinks) in terminals known to
support them, including iTerm2, WezTerm, Warp, kitty, Ghostty, foot, Konsole,
Windows Terminal, and VS Code. Only absolute URLs (such as `https://` and `mailto:`)
are clickable; relative paths are shown but cannot be opened from a terminal. Use
`--hyperlinks always` or `never` to override the detection.

## Piped input

Input from a pipe is shown as it arrives, so `make 2>&1 | mmore` starts paging
right away and `tail -f app.log | mmore` keeps growing; press `F` to keep the newest
lines in view. The same goes for pipes given as file names, such as named pipes
(FIFOs) and process substitution: `mmore <(make 2>&1)`. While input is still arriving, the status line shows
`(waiting for input)` at the end instead of a percentage. Piped input is shown as
plain text, unless its first line names a language (`#!/usr/bin/env python3`),
in which case `mmore` waits for the end and highlights it. Line numbers (`-n`)
for piped input always leave room for five digits, since its length is unknown.

## When output is not a terminal

If the output goes to a pipe or a file, `mmore` prints plain text: no rendering,
no colors, and no pager. Markdown and code are printed as written; color codes in
plain text are removed, so `mmore build.log | grep error` matches the text you
see. Tabs become spaces and other control characters are shown as `^X`. Piped
input is copied through as it arrives, so `tail -f app.log | mmore | grep error`
works. Add `--color always` to get rendered, colored output anyway.

## Colors in the input

Plain text keeps the colors and attributes it was written with: color codes from
tools such as `git`, `ls`, `grep --color`, and compilers, and the backspace
overstriking that `man` uses for bold and underline. The codes are turned into
`mmore`'s own styles rather than passed to the terminal, so search, wrapping, and
widths work on the visible text, and nothing in the input can move the cursor or
change terminal settings. Other control sequences (such as erase line in progress
output) are removed; unknown ones are shown as `^[`. Markdown and code files are
not affected: their own rendering and highlighting apply. `--show-escapes` shows
the codes as text instead, which helps when debugging colored output.

## Development

```sh
cargo test      # unit, snapshot, and command-line tests
cargo clippy --all-targets -- -D warnings
```

`tests/pty/terminal_checks.py` runs the built binary in a pseudo-terminal to check
what unit tests cannot reach: paging keys, search, resizing, colors, hyperlinks,
the mouse, streaming piped input, suspending with Ctrl-Z (using a small
job-control shell), and the terminal background query. Run it after a build:

```sh
cargo build && python3 tests/pty/terminal_checks.py
```

GitHub Actions runs formatting, clippy, the tests, and the terminal checks on Linux
and macOS for every push to `main` and every pull request
(`.github/workflows/ci.yml`).

### Releasing

Set the version in `Cargo.toml`, commit, and push a matching tag:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` then runs the full CI on the tagged commit, and only
if it passes builds the four binaries, publishes them with checksums as a GitHub
release, and updates the formula in
[houseofmackee/homebrew-tap](https://github.com/houseofmackee/homebrew-tap). The
tap update needs the `TAP_DEPLOY_KEY` secret (a write deploy key of the tap).

Markdown rendering has snapshot tests (using `insta`) over the documents in
`tests/fixtures/markdown`. After an intended change to the output, review and
update them with `cargo insta review` (from the `cargo-insta` tool), or run the tests
with `INSTA_UPDATE=always`
and check the diff.

`PLAN.md` describes the design and the milestones it was built in.

## Credits

`mmore` is built on [syntect](https://github.com/trishume/syntect),
[pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark),
[crossterm](https://github.com/crossterm-rs/crossterm), and
[clap](https://github.com/clap-rs/clap). The bundled TypeScript and TOML grammars
come from other projects; see `assets/syntaxes/SOURCES.md` for their sources and
licenses.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

The bundled grammars in `assets/syntaxes` keep their own licenses (Apache-2.0 and
MIT); see `assets/syntaxes/SOURCES.md`.
