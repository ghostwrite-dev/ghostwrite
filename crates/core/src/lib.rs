//! The editing model, with no UI: a [`Buffer`] of text with undo history,
//! helix-style [`Range`] selections, the motions and text objects that move
//! them, the [`modal`] key handling that drives it all, the [`language`]s it
//! knows, and [`syntax`] highlighting.
//!
//! Every offset is a byte offset into the text, always on a char boundary.

pub mod buffer;
pub mod indent;
pub mod language;
pub mod modal;
pub mod movement;
pub mod selection;
pub mod syntax;
pub mod textobject;

pub use buffer::Buffer;
pub use language::Language;
pub use modal::{Effect, Key, MenuKey, Modal, Mode, Outcome, Registers, Review, Target, TreeCommand};
pub use selection::Range;

/// The boundary after the char at `i` (or the end).
pub fn next_char(text: &str, i: usize) -> usize {
    text[i..].chars().next().map_or(text.len(), |c| i + c.len_utf8())
}

/// The boundary before the char ending at `i` (or the start).
pub fn prev_char(text: &str, i: usize) -> usize {
    text[..i].chars().next_back().map_or(0, |c| i - c.len_utf8())
}

/// Byte offset → (zero-based line, char column), for status lines and sessions.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    (text[..start].matches('\n').count(), text[start..offset].chars().count())
}

/// (zero-based line, char column) → byte offset, clamped to the text and the line.
pub fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let start = if line == 0 { 0 } else { text.match_indices('\n').nth(line - 1).map_or(text.len(), |(i, _)| i + 1) };
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    text[start..end].char_indices().nth(column).map_or(end, |(i, _)| start + i)
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
        assert_eq!(line_col(text, 5), (1, 1));
        assert_eq!(offset_of(text, 0, 1), 1);
        assert_eq!(offset_of(text, 0, 9), 3, "clamped to the line");
        assert_eq!(offset_of(text, 7, 0), text.len());
    }
}
