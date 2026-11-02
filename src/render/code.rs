//! Syntax highlighting with syntect.

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::SyntaxReference;
use syntect::util::LinesWithEndings;

use super::plain;
use crate::assets;
use crate::style::{Color, Line, Style};

/// Inputs larger than this are shown as plain text, since highlighting is done up
/// front (about 2 ms per KiB of Rust in a release build). Lazy highlighting for big
/// files is planned for milestone 7 and will lift this limit.
pub const MAX_HIGHLIGHT_BYTES: usize = 256 * 1024;
/// Very long lines (minified code) make the highlighter extremely slow.
pub const MAX_HIGHLIGHT_LINE: usize = 20_000;

/// Whether `text` is small and regular enough to highlight quickly.
pub fn should_highlight(text: &str) -> bool {
    text.len() <= MAX_HIGHLIGHT_BYTES && text.lines().all(|l| l.len() <= MAX_HIGHLIGHT_LINE)
}

pub fn render(text: &str, syntax: &SyntaxReference, theme: &Theme) -> Vec<Line> {
    let syntaxes = assets::syntaxes();
    let mut highlighter = HighlightLines::new(syntax, theme);
    // Text in the theme's default color is left uncolored, so it uses the terminal's
    // own foreground and stays readable whatever the background.
    let default_fg = theme.settings.foreground;

    let mut lines = Vec::new();
    for source_line in LinesWithEndings::from(text) {
        let Ok(regions) = highlighter.highlight_line(source_line, syntaxes) else {
            // The highlighter state is unreliable after an error: fall back entirely.
            return plain::render(text);
        };
        let mut line = Line::default();
        for (st, piece) in regions {
            let piece = piece.trim_end_matches(['\n', '\r']);
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
            line.push(piece, style);
        }
        lines.push(plain::sanitize(&line));
    }
    lines
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
}
