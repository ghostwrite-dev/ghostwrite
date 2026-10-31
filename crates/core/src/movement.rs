//! Motions: where a selection goes for `h j k l`, `w b e`, `f t`, `x` and
//! friends. Word motions are ported from `helix-core/src/movement.rs`.

use crate::buffer::Buffer;
use crate::selection::Range;

pub fn move_horizontally(b: &Buffer, r: Range, forward: bool, count: usize, extend: bool) -> Range {
    let mut at = r.cursor(b.text());
    for _ in 0..count {
        let next = if forward { b.next(at) } else { b.prev(at) };
        if next == at {
            break;
        }
        at = next;
    }
    r.put_cursor(b.text(), at, extend)
}

/// Returns the new range and the column to keep for the next vertical move.
/// The column is where it is on screen: tabs to their stops (`tab_width`),
/// wide characters two columns.
pub fn move_vertically(b: &Buffer, r: Range, down: bool, count: usize, extend: bool, sticky: Option<usize>, tab_width: usize) -> (Range, usize) {
    use unicode_segmentation::UnicodeSegmentation as _;
    let cursor = r.cursor(b.text());
    let line = b.line_of(cursor);
    let col = sticky.unwrap_or_else(|| crate::display_width(&b.text()[b.line_start(line)..cursor], tab_width));
    let target = if down { line.saturating_add(count).min(b.line_count() - 1) } else { line.saturating_sub(count) };
    let (start, end) = (b.line_start(target), b.line_end(target));
    // The character covering that column, or the line's end.
    let row = &b.text()[start..end];
    let at = row.grapheme_indices(true).find(|(i, g)| crate::display_width(&row[..i + g.len()], tab_width) > col).map_or(end, |(i, _)| start + i);
    (r.put_cursor(b.text(), at, extend), col)
}

/// The last char before the line break; the line start on an empty line.
pub fn line_last_char(b: &Buffer, at: usize) -> usize {
    let (start, end) = (b.line_start_at(at), b.line_end_at(at));
    if end > start { b.prev(end) } else { start }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    NextStart,
    NextEnd,
    PrevStart,
    NextLongStart,
    NextLongEnd,
    PrevLongStart,
}

impl Word {
    fn is_prev(self) -> bool {
        matches!(self, Word::PrevStart | Word::PrevLongStart)
    }
}

#[derive(PartialEq, Eq)]
enum Category {
    Whitespace,
    Eol,
    Word,
    Punctuation,
}

fn categorize(c: char) -> Category {
    if is_eol(c) {
        Category::Eol
    } else if c.is_whitespace() {
        Category::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        Category::Word
    } else {
        Category::Punctuation
    }
}

fn is_eol(c: char) -> bool {
    c == '\n' || c == '\r'
}

fn is_word_boundary(a: char, b: char) -> bool {
    categorize(a) != categorize(b)
}

fn is_long_word_boundary(a: char, b: char) -> bool {
    match (categorize(a), categorize(b)) {
        (Category::Word, Category::Punctuation) | (Category::Punctuation, Category::Word) => false,
        (a, b) => a != b,
    }
}

fn reached_target(target: Word, prev: char, next: char) -> bool {
    match target {
        Word::NextStart => is_word_boundary(prev, next) && (is_eol(next) || !next.is_whitespace()),
        Word::NextEnd | Word::PrevStart => is_word_boundary(prev, next) && (!prev.is_whitespace() || is_eol(next)),
        Word::NextLongStart => is_long_word_boundary(prev, next) && (is_eol(next) || !next.is_whitespace()),
        Word::NextLongEnd | Word::PrevLongStart => is_long_word_boundary(prev, next) && (!prev.is_whitespace() || is_eol(next)),
    }
}

/// `CharHelpers::range_to_target` from helix. The head walks one char at a
/// time in the target's direction until it reaches the target.
fn range_to_target(text: &str, target: Word, origin: Range) -> Range {
    let back = target.is_prev();
    // The char the head steps over next, and where it lands.
    let step = |at: usize| -> Option<(char, usize)> {
        if back {
            let c = text[..at].chars().next_back()?;
            Some((c, at - c.len_utf8()))
        } else {
            let c = text[at..].chars().next()?;
            Some((c, at + c.len_utf8()))
        }
    };
    let mut anchor = origin.anchor;
    let mut head = origin.head;
    let mut prev_ch = if back { text[head..].chars().next() } else { text[..head].chars().next_back() };

    // Skip any initial line breaks.
    while let Some((c, next)) = step(head) {
        if !is_eol(c) {
            break;
        }
        prev_ch = Some(c);
        head = next;
    }
    if prev_ch.is_some_and(is_eol) {
        anchor = head;
    }

    let head_start = head;
    while let Some((next_ch, next)) = step(head) {
        if prev_ch.is_none_or(|p| reached_target(target, p, next_ch)) {
            if head == head_start {
                anchor = head;
            } else {
                break;
            }
        }
        prev_ch = Some(next_ch);
        head = next;
    }
    Range::new(anchor, head)
}

