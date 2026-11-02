# mmore: implementation plan

`mmore` is a terminal pager in the spirit of `more`, written in Rust. It pages plain
text like `more` does, renders Markdown files into styled terminal output, and
syntax-highlights source code for common languages.

## 1. Goals and non-goals

### Goals

- Drop-in feel for everyday `more` usage: `mmore FILE...`, `cmd | mmore`, Space to page,
  `q` to quit.
- Best-effort Markdown rendering (CommonMark plus GitHub extensions) that looks good in a
  typical 256-color or truecolor terminal.
- Syntax highlighting for code files and for fenced code blocks inside Markdown.
- Sensible behavior when not attached to a terminal: print rendered (or plain) output and
  exit, like `cat`.
- Single static binary, fast startup, no runtime dependencies.

### Non-goals (for v1)

- Full `less` feature parity (marks, tags, follow mode, editing hooks).
- Pixel-accurate Markdown (images, math, Mermaid diagrams).
- Windows legacy console support beyond what `crossterm` provides for free.

## 2. Tech stack

| Concern              | Crate                                   | Notes                                                       |
|----------------------|-----------------------------------------|-------------------------------------------------------------|
| CLI parsing          | `clap` (derive)                         | Help, version, and shell completions                        |
| Markdown parsing     | `pulldown-cmark`                        | Event stream; enable tables, strikethrough, task lists, footnotes |
| Syntax highlighting  | `syntect` (`default-fancy` feature)     | Pure-Rust regex backend, bundled Sublime syntaxes and themes |
| Terminal control     | `crossterm` (`use-dev-tty` feature)     | Raw mode, alternate screen, key and resize events. The feature is required on macOS: the default kqueue reader cannot poll `/dev/tty` when stdin is a pipe |
| Text width/wrapping  | `unicode-width`, `unicode-segmentation` | Correct width for CJK and emoji when wrapping               |
| Terminal detection   | `std::io::IsTerminal`                   | Decide between pager mode and passthrough                  |
| Errors               | `anyhow`                                | Simple error propagation in the binary                     |
| Testing              | `insta`, `assert_cmd`                   | Snapshot tests for rendering, integration tests for the CLI |

## 3. Architecture

```
          +-----------+     +----------------+     +-------------+     +---------+
input --> |  Source   | --> |   Renderer     | --> |  Document   | --> |  Pager  | --> terminal
          | file/stdin|     | md / code /    |     | Vec<Line>   |     | or      |
          +-----------+     | plain          |     | (styled)    |     | dump    |
                            +----------------+     +-------------+     +---------+
```

### Core types

```rust
struct Span { text: String, style: Style }        // Style = fg, bg, bold, italic, underline, dim, strike, link
struct Line { spans: Vec<Span>, source_line: usize }
struct Document { lines: Vec<Line>, title: String }

trait Renderer {
    fn render(&self, input: &str, width: u16) -> Document;
}
```

- Renderers produce already-wrapped lines for a given width. On terminal resize the
  pager asks the renderer to re-render at the new width (cheap for typical file sizes;
  cache the parsed AST/event list to avoid re-parsing).
- `Style` is our own small struct, converted to ANSI escapes in one place
  (`ansi.rs`). This keeps renderers independent from `crossterm` and makes snapshot
  tests readable (we can snapshot a debug form instead of raw escapes).

### Module layout

```
src/
  main.rs          entry point, wires CLI -> sources -> renderers -> output
  cli.rs           clap definitions
  source.rs        reading files/stdin, size limits, binary detection, UTF-8 lossy decode
  detect.rs        choose renderer: extension, --language flag, shebang, first-line sniffing
  style.rs         Style, Span, Line, Document
  ansi.rs          Style -> ANSI SGR, color-depth downgrade (truecolor -> 256 -> 16 -> none)
  wrap.rs          width-aware word wrapping of span lists, with hanging indent support
  render/
    mod.rs
    plain.rs       plain text (tabs expanded, control chars made visible)
    code.rs        syntect-based highlighting
    markdown.rs    pulldown-cmark event walker
    table.rs       Markdown table layout
  pager/
    mod.rs         event loop, screen drawing, status line
    keys.rs        key bindings -> actions
    search.rs      regex search over rendered plain text
  theme.rs         color palette for Markdown elements, syntect theme selection
```

## 4. Input handling and detection

1. Inputs: zero or more file paths; `-` or no args with piped stdin means stdin.
   No args and stdin is a TTY: print usage and exit with code 1 (matches `more`).
2. Binary detection: if the first 8 KiB contain NUL bytes, show
   `******** FILE: Not a text file ********` and skip, unless `--force` is given.
3. Renderer selection, in order:
   1. `--plain` forces plain text.
   2. `--language LANG` forces a syntect syntax (or `markdown`).
   3. Extension `.md`, `.markdown`, `.mdown`, `.mkd`, `.mdx` selects Markdown.
   4. syntect lookup by extension, then by first line (shebangs, `<?xml`, modelines).
   5. Fallback: plain.
