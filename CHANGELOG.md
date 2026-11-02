# Changelog

All notable changes to mmore. Each release's notes on GitHub are taken from its
section here, so a version can only be released once it has one.

## [Unreleased]

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

[Unreleased]: https://github.com/houseofmackee/mmore/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/houseofmackee/mmore/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/houseofmackee/mmore/releases/tag/v0.1.0
