//! What a selection moves over: a [`Buffer`](crate::Buffer)'s rope, or a plain
//! string. Offsets are bytes; a character is what's seen as one (a grapheme).

use ropey::{Rope, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

pub trait Text {
    /// How many bytes there are.
    fn byte_len(&self) -> usize;
    /// The boundary after the character at `at` (or the end).
    fn next_boundary(&self, at: usize) -> usize;
    /// The boundary before the character ending at `at` (or the start).
    fn prev_boundary(&self, at: usize) -> usize;
    /// `at`, clamped to the text and moved back to a char boundary if it's inside a char.
    fn floor_boundary(&self, at: usize) -> usize;
}

impl Text for str {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn next_boundary(&self, at: usize) -> usize {
        crate::next_char(self, at.min(self.len()))
    }

    fn prev_boundary(&self, at: usize) -> usize {
        crate::prev_char(self, at.min(self.len()))
    }

    fn floor_boundary(&self, at: usize) -> usize {
        self.floor_char_boundary(at.min(self.len()))
    }
}

impl Text for String {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.as_str().next_boundary(at)
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.as_str().prev_boundary(at)
    }

    fn floor_boundary(&self, at: usize) -> usize {
        self.as_str().floor_boundary(at)
    }
}

impl Text for Rope {
    fn byte_len(&self) -> usize {
        self.len_bytes()
    }

    fn next_boundary(&self, at: usize) -> usize {
        next_grapheme(self.slice(..), at.min(self.len_bytes()))
    }

    fn prev_boundary(&self, at: usize) -> usize {
        prev_grapheme(self.slice(..), at.min(self.len_bytes()))
    }

    fn floor_boundary(&self, at: usize) -> usize {
        let at = at.min(self.len_bytes());
        self.char_to_byte(self.byte_to_char(at))
    }
}

impl Text for crate::Buffer {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.rope().next_boundary(at)
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.rope().prev_boundary(at)
    }

    fn floor_boundary(&self, at: usize) -> usize {
        self.rope().floor_boundary(at)
    }
}

/// The grapheme boundary after `at` in `text`, read a chunk at a time (as helix does).
pub fn next_grapheme(text: RopeSlice, at: usize) -> usize {
    let len = text.len_bytes();
    if at >= len {
        return len;
    }
    let (mut chunk, mut chunk_start, _, _) = text.chunk_at_byte(at);
    let mut cursor = GraphemeCursor::new(at, len, true);
    loop {
        match cursor.next_boundary(chunk, chunk_start) {
            Ok(None) => return len,
            Ok(Some(next)) => return next,
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                chunk = text.chunk_at_byte(chunk_start).0;
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let before = text.chunk_at_byte(n - 1).0;
                cursor.provide_context(before, n - before.len());
            }
            Err(_) => return len,
        }
    }
}

/// The grapheme boundary before `at` in `text`.
pub fn prev_grapheme(text: RopeSlice, at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    let (mut chunk, mut chunk_start, _, _) = text.chunk_at_byte(at - 1);
    let mut cursor = GraphemeCursor::new(at, text.len_bytes(), true);
    loop {
        match cursor.prev_boundary(chunk, chunk_start) {
            Ok(None) => return 0,
            Ok(Some(prev)) => return prev,
            Err(GraphemeIncomplete::PrevChunk) => {
                let (before, start, _, _) = text.chunk_at_byte(chunk_start - 1);
                (chunk, chunk_start) = (before, start);
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let before = text.chunk_at_byte(n - 1).0;
                cursor.provide_context(before, n - before.len());
            }
            Err(_) => return 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rope_steps_over_characters_as_a_string_does() {
        // Long enough to be several chunks, with characters across their edges.
        let text = "aé👍🏽 e\u{301}x\r\n🇸🇪".repeat(500);
        let rope = Rope::from_str(&text);
        let mut at = 0;
        while at < text.len() {
            let next = text.as_str().next_boundary(at);
            assert_eq!(rope.next_boundary(at), next, "after {at}");
            assert_eq!(rope.prev_boundary(next), text.as_str().prev_boundary(next), "before {next}");
            at = next;
        }
        assert_eq!(rope.floor_boundary(2), 1, "inside `é`");
    }
}
