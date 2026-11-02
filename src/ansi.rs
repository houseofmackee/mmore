//! Converting styled lines to ANSI escape sequences for the terminal's color depth.

use std::env;

use crate::style::{Color, Line, Style};

/// When to use color, as chosen on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

/// What the output terminal can display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorMode {
    None,
    Ansi16,
    Ansi256,
    TrueColor,
}

impl ColorMode {
    pub fn resolve(when: ColorWhen, is_tty: bool) -> ColorMode {
        let colorterm = env::var("COLORTERM").unwrap_or_default();
        let term = env::var("TERM").unwrap_or_default();
        match when {
            ColorWhen::Never => ColorMode::None,
            ColorWhen::Always => depth(&colorterm, &term).max(ColorMode::Ansi16),
            ColorWhen::Auto => {
                // https://no-color.org: any non-empty value disables color.
                let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
                if !is_tty || no_color {
                    ColorMode::None
                } else {
                    depth(&colorterm, &term)
                }
            }
        }
    }

    pub fn enabled(self) -> bool {
        self != ColorMode::None
    }
}

fn depth(colorterm: &str, term: &str) -> ColorMode {
    if colorterm == "truecolor" || colorterm == "24bit" {
        ColorMode::TrueColor
    } else if term == "dumb" {
        ColorMode::None
    } else if term.contains("256") || term.contains("direct") {
        ColorMode::Ansi256
    } else {
        ColorMode::Ansi16
    }
}

/// Render a line with escape sequences. Every styled span ends with a reset, so a
/// line never leaks its style into the next one.
pub fn render(line: &Line, mode: ColorMode) -> String {
    let mut out = String::new();
    for span in &line.spans {
        if mode == ColorMode::None || span.style.is_plain() {
            out.push_str(&span.text);
            continue;
        }
        let sgr = sgr_params(&span.style, mode);
        if sgr.is_empty() {
            out.push_str(&span.text);
        } else {
            out.push_str("\x1b[");
            out.push_str(&sgr.join(";"));
            out.push('m');
            out.push_str(&span.text);
            out.push_str("\x1b[0m");
        }
    }
    out
}

fn sgr_params(style: &Style, mode: ColorMode) -> Vec<String> {
    let mut p = Vec::new();
    if style.bold {
        p.push("1".to_string());
    }
    if style.dim {
        p.push("2".to_string());
    }
    if style.italic {
        p.push("3".to_string());
    }
    if style.underline {
        p.push("4".to_string());
    }
    if style.strike {
        p.push("9".to_string());
    }
    if let Some(c) = style.fg {
        p.push(color_param(c, mode, false));
    }
    if let Some(c) = style.bg {
        p.push(color_param(c, mode, true));
    }
    p
}

fn color_param(color: Color, mode: ColorMode, background: bool) -> String {
    let base = if background { 40 } else { 30 };
    let ansi = |n: u8| {
        if n < 8 {
            (base + n as u32).to_string()
        } else {
            // Bright variants: 90-97 / 100-107.
            (base + 60 + (n as u32 - 8)).to_string()
        }
    };
    match (color, mode) {
        (Color::Ansi(n), _) => ansi(n.min(15)),
        (Color::Rgb(r, g, b), ColorMode::TrueColor) => format!("{};2;{r};{g};{b}", base + 8),
        (Color::Rgb(r, g, b), ColorMode::Ansi256) => format!("{};5;{}", base + 8, to_256(r, g, b)),
        (Color::Rgb(r, g, b), _) => ansi(to_16(r, g, b)),
    }
}

/// Nearest xterm 256-color index, choosing between the 6x6x6 cube and the gray ramp.
pub fn to_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let cube_index = |v: u8| -> usize {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (v as i32 - **l as i32).abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (cube_index(r), cube_index(g), cube_index(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_code = 16 + 36 * ri + 6 * gi + bi;

    // Gray ramp 232-255 covers 8, 18, ..., 238.
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gray_step = ((avg.saturating_sub(8) + 5) / 10).min(23);
    let gray_level = (8 + gray_step * 10) as u8;
    let gray_code = 232 + gray_step as usize;

    let target = (r, g, b);
    if distance(target, (gray_level, gray_level, gray_level)) < distance(target, cube) {
        gray_code as u8
    } else {
        cube_code as u8
    }
}

/// Nearest of the 16 basic colors, using typical xterm values.
pub fn to_16(r: u8, g: u8, b: u8) -> u8 {
    const PALETTE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| distance((r, g, b), **c))
        .map(|(i, _)| i as u8)
        .unwrap_or(7)
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2) as u32;
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Span;

    fn line(style: Style) -> Line {
        Line {
            spans: vec![Span::new("x", style)],
        }
    }

    fn red() -> Style {
        Style {
            fg: Some(Color::Rgb(255, 0, 0)),
            bold: true,
            ..Style::default()
        }
    }

    #[test]
    fn renders_each_depth() {
        assert_eq!(render(&line(red()), ColorMode::None), "x");
        assert_eq!(
            render(&line(red()), ColorMode::TrueColor),
            "\x1b[1;38;2;255;0;0mx\x1b[0m"
        );
        assert_eq!(
            render(&line(red()), ColorMode::Ansi256),
            "\x1b[1;38;5;196mx\x1b[0m"
        );
        assert_eq!(
            render(&line(red()), ColorMode::Ansi16),
            "\x1b[1;91mx\x1b[0m"
        );
    }

    #[test]
    fn plain_spans_have_no_escapes() {
        assert_eq!(render(&Line::plain("hi"), ColorMode::TrueColor), "hi");
    }

    #[test]
    fn palette_colors_and_backgrounds() {
        let style = Style {
            fg: Some(Color::Ansi(4)),
            bg: Some(Color::Ansi(8)),
            ..Style::default()
        };
        assert_eq!(
            render(&line(style), ColorMode::Ansi256),
            "\x1b[34;100mx\x1b[0m"
        );
    }

    #[test]
    fn downgrade_256() {
        assert_eq!(to_256(255, 0, 0), 196);
        assert_eq!(to_256(0, 0, 0), 16);
        assert_eq!(to_256(255, 255, 255), 231);
        // A mid gray is closer to the gray ramp than the cube.
        assert_eq!(to_256(128, 128, 128), 244);
        // base16-ocean blue #8fa1b3.
        assert_eq!(to_256(0x8f, 0xa1, 0xb3), 109);
    }

    #[test]
    fn downgrade_16() {
        assert_eq!(to_16(250, 10, 10), 9);
        assert_eq!(to_16(0, 0, 0), 0);
        assert_eq!(to_16(200, 200, 200), 7);
    }

    #[test]
    fn depth_detection() {
        assert_eq!(depth("truecolor", "xterm"), ColorMode::TrueColor);
        assert_eq!(depth("", "xterm-256color"), ColorMode::Ansi256);
        assert_eq!(depth("", "xterm"), ColorMode::Ansi16);
        assert_eq!(depth("", "dumb"), ColorMode::None);
    }
}
