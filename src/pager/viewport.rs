//! Scroll position bookkeeping, independent of any terminal I/O.

use super::keys::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// Index of the first visible row.
    pub top: usize,
    /// Total number of rows in the document.
    pub total: usize,
    /// Number of rows available for content (screen height minus the status line).
    pub height: usize,
}

impl Viewport {
    pub fn new(total: usize, height: usize) -> Self {
        Viewport {
            top: 0,
            total,
            height: height.max(1),
        }
    }

    pub fn max_top(&self) -> usize {
        self.total.saturating_sub(self.height)
    }

    pub fn at_end(&self) -> bool {
        self.top >= self.max_top()
    }

    /// Percentage of the document that has been shown, counting the bottom visible row.
    pub fn percent(&self) -> usize {
        if self.total == 0 {
            return 100;
        }
        (self.top + self.height).min(self.total) * 100 / self.total
    }

    pub fn apply(&mut self, action: Action) {
        let half = (self.height / 2).max(1);
        match action {
            Action::LineDown => self.down(1),
            Action::LineUp => self.up(1),
            Action::PageDown => self.down(self.height),
            Action::PageUp => self.up(self.height),
            Action::HalfDown => self.down(half),
            Action::HalfUp => self.up(half),
            Action::Top => self.top = 0,
            Action::Bottom => self.top = self.max_top(),
            Action::Quit | Action::None => {}
        }
    }

    /// Update dimensions (after a resize) and scroll to `top`, clamped to the valid range.
    pub fn reset(&mut self, total: usize, height: usize, top: usize) {
        self.total = total;
        self.height = height.max(1);
        self.top = top.min(self.max_top());
    }

    fn down(&mut self, n: usize) {
        self.top = (self.top + n).min(self.max_top());
    }

    fn up(&mut self, n: usize) {
        self.top = self.top.saturating_sub(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_clamps_at_both_ends() {
        let mut vp = Viewport::new(25, 10);
        vp.apply(Action::PageDown);
        assert_eq!(vp.top, 10);
        vp.apply(Action::PageDown);
        assert_eq!(vp.top, 15);
        assert!(vp.at_end());
        vp.apply(Action::PageDown);
        assert_eq!(vp.top, 15);
        vp.apply(Action::PageUp);
        vp.apply(Action::PageUp);
        assert_eq!(vp.top, 0);
    }

    #[test]
    fn half_and_line_steps() {
        let mut vp = Viewport::new(100, 10);
        vp.apply(Action::HalfDown);
        assert_eq!(vp.top, 5);
        vp.apply(Action::LineDown);
        assert_eq!(vp.top, 6);
        vp.apply(Action::LineUp);
        vp.apply(Action::HalfUp);
        assert_eq!(vp.top, 0);
        vp.apply(Action::Bottom);
        assert_eq!(vp.top, 90);
        vp.apply(Action::Top);
        assert_eq!(vp.top, 0);
    }

    #[test]
    fn percent_counts_bottom_row() {
        let mut vp = Viewport::new(40, 10);
        assert_eq!(vp.percent(), 25);
        vp.apply(Action::Bottom);
        assert_eq!(vp.percent(), 100);
        assert_eq!(Viewport::new(0, 10).percent(), 100);
    }

    #[test]
    fn short_document_is_already_at_end() {
        let vp = Viewport::new(3, 10);
        assert!(vp.at_end());
    }

    #[test]
    fn reset_clamps_top() {
        let mut vp = Viewport::new(100, 10);
        vp.reset(50, 20, 45);
        assert_eq!(vp.top, 30);
        vp.reset(50, 0, 0);
        assert_eq!(vp.height, 1);
    }
}
