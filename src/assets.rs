//! Lazily loaded syntect syntax definitions and themes.

use std::sync::OnceLock;

use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

pub const DEFAULT_DARK_THEME: &str = "base16-ocean.dark";
pub const DEFAULT_LIGHT_THEME: &str = "InspiredGitHub";

/// syntect's built-in syntaxes plus the extra grammars in `assets/syntaxes`,
/// precompiled by `build.rs`.
pub fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let bundle = include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.packdump"));
        syntect::dumps::from_uncompressed_data(bundle).expect("syntax bundle is valid")
    })
}

pub fn themes() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

pub fn theme(name: &str) -> Option<&'static Theme> {
    themes().themes.get(name)
}

/// Theme to use when none was requested, for a light or dark terminal background.
pub fn default_theme_name(light_background: bool) -> &'static str {
    if light_background {
        DEFAULT_LIGHT_THEME
    } else {
        DEFAULT_DARK_THEME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_grammars_are_included() {
        let set = syntaxes();
        assert_eq!(
            set.find_syntax_by_extension("ts").unwrap().name,
            "TypeScript"
        );
        assert_eq!(set.find_syntax_by_extension("toml").unwrap().name, "TOML");
        // Built-in ones are still there.
        assert_eq!(set.find_syntax_by_extension("rs").unwrap().name, "Rust");
    }

    #[test]
    fn default_themes_exist() {
        assert!(theme(DEFAULT_DARK_THEME).is_some());
        assert!(theme(DEFAULT_LIGHT_THEME).is_some());
        assert!(theme("no-such-theme").is_none());
    }
}
