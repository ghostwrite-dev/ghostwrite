//! Match mode (`m`): matching brackets, surrounding pairs, and the `mi`/`ma`
//! text objects.

use crate::buffer::Buffer;
use crate::movement;
use crate::selection::Range;

const BRACKETS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// The pair a character names in `m` commands: brackets by either side,
/// anything else (quotes included) pairs with itself.
pub fn pair_of(c: char) -> (char, char) {
    BRACKETS.into_iter().find(|(open, close)| *open == c || *close == c).unwrap_or((c, c))
}

/// `mm`: the bracket matching the one at `at`.
pub fn matching_bracket(b: &Buffer, at: usize) -> Option<usize> {
    let c = b.char_at(at)?;
    let (open, close) = BRACKETS.into_iter().find(|(o, cl)| *o == c || *cl == c)?;
    let mut depth = 0i32;
    let mut step = |(i, ch): (usize, char), toward: char, away: char| {
        if ch == away {
            depth += 1;
        } else if ch == toward {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        None
    };
    if c == open { b.chars_at(at).find_map(|ch| step(ch, close, open)) } else { b.chars_before(b.next(at)).find_map(|ch| step(ch, open, close)) }
}

/// The nearest `open`…`close` pair around `from..to`: where the open and
/// close chars are. Same-char pairs (quotes) are looked for on the line.
pub fn surrounding(b: &Buffer, from: usize, to: usize, (open, close): (char, char)) -> Option<(usize, usize)> {
    if open == close {
        let start = b.line_start_at(from);
        let end = b.line_end_at(b.prev(to).max(from));
        let left = b.chars_before(from).take_while(|(i, _)| *i >= start).find(|(_, c)| *c == open)?.0;
        let after = to.max(if from == to { b.next(from) } else { from }).min(end);
        let right = b.chars_at(after).take_while(|(i, _)| *i < end).find(|(_, c)| *c == close)?.0;
        return Some((left, right));
    }
    // On the pair's own bracket (a one-char selection): that pair, as in helix.
    if b.next(from) >= to {
        match b.char_at(from) {
            Some(c) if c == open => return Some((from, matching_bracket(b, from)?)),
            Some(c) if c == close => return Some((matching_bracket(b, from)?, from)),
            _ => {}
        }
    }
    let mut depth = 0i32;
    let mut left = None;
    for (i, ch) in b.chars_before(from) {
        if ch == close {
            depth += 1;
        } else if ch == open && depth == 0 {
            left = Some(i);
            break;
        } else if ch == open {
            depth -= 1;
        }
    }
    let left = left?;
    let right = matching_bracket(b, left)?;
    (right >= b.prev(to).max(from)).then_some((left, right))
}

/// The innermost bracket pair of any kind around `from..to` (`mim` / `mam`).
pub fn nearest_pair(b: &Buffer, from: usize, to: usize) -> Option<(usize, usize)> {
    BRACKETS.into_iter().filter_map(|pair| surrounding(b, from, to, pair)).max_by_key(|(left, _)| *left)
}

/// The pair `which` names around `from..to` (`m`: the nearest of any
/// bracket), `count` pairs out: for `md`, `mr`, `mi`, `ma`.
pub fn pair_around(b: &Buffer, from: usize, to: usize, which: char, count: usize) -> Option<(usize, usize)> {
    let find = |from: usize, to: usize| if which == 'm' { nearest_pair(b, from, to) } else { surrounding(b, from, to, pair_of(which)) };
    let mut found = find(from, to)?;
    for _ in 1..count {
        found = find(found.0, b.next(found.1))?;
    }
    Some(found)
}

/// `mi`/`ma`: a word, WORD, paragraph (`count` of them), or pair (`count`
/// out), inside or around. As in helix, any other letter or digit names
/// an object of a syntax tree or a diff, which there aren't here, and
/// anything else is a pair of itself.
pub fn text_object(b: &Buffer, r: Range, object: char, around: bool, count: usize) -> Option<Range> {
    let cursor = r.cursor(b);
    match object {
        'w' | 'W' => {
            let long = object == 'W';
            let is_word = |c: char| if long { !c.is_whitespace() } else { c.is_alphanumeric() || c == '_' };
            if !is_word(b.char_at(cursor)?) {
                return None;
            }
            let blank = |(_, c): &(usize, char)| matches!(c, ' ' | '\t');
            let mut start = b.chars_before(cursor).take_while(|(_, c)| is_word(*c)).last().map_or(cursor, |(i, _)| i);
            let mut end = b.chars_at(cursor).find(|(_, c)| !is_word(*c)).map_or(b.len(), |(i, _)| i);
            if around {
                let after = b.chars_at(end).find(|c| !blank(c)).map_or(b.len(), |(i, _)| i) - end;
                // None after (the line's last word): the space before goes instead.
                if after == 0 {
                    start = b.chars_before(start).find(|c| !blank(c)).map_or(0, |(i, c)| i + c.len_utf8());
                }
                end += after;
            }
            Some(Range::new(start, end))
        }
        'p' => Some(paragraph(b, r, around, count)),
        c if c.is_ascii_alphanumeric() && c != 'm' => None,
        c => {
            let (left, right) = pair_around(b, r.from(), r.to(), c, count)?;
            Some(if around { Range::new(left, b.next(right)) } else { Range::new(b.next(left), right) })
        }
    }
}

/// `mip`, `map`: the paragraph (and `count - 1` after it), and around it
/// the blank lines after; as helix's `textobject_paragraph`.
fn paragraph(b: &Buffer, r: Range, around: bool, count: usize) -> Range {
    let (empty, lines) = (|line: usize| movement::empty_line(b, line), b.line_count());
    let r = r.min_width_1(b);
    let cursor = r.cursor(b);
    let mut line = b.line_of(cursor);
    let curr_line_empty = empty(line);
    let last_char = b.prev(movement::start_of(b, line + 1)) == cursor;
    let prev_empty_to_line = empty(line.saturating_sub(1)) && !curr_line_empty;
    let curr_empty_to_line = curr_line_empty && !(line + 1 >= lines || empty(line + 1));
    // Back over blank lines, then the paragraph before them.
    let back = |mut line: usize| {
        while line > 0 && empty(line - 1) {
            line -= 1;
        }
        while line > 0 && !empty(line - 1) {
            line -= 1;
        }
        line
    };
    let mut first = line + (prev_empty_to_line || curr_empty_to_line) as usize;
    if !(curr_empty_to_line && last_char) {
        first = back(first);
    } else {
        line += 1;
    }
    let mut paragraphs = 0;
    for _ in 0..count {
        let start = line;
        while line < lines && !empty(line) {
            line += 1;
        }
        paragraphs += (line > start) as usize;
        while line < lines && empty(line) {
            line += 1;
        }
    }
    // At the end with fewer paragraphs than asked for: the one before too.
    if paragraphs != count && line >= lines {
        first = back(first);
    }
    if !around {
        while line > 0 && empty(line - 1) {
            line -= 1;
        }
    }
    Range::new(movement::start_of(b, first), movement::start_of(b, line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_mode_brackets_and_pairs() {
        let b = Buffer::new("f(a, [b, c], \"q\")");
        assert_eq!(matching_bracket(&b, 1), Some(16));
        assert_eq!(matching_bracket(&b, 16), Some(1));
        assert_eq!(matching_bracket(&b, 5), Some(10));
        // Inside the [...]: the nearest pair is the brackets, then the parens.
        assert_eq!(nearest_pair(&b, 6, 7), Some((5, 10)));
        assert_eq!(surrounding(&b, 6, 7, pair_of('(')), Some((1, 16)));
        assert_eq!(surrounding(&b, 14, 15, pair_of('"')), Some((13, 15)));
        assert_eq!(text_object(&b, Range::point(6), '[', false, 1), Some(Range::new(6, 10)));
        assert_eq!(text_object(&b, Range::point(6), ']', true, 1), Some(Range::new(5, 11)));
        assert_eq!(text_object(&b, Range::point(6), 'm', false, 1), Some(Range::new(6, 10)));
    }

    #[test]
    fn word_and_paragraph_objects() {
        let b = Buffer::new("one two_x  three\n\nnext para\n");
        assert_eq!(text_object(&b, Range::point(5), 'w', false, 1), Some(Range::new(4, 9)));
        assert_eq!(text_object(&b, Range::point(5), 'w', true, 1), Some(Range::new(4, 11)));
        assert_eq!(text_object(&b, Range::point(2), 'p', false, 1), Some(Range::new(0, 17)));
        assert_eq!(text_object(&b, Range::point(2), 'p', true, 1), Some(Range::new(0, 18)));
        assert_eq!(text_object(&b, Range::point(2), 'p', false, 2), Some(Range::new(0, 28)), "two paragraphs");
    }

    #[test]
    fn letters_arent_pairs_and_counts_go_out() {
        let b = Buffer::new("f(a, [b, x], q)");
        assert_eq!(text_object(&b, Range::point(6), 'x', false, 1), None, "a letter names a syntax tree's object");
        assert_eq!(text_object(&b, Range::point(6), '(', false, 1), Some(Range::new(2, 14)));
        assert_eq!(text_object(&b, Range::point(6), 'm', true, 2), Some(Range::new(1, 15)), "the second pair out");
        assert_eq!(text_object(&b, Range::point(6), '[', false, 2), None, "there's no second");
        let b = Buffer::new("say |a| and |b|");
        assert_eq!(text_object(&b, Range::point(5), '|', false, 1), Some(Range::new(5, 6)), "punctuation pairs with itself");
    }
}
