//! Syntax highlighting with syntect.

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::SyntaxReference;
use syntect::util::LinesWithEndings;

use super::plain;
use crate::assets;
use crate::style::{Color, Line, Style};

/// Code blocks inside Markdown larger than this are shown plain, since they are
/// highlighted while laying out the document. (Code files are highlighted lazily
/// and have no size limit.)
pub const MAX_BLOCK_BYTES: usize = 256 * 1024;
/// Very long lines (minified code) make the highlighter extremely slow.
pub const MAX_HIGHLIGHT_LINE: usize = 20_000;

/// Whether a Markdown code block is small and regular enough to highlight.
pub fn should_highlight(text: &str) -> bool {
    text.len() <= MAX_BLOCK_BYTES && text.lines().all(|l| l.len() <= MAX_HIGHLIGHT_LINE)
}

/// Highlight all of `text`. Lines that cannot be highlighted (after a very long
/// line or a highlighter error) are shown plain.
pub fn render(text: &str, syntax: &SyntaxReference, theme: &Theme) -> Vec<Line> {
    let mut lines = Vec::new();
    highlight_lines(text, syntax, theme, |line| {
        lines.push(line);
        true
    });
    let done = lines.len();
    lines.extend(plain::render(text).into_iter().skip(done));
    lines
}

/// Highlight `text` line by line, handing each finished line to `sink`, which
/// returns false to stop. Also stops at a line too long to highlight quickly or
/// on a highlighter error, since the state after either is unreliable. Lines have
/// the same text as `plain::render` produces, only styled.
pub fn highlight_lines(
    text: &str,
    syntax: &SyntaxReference,
    theme: &Theme,
    mut sink: impl FnMut(Line) -> bool,
) {
    let syntaxes = assets::syntaxes();
    let mut highlighter = HighlightLines::new(syntax, theme);
    // Text in the theme's default color is left uncolored, so it uses the terminal's
    // own foreground and stays readable whatever the background.
    let default_fg = theme.settings.foreground;

    for source_line in LinesWithEndings::from(text) {
        if source_line.len() > MAX_HIGHLIGHT_LINE {
            return;
        }
        let Ok(regions) = highlighter.highlight_line(source_line, syntaxes) else {
            return;
        };
        // Same line ending rule as `str::lines`: "\n", or "\r\n".
        let body = source_line
            .strip_suffix('\n')
            .map_or(source_line, |l| l.strip_suffix('\r').unwrap_or(l));
        let mut line = Line::default();
        let mut pos = 0;
        for (st, piece) in regions {
            let end = (pos + piece.len()).min(body.len());
            let visible = if pos < end { &piece[..end - pos] } else { "" };
            pos += piece.len();
            let fg = (Some(st.foreground) != default_fg).then_some(Color::Rgb(
                st.foreground.r,
                st.foreground.g,
                st.foreground.b,
            ));
            let style = Style {
                fg,
                bold: st.font_style.contains(FontStyle::BOLD),
                italic: st.font_style.contains(FontStyle::ITALIC),
                underline: st.font_style.contains(FontStyle::UNDERLINE),
                ..Style::default()
            };
            line.push(visible, style);
        }
        if !sink(plain::sanitize(&line)) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> &'static SyntaxReference {
        assets::syntaxes().find_syntax_by_extension("rs").unwrap()
    }

    #[test]
    fn highlights_rust_and_preserves_text() {
        let src = "fn main() {\n\tlet x = \"hi\"; // note\n}\n";
        let theme = assets::theme("base16-ocean.dark").unwrap();
        let lines = render(src, rust(), theme);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(
            text,
            vec!["fn main() {", "        let x = \"hi\"; // note", "}"]
        );
        // `fn` keyword and the string literal get distinct colors.
        let colored = lines[1]
            .spans
            .iter()
            .filter(|s| s.style.fg.is_some())
            .count();
        assert!(
            colored >= 2,
            "expected colored spans, got {:?}",
            lines[1].spans
        );
        assert!(lines[0].spans[0].style.fg.is_some());
    }

    #[test]
    fn last_line_without_newline() {
        let theme = assets::theme("base16-ocean.dark").unwrap();
        let lines = render("a\nb", rust(), theme);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn highlight_limits() {
        assert!(should_highlight("short\nlines\n"));
        assert!(!should_highlight(&"x".repeat(MAX_HIGHLIGHT_LINE + 1)));
    }

    #[test]
    fn text_matches_plain_rendering_exactly() {
        let theme = assets::theme("base16-ocean.dark").unwrap();
        let src = "a\r\nb\rc\n\tx = 1;\n\x1b[0m\n\nlast";
        let highlighted: Vec<String> = render(src, rust(), theme).iter().map(Line::text).collect();
        let plain: Vec<String> = plain::render(src).iter().map(Line::text).collect();
        assert_eq!(highlighted, plain);
    }

    #[test]
    fn stops_at_very_long_lines_and_rest_is_plain() {
        let theme = assets::theme("base16-ocean.dark").unwrap();
        let src = format!(
            "fn a() {{}}\n{}\nfn b() {{}}\n",
            "x".repeat(MAX_HIGHLIGHT_LINE + 1)
        );
        let lines = render(&src, rust(), theme);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].spans.iter().any(|s| s.style.fg.is_some()));
        assert!(lines[2].spans.iter().all(|s| s.style.fg.is_none()));
    }

    #[test]
    fn sink_can_stop_early() {
        let theme = assets::theme("base16-ocean.dark").unwrap();
        let mut count = 0;
        highlight_lines("a\nb\nc\n", rust(), theme, |_| {
            count += 1;
            count < 2
        });
        assert_eq!(count, 2);
    }
}