4. Stdin has no extension, so it uses first-line detection, or `--language`.

## 5. Markdown rendering spec

Walk the `pulldown-cmark` event stream with a style stack and a block-context stack
(for indentation prefixes such as quote bars and list indents).

| Element               | Rendering                                                                                      |
|-----------------------|------------------------------------------------------------------------------------------------|
| H1                    | Bold, bright color, full-width underline of `═`; blank line after                             |
| H2                    | Bold, color, underline of `─`                                                                  |
| H3 to H6              | Bold with decreasing emphasis; `###` style prefix kept dim for orientation                    |
| Paragraph             | Word-wrapped to width (capped at `--max-width`, default 100) with blank line after            |
| Emphasis / strong     | Italic / bold (italic falls back to underline if the terminal lacks it; configurable)         |
| Strikethrough         | SGR 9                                                                                          |
| Inline code           | Distinct fg plus subtle bg, padded with one space each side                                   |
| Fenced code block     | Highlighted with syntect using the info string; boxed or indented with bg shade; language label in the top border |
| Indented code block   | Same, without highlighting                                                                     |
| Block quote           | `│ ` prefix in dim color, nested quotes stack prefixes; GitHub alerts (`> [!NOTE]`) get a colored label |
| Unordered list        | `•`, `◦`, `▪` by nesting depth, hanging indent for wrapped lines                               |
| Ordered list          | Right-aligned numbers, honoring start number                                                  |
| Task list             | `☐` / `☑` (ASCII `[ ]` / `[x]` with `--ascii`)                                                 |
| Table                 | Box-drawing borders, header in bold, column alignment from `:---:` markers. If too wide, shrink columns proportionally and wrap cell text; as last resort, render each row as a "key: value" record |
| Horizontal rule       | Dim `─` line across the content width                                                         |
| Link                  | Underlined text; URL shown dim in parentheses after it unless text equals URL. Emit OSC 8 hyperlinks for absolute URLs when the terminal supports them (`--hyperlinks auto/always/never`); relative paths and anchors are not clickable |
| Image                 | `🖼 alt text` (or `[image: alt]` in ASCII mode) plus dim URL                                    |
| Footnotes             | `[^1]` as superscript-ish dim marker; definitions rendered at their position with a label     |
| HTML blocks / inline  | Passed through dim; `<br>` becomes a line break; `<!-- comments -->` hidden                    |
| YAML/TOML front matter| Rendered as a dim, highlighted block at the top (toggle with `--no-front-matter`)              |
| Hard/soft breaks      | Hard break = newline; soft break = space                                                       |

## 6. Syntax highlighting

- Start from syntect's built-in syntaxes (Rust, Python, JavaScript, Go, C, C++, Java,
  C#, Ruby, PHP, shell, SQL, HTML, CSS, JSON, YAML, Markdown, Makefile, and more).
- Add extra `.sublime-syntax` files from `assets/syntaxes` (TypeScript and TOML so far,
  with their sources and licenses recorded there). `build.rs` merges them with the
  built-in set into an uncompressed bundle embedded in the binary, which loads in under
  1 ms. A few languages without a grammar map to a close one (Kotlin to Java).
- Themes: default `base16-ocean.dark`; detect light backgrounds through `COLORFGBG` or an
  OSC 11 query (best effort, with timeout) and pick a light theme. `--theme NAME` and
  `--list-themes` flags.
- Highlight lazily: a background thread highlights each code file from the top (syntect
  state must be carried forward line by line) and publishes lines in batches. The pager
  shows lines plain until their highlighted version is ready, then swaps it in at draw
  time; highlighting never changes the text, so wrapping and scrolling are unaffected.
  The first screen waits up to 50 ms so small files open already colored. Highlighting
  stops at a line over 20,000 bytes (minified code); the rest stays plain.
- Map syntect colors to our `Style`, then downgrade color depth according to
  `COLORTERM` / `TERM`.

## 7. Pager behavior

### Modes

- **Pager mode**: stdout is a TTY. Enter alternate screen and raw mode, draw a page,
  show a status line, handle keys.
- **Dump mode**: stdout is not a TTY (or `--no-pager`). Write rendered output with colors
  only if `--color=always`; default `auto` means no ANSI when piped. Respect `NO_COLOR`.
- **Fits on screen**: if the whole document fits, print it and exit without entering the
  alternate screen (configurable via `--always-page`). Matches classic `more` feel.

### Key bindings (more-compatible, with a few `less` extras)

