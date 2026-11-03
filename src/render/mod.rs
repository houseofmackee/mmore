//! Turning source text into displayable lines.

pub mod code;
pub mod escapes;
pub mod markdown;
pub mod plain;
pub mod symbols;
pub mod table;

use crate::style::Line;

/// Whether a line shows nothing but spaces.
pub fn is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.text.trim().is_empty())
}

/// Drop blank lines that come right after a blank line (`-s`). `after_blank`
/// says whether the line before these was blank; also returns whether the last
/// line kept is.
pub fn squeeze_blank(rendered: Rendered, after_blank: bool) -> (Rendered, bool) {
    let mut out = Rendered {
        links: rendered.links,
        headings: rendered.headings,
        ..Rendered::default()
    };
    let mut blank = after_blank;
    for (line, source) in rendered.lines.into_iter().zip(rendered.source) {
        let this = is_blank(&line);
        if !(this && blank) {
            out.push(line, source);
        }
        blank = this;
    }
    (out, blank)
}

/// A URL with a scheme ("https://...", "mailto:...").
pub fn is_absolute_url(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, rest)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && (rest.starts_with("//") || scheme.eq_ignore_ascii_case("mailto"))
    })
}

/// The anchor GitHub gives a heading: lowercase, spaces become `-`, and
/// punctuation other than `-` and `_` is dropped ("Hello, World!" is
/// `hello-world`).
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Anchors for headings in order; repeated ones get `-1`, `-2`, and so on, as on
/// GitHub.
pub fn slugs(headings: &[Heading]) -> Vec<String> {
    let mut seen = std::collections::HashMap::new();
    headings
        .iter()
        .map(|h| {
            let base = slug(&h.text);
            let n = seen.entry(base.clone()).or_insert(0usize);
            let slug = if *n == 0 { base } else { format!("{base}-{n}") };
            *n += 1;
            slug
        })
        .collect()
}

/// Rendered output with the source line each output line came from. Source line
/// numbers never decrease, so they can anchor the scroll position across resizes.
#[derive(Debug, Default)]
pub struct Rendered {
    pub lines: Vec<Line>,
    pub source: Vec<usize>,
    /// Hyperlink targets, indexed by `Style::link`.
    pub links: Vec<String>,
    /// Markdown headings, in order.
    pub headings: Vec<Heading>,
}

/// A Markdown heading, for jumping between headings and the table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1 for `#`, up to 6.
    pub level: u8,
    pub text: String,
    /// Source line where the heading is.
    pub source: usize,
}

impl Rendered {
    pub fn push(&mut self, line: Line, source: usize) {
        self.lines.push(line);
        self.source.push(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_runs_are_squeezed() {
        let mut r = Rendered::default();
        for (i, text) in ["a", "", " ", "", "b", "", "c", ""].iter().enumerate() {
            r.push(Line::plain(*text), i);
        }
        let (out, ends_blank) = squeeze_blank(r, false);
        let texts: Vec<String> = out.lines.iter().map(Line::text).collect();
        assert_eq!(texts, ["a", "", "b", "", "c", ""]);
        assert_eq!(out.source, [0, 1, 4, 5, 6, 7], "source lines are kept");
        assert!(ends_blank);
        // Continuing after a blank line drops a leading blank one.
        let mut more = Rendered::default();
        more.push(Line::plain(""), 8);
        more.push(Line::plain("d"), 9);
        let (out, ends_blank) = squeeze_blank(more, true);
        assert_eq!(out.source, [9]);
        assert!(!ends_blank);
    }

    #[test]
    fn slugs_like_github() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("Install `mmore` (macOS)"), "install-mmore-macos");
        assert_eq!(slug("snake_case and-dash"), "snake_case-and-dash");
        assert_eq!(slug("Ünïcode Straße"), "ünïcode-straße");
        let heading = |text: &str| Heading {
            level: 2,
            text: text.into(),
            source: 0,
        };
        assert_eq!(
            slugs(&[
                heading("Usage"),
                heading("Usage"),
                heading("Other"),
                heading("Usage")
            ]),
            vec!["usage", "usage-1", "other", "usage-2"]
        );
    }
}
