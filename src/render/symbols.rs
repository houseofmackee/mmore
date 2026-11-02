//! Characters used to draw Markdown structure, in Unicode and ASCII variants.

pub struct Symbols {
    /// Bullets by list nesting depth.
    pub bullets: [&'static str; 3],
    pub quote_bar: &'static str,
    pub h1_rule: &'static str,
    pub h2_rule: &'static str,
    pub rule: &'static str,
    pub code_top: &'static str,
    pub code_side: &'static str,
    pub code_bottom: &'static str,
    pub unchecked: &'static str,
    pub checked: &'static str,
    pub image: &'static str,
    pub table: TableChars,
}

pub struct TableChars {
    pub horizontal: &'static str,
    pub vertical: &'static str,
    /// Left, middle, and right joints of the top, separator, and bottom borders.
    pub top: [&'static str; 3],
    pub middle: [&'static str; 3],
    pub bottom: [&'static str; 3],
}

pub const UNICODE: Symbols = Symbols {
    bullets: ["•", "◦", "▪"],
    quote_bar: "│",
    h1_rule: "═",
    h2_rule: "─",
    rule: "─",
    code_top: "┌─",
    code_side: "│",
    code_bottom: "└─",
    unchecked: "☐",
    checked: "☑",
    image: "▣",
    table: TableChars {
        horizontal: "─",
        vertical: "│",
        top: ["┌", "┬", "┐"],
        middle: ["├", "┼", "┤"],
        bottom: ["└", "┴", "┘"],
    },
};

pub const ASCII: Symbols = Symbols {
    bullets: ["*", "-", "+"],
    quote_bar: "|",
    h1_rule: "=",
    h2_rule: "-",
    rule: "-",
    code_top: "+-",
    code_side: "|",
    code_bottom: "+-",
    unchecked: "[ ]",
    checked: "[x]",
    image: "[image]",
    table: TableChars {
        horizontal: "-",
        vertical: "|",
        top: ["+", "+", "+"],
        middle: ["+", "+", "+"],
        bottom: ["+", "+", "+"],
    },
};

pub fn get(ascii: bool) -> &'static Symbols {
    if ascii { &ASCII } else { &UNICODE }
}
