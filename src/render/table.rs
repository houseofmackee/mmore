//! Markdown table layout.
//!
//! Tables are drawn with borders at their natural size when they fit. Otherwise
//! columns are shrunk (widest first, sparing columns already at their longest word)
//! and cell text wraps. When even that leaves no room, each row is shown as a
//! record of "Header: value" lines.

use pulldown_cmark::Alignment;
use unicode_width::UnicodeWidthStr;

use super::symbols::Symbols;
use crate::style::{Line, Span, Style};
use crate::wrap;

/// Narrowest useful column; tables that cannot give every column this much are
/// shown as records instead.
const MIN_COLUMN: usize = 3;

pub struct Table {
    pub aligns: Vec<Alignment>,
    /// Cells as styled spans, row by row. The first row is the header.
    pub rows: Vec<Vec<Vec<Span>>>,
}

impl Table {
    fn columns(&self) -> usize {
        self.rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(self.aligns.len())
    }

    fn cell(&self, row: usize, col: usize) -> &[Span] {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map_or(&[], Vec::as_slice)
    }
}

pub fn layout(table: &Table, width: usize, sym: &Symbols) -> Vec<Line> {
    let columns = table.columns();
    if columns == 0 || table.rows.is_empty() {
        return Vec::new();
    }
    let natural: Vec<usize> = (0..columns)
        .map(|c| {
            (0..table.rows.len())
                .map(|r| natural_width(table.cell(r, c)))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    let longest_word: Vec<usize> = (0..columns)
        .map(|c| {
            (0..table.rows.len())
                .map(|r| longest_word(table.cell(r, c)))
                .max()
                .unwrap_or(0)
        })
        .collect();

    // Borders: "│ " before each column, " " after it, and a final "│".
    let budget = width.saturating_sub(3 * columns + 1);
    if budget < columns * MIN_COLUMN {
        return records(table, columns, width);
    }
    let widths = fit(&natural, &longest_word, budget);
    grid(table, &widths, sym)
}

/// Shrink column widths until they fit in `budget`.
fn fit(natural: &[usize], longest_word: &[usize], budget: usize) -> Vec<usize> {
    let mut widths = natural.to_vec();
    let mut total: usize = widths.iter().sum();
    while total > budget {
        // Prefer columns that can shrink without splitting words.
        let pick = (0..widths.len())
            .filter(|&c| widths[c] > longest_word[c].max(MIN_COLUMN))
            .max_by_key(|&c| widths[c])
            .or_else(|| {
                (0..widths.len())
                    .filter(|&c| widths[c] > MIN_COLUMN)
                    .max_by_key(|&c| widths[c])
            });
        let Some(c) = pick else { break };
        widths[c] -= 1;
        total -= 1;
    }
    widths
}

fn grid(table: &Table, widths: &[usize], sym: &Symbols) -> Vec<Line> {
    let border = Style {
        dim: true,
        ..Style::default()
    };
    let t = &sym.table;
    let rule = |joints: [&str; 3]| {
        let segments: Vec<String> = widths.iter().map(|w| t.horizontal.repeat(w + 2)).collect();
        let text = format!("{}{}{}", joints[0], segments.join(joints[1]), joints[2]);
        Line {
            spans: vec![Span::new(text, border)],
        }
    };

    let mut lines = vec![rule(t.top)];
    for row in 0..table.rows.len() {
        let header = row == 0;
        let cells: Vec<Vec<Line>> = widths
            .iter()
            .enumerate()
            .map(|(c, &w)| {
                let mut spans = table.cell(row, c).to_vec();
                if header {
                    for span in &mut spans {
                        span.style.bold = true;
                    }
                }
                wrap::wrap_words(&spans, w)
            })
            .collect();
        let height = cells.iter().map(Vec::len).max().unwrap_or(1);
        for i in 0..height {
            let mut line = Line::default();
            for (c, &w) in widths.iter().enumerate() {
                line.push(t.vertical, border);
                line.push(" ", Style::default());
                let content = cells[c].get(i).cloned().unwrap_or_default();
                let pad = w.saturating_sub(content.width());
                let left = match table.aligns.get(c) {
                    Some(Alignment::Center) => pad / 2,
                    Some(Alignment::Right) => pad,
                    _ => 0,
                };
                line.push(&" ".repeat(left), Style::default());
                line.append(content);
                line.push(&" ".repeat(pad - left + 1), Style::default());
            }
            line.push(t.vertical, border);
            lines.push(line);
        }
        if header && table.rows.len() > 1 {
            lines.push(rule(t.middle));
        }
    }
    lines.push(rule(t.bottom));
    lines
}

/// Fallback for very narrow widths: one "Header: value" line per cell, with a
/// blank line between rows.
fn records(table: &Table, columns: usize, width: usize) -> Vec<Line> {
    let bold = Style {
        bold: true,
        ..Style::default()
    };
    let header_text = |c: usize| -> String {
        table
            .cell(0, c)
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>()
    };
    let mut lines = Vec::new();
    if table.rows.len() == 1 {
        for c in 0..columns {
            lines.push(Line {
                spans: vec![Span::new(header_text(c), bold)],
            });
        }
        return lines;
    }
    for row in 1..table.rows.len() {
        if row > 1 {
            lines.push(Line::default());
        }
        for c in 0..columns {
            let mut spans = vec![Span::new(format!("{}:", header_text(c)), bold)];
            spans.push(Span::new(" ", Style::default()));
            spans.extend(table.cell(row, c).iter().cloned());
            lines.extend(wrap::wrap_words(&spans, width));
        }
    }
    lines
}

/// Width of a cell's text on one line (or its widest line, with hard breaks).
fn natural_width(spans: &[Span]) -> usize {
    wrap::wrap_words(spans, usize::MAX / 2)
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
}

fn longest_word(spans: &[Span]) -> usize {
    let text: String = spans.iter().map(|s| s.text.as_str()).collect();
    text.split_whitespace()
        .map(|w| w.width())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::symbols;

    fn cell(s: &str) -> Vec<Span> {
        vec![Span::new(s, Style::default())]
    }

    fn table(aligns: Vec<Alignment>, rows: &[&[&str]]) -> Table {
        Table {
            aligns,
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| cell(c)).collect())
                .collect(),
        }
    }

    fn text(lines: &[Line]) -> String {
        lines.iter().map(Line::text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn natural_size_with_alignment() {
        let t = table(
            vec![Alignment::Left, Alignment::Center, Alignment::Right],
            &[
                &["Name", "Mid", "Num"],
                &["a", "b", "1"],
                &["long name", "c", "100"],
            ],
        );
        assert_eq!(
            text(&layout(&t, 80, &symbols::UNICODE)),
            "┌───────────┬─────┬─────┐\n\
             │ Name      │ Mid │ Num │\n\
             ├───────────┼─────┼─────┤\n\
             │ a         │  b  │   1 │\n\
             │ long name │  c  │ 100 │\n\
             └───────────┴─────┴─────┘"
        );
    }

    #[test]
    fn header_is_bold_and_borders_dim() {
        let t = table(vec![Alignment::None], &[&["H"], &["v"]]);
        let lines = layout(&t, 80, &symbols::UNICODE);
        assert!(lines[0].spans[0].style.dim);
        assert!(lines[1].spans.iter().any(|s| s.text == "H" && s.style.bold));
    }

    #[test]
    fn shrinks_widest_column_and_wraps() {
        let t = table(
            vec![Alignment::None, Alignment::None],
            &[
                &["id", "description"],
                &["1", "a fairly long description here"],
            ],
        );
        let lines = layout(&t, 30, &symbols::UNICODE);
        assert!(lines.iter().all(|l| l.width() <= 30), "{}", text(&lines));
        assert_eq!(
            text(&lines),
            "┌────┬───────────────────────┐\n\
             │ id │ description           │\n\
             ├────┼───────────────────────┤\n\
             │ 1  │ a fairly long         │\n\
             │    │ description here      │\n\
             └────┴───────────────────────┘"
        );
    }

    #[test]
    fn too_narrow_becomes_records() {
        let t = table(
            vec![Alignment::None; 3],
            &[&["A", "B", "C"], &["1", "2", "3"], &["4", "5", "6"]],
        );
        assert_eq!(
            text(&layout(&t, 15, &symbols::UNICODE)),
            "A: 1\nB: 2\nC: 3\n\nA: 4\nB: 5\nC: 6"
        );
    }

    #[test]
    fn ragged_rows_are_padded() {
        let t = Table {
            aligns: vec![Alignment::None, Alignment::None],
            rows: vec![vec![cell("a"), cell("b")], vec![cell("x")]],
        };
        assert_eq!(
            text(&layout(&t, 80, &symbols::ASCII)),
            "+---+---+\n| a | b |\n+---+---+\n| x |   |\n+---+---+"
        );
    }

    #[test]
    fn fit_respects_budget() {
        assert_eq!(fit(&[10, 20], &[5, 5], 20), vec![10, 10]);
        assert_eq!(fit(&[10, 20], &[10, 20], 24), vec![10, 14]);
        // Never below the minimum column width.
        assert_eq!(fit(&[10, 10], &[10, 10], 2), vec![3, 3]);
    }
}
