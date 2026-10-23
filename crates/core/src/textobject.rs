//! Match mode (`m`): matching brackets, surrounding pairs, and the `mi`/`ma`
//! text objects.

use crate::buffer::Buffer;
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
    let text = b.text();
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
    if c == open {
        text[at..].char_indices().find_map(|(i, ch)| step((at + i, ch), close, open))
    } else {
        text[..b.next(at)].char_indices().rev().find_map(|ch| step(ch, open, close))
    }
}

/// The nearest `open`…`close` pair around `from..to`: where the open and
/// close chars are. Same-char pairs (quotes) are looked for on the line.
pub fn surrounding(b: &Buffer, from: usize, to: usize, (open, close): (char, char)) -> Option<(usize, usize)> {
    let text = b.text();
    if open == close {
        let start = b.line_start_at(from);
        let end = b.line_end_at(b.prev(to).max(from));
        let left = text[start..from].rfind(open)? + start;
        let after = to.max(if from == to { b.next(from) } else { from }).min(end);
        let right = text[after..end].find(close)? + after;
        return Some((left, right));
    }
    let mut depth = 0i32;
    let mut left = None;
    for (i, ch) in text[..from].char_indices().rev() {
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

/// `mi`/`ma`: a word, WORD, paragraph, or pair, inside or around.
pub fn text_object(b: &Buffer, r: Range, object: char, around: bool) -> Option<Range> {
    let text = b.text();
    let cursor = r.cursor(text);
    match object {
        'w' | 'W' => {
            let long = object == 'W';
            let is_word = |c: char| if long { !c.is_whitespace() } else { c.is_alphanumeric() || c == '_' };
            if !is_word(b.char_at(cursor)?) {
                return None;
            }
            let start = text[..cursor].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(cursor, |(i, _)| i);
            let mut end = text[cursor..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| cursor + i);
            if around {
                end += text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
            }
            Some(Range::new(start, end))
        }
        'p' => {
            let blank = |line: usize| b.line(line).trim().is_empty();
            let mut first = b.line_of(cursor);
            while first > 0 && !blank(first - 1) {
                first -= 1;
            }
            let mut last = b.line_of(cursor);
            while last < b.last_line() && !blank(last + 1) {
                last += 1;
            }
            if around {
                while last < b.last_line() && blank(last + 1) {
                    last += 1;
                }
            }
            Some(Range::new(b.line_start(first), b.lines_range(last..last + 1).end))
        }
        c => {
            let (left, right) = if c == 'm' { nearest_pair(b, r.from(), r.to())? } else { surrounding(b, r.from(), r.to(), pair_of(c))? };
            Some(if around { Range::new(left, b.next(right)) } else { Range::new(b.next(left), right) })
        }
    }
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
        assert_eq!(text_object(&b, Range::point(6), '[', false), Some(Range::new(6, 10)));
        assert_eq!(text_object(&b, Range::point(6), ']', true), Some(Range::new(5, 11)));
        assert_eq!(text_object(&b, Range::point(6), 'm', false), Some(Range::new(6, 10)));
    }

    #[test]
    fn word_and_paragraph_objects() {
        let b = Buffer::new("one two_x  three\n\nnext para\n");
        assert_eq!(text_object(&b, Range::point(5), 'w', false), Some(Range::new(4, 9)));
        assert_eq!(text_object(&b, Range::point(5), 'w', true), Some(Range::new(4, 11)));
        assert_eq!(text_object(&b, Range::point(2), 'p', false), Some(Range::new(0, 17)));
        assert_eq!(text_object(&b, Range::point(2), 'p', true), Some(Range::new(0, 18)));
    }
}