| Key                         | Action                                       |
|-----------------------------|----------------------------------------------|
| `Space`, `f`, `PgDn`        | Next page (at the end of a file: next file)  |
| `b`, `PgUp`                 | Previous page                                |
| `Enter`, `j`, `↓`           | Next line                                    |
| `k`, `↑`                    | Previous line                                |
| `d` / `u`                   | Half page down / up                          |
| `g`, `Home` / `G`, `End`    | Top / bottom                                 |
| `/pattern`, `?pattern`      | Search forward / backward (regex, smart case)|
| `n` / `N`                   | Next / previous match                        |
| `=`                         | Show current line number                     |
| `:n` / `:p`                 | Next / previous file                         |
| `m`                         | Toggle rendered vs. raw source view          |
| `h`                         | Help screen                                  |
| `q`, `Q`, `Ctrl-C`          | Quit                                         |

- Status line: `--More-- (42%)` in reverse video, like `more`, plus filename when paging
  multiple files. While searching, show the prompt in the status line.
- Search matches against the plain text of rendered lines and highlights hits in reverse
  video.
- Mouse wheel scrolling via `crossterm` mouse capture (off by default, `--mouse` to
  enable, since capture breaks terminal text selection).
- Always restore the terminal on exit, including on panic (install a panic hook that
  leaves raw mode and the alternate screen first).

## 8. CLI surface

```
mmore [OPTIONS] [FILE]...

Options:
  -p, --plain              Disable Markdown rendering and highlighting
  -l, --language <LANG>    Force syntax (e.g. rust, python, markdown)
  -n, --line-numbers       Show source line numbers in the gutter (in rendered Markdown,
                           numbers mark where each block starts)
      --theme <NAME>       Syntect theme
      --list-themes        List available themes and exit
      --list-languages     List supported languages and exit
      --color <WHEN>       auto | always | never  [default: auto]
      --hyperlinks <WHEN>  auto | always | never  [default: auto]
      --width <N>          Override render width
      --max-width <N>      Cap prose width for Markdown [default: 100]
      --ascii              Use ASCII instead of Unicode symbols
      --no-pager           Print to stdout without paging
      --always-page        Page even if content fits on one screen
      --force              Display binary files anyway
  -h, --help
  -V, --version
```

Environment: `MMORE_OPTS` (default flags, parsed before argv), `NO_COLOR`, `COLORTERM`,
`COLORFGBG`. A config file (`~/.config/mmore/config.toml`) is a later milestone.

## 9. Testing strategy

- **Unit tests**: wrapping (width math, hanging indents, wide chars), color downgrade,
  detection rules, key mapping.
- **Snapshot tests** (`insta`): a corpus in `tests/fixtures/` of Markdown files covering
  every element in section 5, rendered at widths 40, 80, and 120. Snapshot a readable
  debug form (`[bold]Heading[/]`) rather than raw ANSI.
- **Integration tests** (`assert_cmd`): dump mode output, `--plain`, stdin input, exit
  codes, binary file handling, `NO_COLOR`.
- **Manual pager checklist**: resize, search, multi-file navigation, Ctrl-C restore,
  piping into `head`.
- CI: GitHub Actions running `cargo fmt --check`, `cargo clippy -D warnings`, and
  `cargo test` on macOS and Linux.

## 10. Milestones

1. **Skeleton**: `cargo init`, clap CLI, file and stdin reading, dump mode with plain text.
2. **Pager core**: raw mode, alternate screen, page/line scrolling, status line, quit,
   resize handling, panic-safe terminal restore, fits-on-screen exit.
3. **Code highlighting**: syntect integration, detection, themes, color downgrade.
4. **Markdown, part 1**: headings, paragraphs, emphasis, inline code, code blocks, lists,
   quotes, rules, links.
5. **Markdown, part 2**: tables, task lists, footnotes, images, HTML, front matter, GitHub
   alerts, OSC 8 hyperlinks.
6. **Pager extras**: search, multi-file `:n`/`:p`, line numbers, raw/rendered toggle, help
   screen.
7. **Polish**: light-theme detection, `MMORE_OPTS`, lazy highlighting for large files,
   README, release builds (`cargo install`, Homebrew tap).

**Status (2026-11-02):** milestones 1 to 7 are done, except the Homebrew tap, which
needs a published repository. Added since: mouse wheel scrolling (`--mouse`), CI on
GitHub Actions (including the pseudo-terminal checks), showing piped input as it
arrives with `F` to follow it, and Ctrl-Z to suspend. Not yet done from the plan: the
config file (`~/.config/mmore/config.toml`).

## 11. Risks and open questions

- **Large files**: rendering everything up front is simple but slow for multi-MB logs.
  Plan: plain and code renderers stream and render lazily; Markdown stays whole-document
  (Markdown files are rarely huge).
- **Terminal capability detection** (italics, truecolor, OSC 8) is heuristic; provide
  flags to override.
- **Wide tables** have no perfect answer in a narrow terminal; the shrink, wrap, and
  record-view fallback chain is best effort.
- **Exit at end of file**: classic `more` exits when you page past the end; `less` does
  not. Proposal: stay open at EOF and show `--More-- (END)`, with `-e` to exit at EOF.
