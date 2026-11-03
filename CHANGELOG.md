# Changelog

All notable changes to mmore. Each release's notes on GitHub are taken from its
section here, so a version can only be released once it has one.

## [Unreleased]

### Added

- `--tabs N` (or `-x N`, as in `less`) sets the distance between tab stops.
- The README explains how to use mmore as git's pager.
- `+LINE`, `+G`, `+/PATTERN`, and `+?PATTERN` on the command line start at a line,
  the end, the first match, or the last match, as in `less` and `more`;
  `--pattern PATTERN` is the same as `+/PATTERN`.
- `-X` (`--keep-screen`) leaves the last screen in the terminal after quitting,
  so it stays in view and in the scrollback.
- `-s` (`--squeeze-blank`) shows runs of blank lines as one, as in `less` and
  `cat`, also when printing and for piped input as it arrives.

### Changed

- Long lines of plain text and code wrap between words (as Markdown always
  did) instead of anywhere; a word longer than the screen is still split.

### Fixed

- Marks, `''`, and Backspace after following a link go back to the exact row,
  not to the first row of a long (wrapped) line, which could be a row or more off.
- The help screen and the table of contents name the keys that close them (and
  choose a heading) as they are bound, instead of always `q` and `j`/`k`.

## [0.6.3] - 2026-11-03

### Fixed

- Quitting while the mouse wheel still had momentum could leave `^[OB` on the
  screen after the prompt: terminals that send the wheel as arrow keys to
  full-screen programs kept sending them after mmore had handed the terminal
  back. mmore now discards input that is still arriving before it does.

## [0.6.2] - 2026-11-03

### Fixed

- In Warp, zsh's `%` mark could still appear after quitting. Warp gives
  full-screen programs a different size and switches back a moment after they
  leave, and a shell that looked at the size in between got the full-screen
  width. When the size changed like that, mmore now waits (at most 0.3 s) for
  the size to come back before it exits.

## [0.6.1] - 2026-11-03

### Fixed

- The last line could be hidden under the status line, and the screen laid out
  for the wrong size, when the terminal changed size just as the pager started
  (as some terminals do when a program goes full screen). mmore now listens for
  size changes before it starts and checks the size again while it waits.
- When the terminal answered the background color query late (often while a web
  document was being fetched), its answer was printed on the screen, and zsh
  then showed its `%` mark after quitting. Echo now stays off until the pager
  takes over, and a late answer is dropped before mmore exits.

## [0.6.0] - 2026-11-03

### Added

- A config file (`~/.config/mmore/config`, or the file in `MMORE_CONFIG`) for
  default options and key bindings, such as `bind x quit`. The help screen lists
  the bindings.
- Documents on the web: `mmore https://...` fetches and shows a document, and a
  GitHub repository's address shows its README. Relative links in it are
  followed on the same site, and `R` fetches it again. It uses `curl`.

### Fixed

- `R` did nothing on compressed files that were being read as a stream (it
  only worked with `--watch`).

## [0.5.0] - 2026-11-03

### Added

- Shell completions (`mmore --completions zsh`, and bash, fish, elvish, and
  PowerShell) and a manual page (`mmore --man`), which the Homebrew formula
  installs.
- With `--mouse`, clicking a link follows it.
- Links can be followed in the raw source view (`r`) too, and `]`, `[`, and `t`
  work there.

### Changed

- Compressed files can be reloaded with `R` and watched with `--watch`.
- `R` on piped input says that it cannot be reloaded, instead of doing nothing.

## [0.4.1] - 2026-11-03

### Fixed

- A stray `%` could appear in zsh after quitting, in terminals that do not put
  the cursor back where it was when the pager screen closes. mmore now goes back
  to the start of the line itself.
- A terminal that answered the background color query late (after mmore had
  stopped waiting) had its reply read as key presses, which could switch to the
  raw view or open a search prompt that swallowed `q`. Such replies are ignored.

## [0.4.0] - 2026-11-02

### Added

- Compressed files (gzip, bzip2, xz, and zstd) are shown decompressed, and
  `guide.md.gz` is rendered as Markdown.
- With several files, searching goes on into the next (or previous) file once the
  current one has no more matches.
- Links in Markdown can be followed: `Tab` and `Shift-Tab` select one, `Enter`
  goes to the heading or local file it points to (or opens a web page in the
  browser), and `Backspace` goes back.

## [0.3.0] - 2026-11-02

### Added

- `]` and `[` jump to the next and previous heading in Markdown, and `t` opens a
  table of contents to choose one from.
- `--watch` reloads the file whenever it changes on disk, to preview a document
  while editing it or keep up with a log; at the end of the file it stays at the
  end. `R` reloads by hand.
- Marks, as in `less`: `m` and a letter marks the current place (in any file),
  `'` and the letter goes back to it, and `''` returns to where you were before
  the last jump (`g`, `G`, `%`, a search, a heading, or a mark).
- Search as you type: while typing after `/` or `?`, the view jumps to the first
  match and highlights all of them; Enter keeps the search, Esc goes back.

### Changed

- The switch between the rendered view and the raw source moved from `m` to `r`,
  since `m` now sets marks as in `less`.

## [0.2.0] - 2026-11-02

### Added

- Colors and attributes in plain text and piped input are shown, like `less -R`:
  output from `git log --color`, `ls --color`, compilers, and `man` pages (bold
  and underline from backspace overstriking). The codes become mmore's own
  styles, so nothing from the input reaches the terminal raw. `--show-escapes`
  shows them as text instead.
- Counts before keys, as in `less`: `120g` goes to line 120, `50%` halfway, and
  `5j` moves 5 lines.
- `-S` (`--chop-long-lines`), or `w` while paging, cuts long lines off instead of
  wrapping them; Left and Right scroll sideways, and search scrolls to the match.
- `v` opens the file in `$VISUAL` or `$EDITOR` at the current line and reloads it
  afterwards.
- `&pattern` shows only matching lines (`&` and Enter shows all again). It also
  applies to piped input as it arrives.
- Named pipes and process substitution (`mmore <(make 2>&1)`) are shown while
  they are being written, like piped stdin.

### Changed

- Printing a code file with colors writes each line as soon as it is highlighted:
  on a 12 MB file the first line comes after 0.18 s instead of 22.5 s.
- Releases run the full CI (including the terminal checks) on the tagged commit
  before anything is built.

### Fixed

- Keys pressed right after starting (while mmore asks the terminal for its
  background color) are no longer lost.

## [0.1.0] - 2026-11-02

First release.

- A pager in the spirit of `more`, with search, several files, line numbers,
  the mouse wheel (`--mouse`), suspending with Ctrl-Z, and a help screen.
- Markdown rendered in the terminal: headings, lists, task lists, block quotes,
  GitHub alerts, tables that fit the width, code blocks with highlighting,
  footnotes, front matter, images, and clickable links.
- Syntax highlighting for the languages in `mmore --list-languages`, done in the
  background so large files open at once.
- Piped input shown as it arrives, with `F` to follow it.
- Light or dark theme picked from the terminal's background.
- Builds for macOS and Linux, and a Homebrew formula.

[Unreleased]: https://github.com/houseofmackee/mmore/compare/v0.6.3...HEAD
[0.6.3]: https://github.com/houseofmackee/mmore/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/houseofmackee/mmore/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/houseofmackee/mmore/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/houseofmackee/mmore/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/houseofmackee/mmore/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/houseofmackee/mmore/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/houseofmackee/mmore/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/houseofmackee/mmore/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/houseofmackee/mmore/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/houseofmackee/mmore/releases/tag/v0.1.0