/// `word_move` from helix: the range selecting the next word in `target`'s direction.
pub fn word_move(b: &Buffer, r: Range, count: usize, target: Word) -> Range {
    let text = b.text();
    if (target.is_prev() && r.head == 0) || (!target.is_prev() && r.head == text.len()) {
        return r;
    }
    let start = if target.is_prev() {
        if r.anchor < r.head { Range::new(r.head, b.prev(r.head)) } else { Range::new(b.next(r.head), r.head) }
    } else if r.anchor < r.head {
        Range::new(b.prev(r.head), r.head)
    } else {
        Range::new(r.head, b.next(r.head))
    };
    let mut range = start;
    for _ in 0..count {
        let next = range_to_target(text, target, range);
        if next == range {
            break;
        }
        range = next;
    }
    range
}

/// `f`/`t`/`F`/`T`: select up to (`till`) or through the `count`th `ch` on the line.
pub fn find_char(b: &Buffer, r: Range, ch: char, forward: bool, till: bool, count: usize, extend: bool) -> Option<Range> {
    let text = b.text();
    let cursor = r.cursor(text);
    let (start, end) = (b.line_start_at(cursor), b.line_end_at(cursor));
    let at = if forward {
        let mut from = b.next(cursor);
        if till {
            from = b.next(from);
        }
        let from = from.min(end);
        let found = text[from..end].char_indices().filter(|(_, c)| *c == ch).nth(count - 1)?.0 + from;
        if till { b.prev(found) } else { found }
    } else {
        let upto = if till { b.prev(cursor) } else { cursor }.max(start);
        let found = text[start..upto].char_indices().rev().filter(|(_, c)| *c == ch).nth(count - 1)?.0 + start;
        if till { b.next(found) } else { found }
    };
    Some(if extend {
        r.put_cursor(text, at, true)
    } else if forward {
        Range::new(cursor, b.next(at))
    } else {
        Range::new(b.next(cursor), at)
    })
}

/// `x`: select the whole line, or the next one if lines are already selected.
pub fn extend_line_below(b: &Buffer, r: Range, count: usize) -> Range {
    let (from, to) = (r.from(), r.to());
    let len = b.len();
    let start = b.line_start_at(from);
    let full = from == start && to > from && (to == len || b.text()[..to].ends_with('\n'));
    let next_line = |at: usize| (b.line_end_at(at) + 1).min(len);
    let mut head = if full { to } else { next_line(if to > from { b.prev(to) } else { to }) };
    for _ in 0..if full { count } else { count - 1 } {
        if head >= len {
            break;
        }
        head = next_line(head);
    }
    Range::new(start, head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, r: Range, t: Word) -> Range {
        word_move(&Buffer::new(text), r, 1, t)
    }

    #[test]
    fn next_word_start_selects_word_and_trailing_space() {
        assert_eq!(w("hello world", Range::point(0), Word::NextStart), Range::new(0, 6));
        assert_eq!(w("hello world", Range::new(0, 6), Word::NextStart), Range::new(6, 11));
    }

    #[test]
    fn word_end_and_prev_word() {
        assert_eq!(w("foo.bar baz", Range::point(0), Word::NextEnd), Range::new(0, 3));
        assert_eq!(w("foo bar", Range::point(6), Word::PrevStart), Range::new(7, 4));
        assert_eq!(w("foo.bar baz", Range::point(0), Word::NextLongEnd), Range::new(0, 7));
    }

    #[test]
    fn word_motion_crosses_lines() {
        assert_eq!(w("foo\n  bar", Range::new(0, 3), Word::NextStart), Range::new(4, 6));
    }

    #[test]
    fn word_motion_steps_over_wide_chars() {
        assert_eq!(w("héllo wörld", Range::point(0), Word::NextStart), Range::new(0, 7));
    }

    #[test]
    fn vertical_keeps_column() {
        let b = Buffer::new("abcdef\nab\nabcdef");
        let (r, col) = move_vertically(&b, Range::point(4), true, 1, false, None, 4);
        assert_eq!(r, Range::point(9));
        let (r, _) = move_vertically(&b, r, true, 1, false, Some(col), 4);
        assert_eq!(r, Range::point(14));
    }

    #[test]
    fn line_selection_extends() {
        let b = Buffer::new("one\ntwo\nthree");
        let r = extend_line_below(&b, Range::point(1), 1);
        assert_eq!(r, Range::new(0, 4));
        assert_eq!(extend_line_below(&b, r, 1), Range::new(0, 8));
        assert_eq!(extend_line_below(&b, Range::new(0, 8), 1), Range::new(0, 13));
    }

    #[test]
    fn find_char_on_line() {
        let b = Buffer::new("let x = foo(bar);");
        assert_eq!(find_char(&b, Range::point(0), '(', true, false, 1, false), Some(Range::new(0, 12)));
        assert_eq!(find_char(&b, Range::point(0), '(', true, true, 1, false), Some(Range::new(0, 11)));
        assert_eq!(find_char(&b, Range::point(0), 'z', true, false, 1, false), None);
        assert_eq!(find_char(&b, Range::point(12), 'x', false, false, 1, false), Some(Range::new(13, 4)));
    }
}
