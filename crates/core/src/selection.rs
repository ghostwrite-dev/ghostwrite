//! Helix's selection: a range with an `anchor` and a `head`. In normal mode
//! the char before the head (or at it, for an empty range) is the block
//! cursor.

use crate::{next_char, prev_char};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
}

impl Range {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn point(at: usize) -> Self {
        Self::new(at, at)
    }

    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn bytes(&self) -> std::ops::Range<usize> {
        self.from()..self.to()
    }

    /// The char the block cursor sits on.
    pub fn cursor(&self, text: &str) -> usize {
        if self.head > self.anchor { prev_char(text, self.head) } else { self.head }
    }

    /// Helix treats an empty range as covering the char under the cursor.
    pub fn min_width_1(&self, text: &str) -> Self {
        if self.is_empty() && self.head < text.len() { Self::new(self.anchor, next_char(text, self.head)) } else { *self }
    }

    /// Wider than the one char under the cursor.
    pub fn is_wide(&self, text: &str) -> bool {
        self.to() > next_char(text, self.from().min(text.len()))
    }

    pub fn flip(&self) -> Self {
        Self::new(self.head, self.anchor)
    }

    pub fn clamp(&self, len: usize) -> Self {
        Self::new(self.anchor.min(len), self.head.min(len))
    }

    /// Clamped to `text`, each end on a char boundary (moved back if it was inside one).
    pub fn within(&self, text: &str) -> Self {
        let snap = |at: usize| {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        Self::new(snap(self.anchor), snap(self.head))
    }

    /// Move the cursor to `at`, extending from the anchor when `extend` is
    /// set (`Range::put_cursor` in helix).
    pub fn put_cursor(&self, text: &str, at: usize, extend: bool) -> Self {
        if !extend {
            return Self::point(at);
        }
        let anchor = if self.head >= self.anchor && at < self.anchor {
            next_char(text, self.anchor)
        } else if self.head < self.anchor && at >= self.anchor {
            prev_char(text, self.anchor)
        } else {
            self.anchor
        };
        if anchor <= at { Self::new(anchor, next_char(text, at)) } else { Self::new(anchor, at) }
    }
}

#[cfg(test)]
mod tests {
    use super::Range;

    #[test]
    fn extend_put_cursor_flips_over_anchor() {
        let text = "abcdef";
        let r = Range::new(2, 3);
        assert_eq!(r.put_cursor(text, 4, true), Range::new(2, 5));
        assert_eq!(r.put_cursor(text, 0, true), Range::new(3, 0));
    }

    #[test]
    fn cursor_sits_before_a_forward_head() {
        assert_eq!(Range::new(0, 3).cursor("aéb"), 1);
        assert_eq!(Range::new(3, 0).cursor("aéb"), 0);
        assert!(!Range::point(1).min_width_1("aéb").is_wide("aéb"));
        assert!(Range::new(0, 3).is_wide("aéb"));
    }
}
