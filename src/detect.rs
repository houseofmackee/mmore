//! Choosing how to render an input.

use std::path::Path;

use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::source::Origin;

/// How an input should be rendered.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Plain,
    Markdown,
    Code(&'static SyntaxReference),
}

const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn", "mdx"];

/// Languages missing from syntect's bundled set, mapped to the closest one it has.
const ALIASES: &[(&str, &str)] = &[
    ("tsx", "ts"),
    ("typescript", "ts"),
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("kt", "java"),
    ("kts", "java"),
    ("kotlin", "java"),
    ("jsonc", "json"),
    ("json5", "json"),
    ("dockerfile", "sh"),
    ("vue", "html"),
    ("svelte", "html"),
];

/// Decide the renderer for an input, given an optional `--language` override.
pub fn detect(
    syntaxes: &'static SyntaxSet,
    origin: &Origin,
    first_line: &str,
    language: Option<&str>,
) -> Kind {
    if let Some(lang) = language {
        return by_language(syntaxes, lang).unwrap_or(Kind::Plain);
    }

    if let Origin::File(path) = origin
        && let Some(kind) = by_path(syntaxes, path)
    {
        return kind;
    }

    syntaxes
        .find_syntax_by_first_line(first_line)
        .map(code_or_plain)
        .unwrap_or(Kind::Plain)
}

/// Resolve a `--language` value (a name like "Rust" or "python", or an extension).
pub fn by_language(syntaxes: &'static SyntaxSet, lang: &str) -> Option<Kind> {
    let lower = lang.to_ascii_lowercase();
    if lower == "markdown" || MARKDOWN_EXTENSIONS.contains(&lower.as_str()) {
        return Some(Kind::Markdown);
    }
    if lower == "plain" || lower == "text" || lower == "txt" {
        return Some(Kind::Plain);
    }
    syntaxes
        .find_syntax_by_token(&lower)
        .or_else(|| syntaxes.find_syntax_by_name(lang))
        .or_else(|| alias(syntaxes, &lower))
        .map(code_or_plain)
}

fn by_path(syntaxes: &'static SyntaxSet, path: &Path) -> Option<Kind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if let Some(ext) = &ext
        && MARKDOWN_EXTENSIONS.contains(&ext.as_str())
    {
        return Some(Kind::Markdown);
    }

    // Whole file names first (Makefile, Gemfile), then extensions.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let syntax = syntaxes
        .find_syntax_by_extension(name)
        .or_else(|| {
            ext.as_deref()
                .and_then(|e| syntaxes.find_syntax_by_extension(e))
        })
        .or_else(|| alias(syntaxes, &name.to_ascii_lowercase()))
        .or_else(|| ext.as_deref().and_then(|e| alias(syntaxes, e)))?;
    Some(code_or_plain(syntax))
}

fn alias(syntaxes: &'static SyntaxSet, token: &str) -> Option<&'static SyntaxReference> {
    ALIASES
        .iter()
        .find(|(from, _)| *from == token)
        .and_then(|(_, to)| syntaxes.find_syntax_by_extension(to))
}

fn code_or_plain(syntax: &'static SyntaxReference) -> Kind {
    if syntax.name == "Plain Text" {
        Kind::Plain
    } else if syntax.name == "Markdown" {
        Kind::Markdown
    } else {
        Kind::Code(syntax)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets;
    use std::path::PathBuf;

    fn file(name: &str) -> Origin {
        Origin::File(PathBuf::from(name))
    }

    fn name_of(kind: Kind) -> String {
        match kind {
            Kind::Plain => "plain".into(),
            Kind::Markdown => "markdown".into(),
            Kind::Code(s) => s.name.clone(),
        }
    }

    fn detect_name(origin: &Origin, first: &str, lang: Option<&str>) -> String {
        name_of(detect(assets::syntaxes(), origin, first, lang))
    }

    #[test]
    fn by_extension() {
        assert_eq!(detect_name(&file("src/main.rs"), "", None), "Rust");
        assert_eq!(detect_name(&file("a.py"), "", None), "Python");
        assert_eq!(detect_name(&file("a.JSON"), "", None), "JSON");
        assert_eq!(detect_name(&file("README.md"), "", None), "markdown");
        assert_eq!(detect_name(&file("notes.txt"), "", None), "plain");
        assert_eq!(detect_name(&file("data.unknownext"), "", None), "plain");
    }

    #[test]
    fn by_file_name() {
        assert_eq!(detect_name(&file("Makefile"), "", None), "Makefile");
    }

    #[test]
    fn bundled_and_aliased_languages() {
        assert_eq!(detect_name(&file("app.ts"), "", None), "TypeScript");
        assert_eq!(detect_name(&file("app.tsx"), "", None), "TypeScript");
        assert_eq!(detect_name(&file("Cargo.toml"), "", None), "TOML");
        assert_eq!(detect_name(&file("Cargo.lock"), "", None), "TOML");
    }

    #[test]
    fn by_first_line() {
        assert_eq!(
            detect_name(&Origin::Stdin, "#!/usr/bin/env python3", None),
            "Python"
        );
        assert_eq!(
            detect_name(&file("script"), "#!/bin/bash", None),
            "Bourne Again Shell (bash)"
        );
        assert_eq!(detect_name(&Origin::Stdin, "hello", None), "plain");
    }

    #[test]
    fn language_override() {
        assert_eq!(detect_name(&file("a.txt"), "", Some("rust")), "Rust");
        assert_eq!(detect_name(&file("a.rs"), "", Some("Python")), "Python");
        assert_eq!(detect_name(&file("a.rs"), "", Some("md")), "markdown");
        assert_eq!(detect_name(&file("a.rs"), "", Some("plain")), "plain");
        assert_eq!(
            detect_name(&file("a.rs"), "", Some("typescript")),
            "TypeScript"
        );
        assert!(by_language(assets::syntaxes(), "nonsense-lang").is_none());
    }
}
