//! Lazily loaded syntect syntax definitions and themes.

use std::env;
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

/// Theme to use when none was requested, based on the terminal background.
pub fn default_theme_name() -> &'static str {
    if background_is_light(env::var("COLORFGBG").ok().as_deref()) {
        DEFAULT_LIGHT_THEME
    } else {
        DEFAULT_DARK_THEME
    }
}

/// `COLORFGBG` is set by some terminals (rxvt, Konsole, iTerm2 optionally) as
/// "fg;bg" or "fg;default;bg". Background 7 or 15 means a light terminal.
fn background_is_light(colorfgbg: Option<&str>) -> bool {
    colorfgbg
        .and_then(|v| v.rsplit(';').next())
        .and_then(|bg| bg.trim().parse::<u8>().ok())
        .is_some_and(|bg| bg == 7 || bg == 15)
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

    #[test]
    fn light_background_detection() {
        assert!(background_is_light(Some("0;15")));
        assert!(background_is_light(Some("0;default;7")));
        assert!(!background_is_light(Some("15;0")));
        assert!(!background_is_light(Some("garbage")));
        assert!(!background_is_light(None));
    }
}
