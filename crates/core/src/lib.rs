//! The editing model, with no UI: a [`Buffer`] of text with undo history,
//! helix-style [`Range`] selections, the motions and text objects that move
//! them, the [`modal`] key handling that drives it all, what's particular to a
//! [`language`] (it has none of its own: plugins add them), and [`syntax`] over
//! any grammar, its highlights and text objects, and [`abbreviation`]s.
//!
//! The text is a rope ([`Rope`]): what reads it reads a line or a range at a
//! time. Every offset is a byte offset into it, always on a char boundary.

pub mod abbreviation;
pub mod buffer;
pub mod indent;
pub mod language;
pub mod modal;
pub mod movement;
pub mod nodes;
pub mod selection;
pub mod syntax;
pub mod text;
pub mod textobject;

pub use buffer::{Buffer, Edit};
pub use language::Language;
pub use modal::{Command, Direction, Effect, Key, Keymap, MenuKey, Modal, Mode, Outcome, Place, Registers, Review, Target, TreeCommand, ViewKind};
pub use ropey::{Rope, RopeSlice};
pub use selection::Range;
pub use text::Text;

/// The boundary after the character at `i` (or the end): a character as
/// it's seen, which may be several code points (`é` as `e` and an accent,
/// a flag, an emoji with a skin tone).
pub fn next_char(text: &str, i: usize) -> usize {
    let mut cursor = unicode_segmentation::GraphemeCursor::new(i, text.len(), true);
    cursor.next_boundary(text, 0).ok().flatten().unwrap_or(text.len())
}

/// The boundary before the character ending at `i` (or the start).
pub fn prev_char(text: &str, i: usize) -> usize {
    let mut cursor = unicode_segmentation::GraphemeCursor::new(i, text.len(), true);
    cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0)
}

/// How many columns `text` takes on screen: wide characters two, a tab to
/// the next stop of `tab_width`.
pub fn display_width(text: &str, tab_width: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation as _;
    text.graphemes(true).fold(0, |column, g| match g {
        "\t" => column + tab_width.max(1) - column % tab_width.max(1),
        g => column + unicode_width::UnicodeWidthStr::width(g).max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chars_and_positions() {
        let text = "aé\nx";
        assert_eq!(next_char(text, 1), 3);
        assert_eq!(prev_char(text, 3), 1);
        assert_eq!(next_char(text, text.len()), text.len());
        let buffer = Buffer::new(text);
        assert_eq!(buffer.at_line_col(0, 1), 1);
        assert_eq!(buffer.at_line_col(0, 9), 3, "clamped to the line");
        assert_eq!(buffer.at_line_col(1, 1), text.len());
    }
}
