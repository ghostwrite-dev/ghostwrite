//! Helix-style modal editing: the pure half. Positions are char indices into
//! a [`Doc`]; selections follow helix's `Range { anchor, head }`, where a
//! range of width one is the block cursor. Word motions are ported from
//! `helix-core/src/movement.rs`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Select,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NOR",
            Mode::Insert => "INS",
            Mode::Select => "SEL",
        }
    }
}

/// A snapshot of the text, indexable by char with byte offsets alongside.
pub struct Doc {
    chars: Vec<char>,
    bytes: Vec<usize>,
}

impl Doc {
    pub fn new(text: &str) -> Self {
        let mut chars = Vec::with_capacity(text.len());
        let mut bytes = Vec::with_capacity(text.len() + 1);
        for (b, c) in text.char_indices() {
            chars.push(c);
            bytes.push(b);
        }
        bytes.push(text.len());
        Self { chars, bytes }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn byte(&self, ci: usize) -> usize {
        self.bytes[ci.min(self.len())]
    }

    pub fn char_of_byte(&self, b: usize) -> usize {
        self.bytes.partition_point(|&x| x < b)
    }

    pub fn slice(&self, from: usize, to: usize) -> String {
        self.chars[from.min(self.len())..to.min(self.len())].iter().collect()
    }

    pub fn line_start(&self, ci: usize) -> usize {
        let ci = ci.min(self.len());
        self.chars[..ci].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
    }

    /// Index of the line's `\n`, or the end of the text on the last line.
    pub fn line_end(&self, ci: usize) -> usize {
        let ci = ci.min(self.len());
        self.chars[ci..].iter().position(|&c| c == '\n').map_or(self.len(), |i| ci + i)
    }

    pub fn line_of(&self, ci: usize) -> usize {
        self.chars[..ci.min(self.len())].iter().filter(|&&c| c == '\n').count()
    }

    pub fn line_count(&self) -> usize {
        self.chars.iter().filter(|&&c| c == '\n').count() + 1
    }

    /// The last line holding text: a trailing newline doesn't start one.
    pub fn last_line(&self) -> usize {
        let lines = self.line_count();
        if lines > 1 && self.chars.last() == Some(&'\n') { lines - 2 } else { lines - 1 }
    }

    /// Start of the zero-based line `line`, clamped to the last line.
    pub fn nth_line_start(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let mut seen = 0;
        for (i, &c) in self.chars.iter().enumerate() {
            if c == '\n' {
                seen += 1;
                if seen == line {
                    return i + 1;
                }
            }
        }
        self.line_start(self.len())
    }

    pub fn first_non_whitespace(&self, ci: usize) -> usize {
        let start = self.line_start(ci);
        let end = self.line_end(ci);
        (start..end).find(|&i| !matches!(self.chars[i], ' ' | '\t')).unwrap_or(end)
    }

    pub fn indent(&self, ci: usize) -> String {
        let start = self.line_start(ci);
        self.slice(start, self.first_non_whitespace(ci))
    }
}

/// The file's indent: `(width, hard_tabs)`. Like helix's auto-detection, it
/// counts the steps between consecutive lines' indents and takes the most
/// common of 2, 4 or 8; a file that's mostly tab-indented keeps tabs; with
/// nothing to go on it's 4 spaces.
pub fn detect_indent(text: &str) -> (usize, bool) {
    let (mut tabs, mut spaced, mut steps) = (0, 0, [0usize; 9]);
    let mut prev = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > 0 {
            spaced += 1;
        }
        let step = indent.abs_diff(prev);
        if (1..=8).contains(&step) {
            steps[step] += 1;
        }
        prev = indent;
    }
    if tabs > spaced {
        return (4, true);
    }
    let width = [4, 2, 8].into_iter().max_by_key(|&w| (steps[w], w == 4)).filter(|&w| steps[w] > 0).unwrap_or(4);
    (width, false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The char the block cursor sits on.
    pub fn cursor(&self) -> usize {
        if self.head > self.anchor { self.head - 1 } else { self.head }
    }

    /// Helix treats an empty range as covering the char under the cursor.
    pub fn min_width_1(&self, doc: &Doc) -> Self {
        if self.anchor == self.head && self.head < doc.len() {
            Self::new(self.anchor, self.head + 1)
        } else {
            *self
        }
    }

    pub fn flip(&self) -> Self {
        Self::new(self.head, self.anchor)
    }

    /// Move the cursor to `at`, extending from the anchor when `extend` is
    /// set (`Range::put_cursor` in helix).
    pub fn put_cursor(&self, doc: &Doc, at: usize, extend: bool) -> Self {
        if !extend {
            return Self::point(at);
        }
        let next = |i: usize| (i + 1).min(doc.len());
        let anchor = if self.head >= self.anchor && at < self.anchor {
            next(self.anchor)
        } else if self.head < self.anchor && at >= self.anchor {
            self.anchor.saturating_sub(1)
        } else {
            self.anchor
        };
        if anchor <= at { Self::new(anchor, next(at)) } else { Self::new(anchor, at) }
    }
}

pub fn move_horizontally(doc: &Doc, r: Range, forward: bool, count: usize, extend: bool) -> Range {
    let cursor = r.cursor();
    let at = if forward { (cursor + count).min(doc.len()) } else { cursor.saturating_sub(count) };
    r.put_cursor(doc, at, extend)
}

/// Returns the new range and the column to keep for the next vertical move.
pub fn move_vertically(doc: &Doc, r: Range, down: bool, count: usize, extend: bool, sticky: Option<usize>) -> (Range, usize) {
    let cursor = r.cursor();
    let col = sticky.unwrap_or(cursor - doc.line_start(cursor));
    let line = doc.line_of(cursor);
    let target = if down { (line + count).min(doc.line_count() - 1) } else { line.saturating_sub(count) };
    let start = doc.nth_line_start(target);
    let at = (start + col).min(doc.line_end(start));
    (r.put_cursor(doc, at, extend), col)
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
    if c == '\n' || c == '\r' {
        Category::Eol
    } else if c.is_whitespace() {
        Category::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        Category::Word
    } else {
        Category::Punctuation
    }
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

fn is_eol(c: char) -> bool {
    c == '\n' || c == '\r'
}

fn reached_target(target: Word, prev: char, next: char) -> bool {
    match target {
        Word::NextStart => is_word_boundary(prev, next) && (is_eol(next) || !next.is_whitespace()),
        Word::NextEnd | Word::PrevStart => is_word_boundary(prev, next) && (!prev.is_whitespace() || is_eol(next)),
        Word::NextLongStart => is_long_word_boundary(prev, next) && (is_eol(next) || !next.is_whitespace()),
        Word::NextLongEnd | Word::PrevLongStart => {
            is_long_word_boundary(prev, next) && (!prev.is_whitespace() || is_eol(next))
        }
    }
}

/// `CharHelpers::range_to_target` from helix, over a char slice.
fn range_to_target(chars: &[char], target: Word, origin: Range) -> Range {
    let prev = target.is_prev();
    // `pos` is the gap index the iterator stands at, as in helix's `Chars`.
    let mut pos = origin.head;
    let next_char = |pos: &mut usize| -> Option<char> {
        if prev {
            (*pos > 0).then(|| {
                *pos -= 1;
                chars[*pos]
            })
        } else {
            chars.get(*pos).copied().inspect(|_| *pos += 1)
        }
    };
    let advance = |idx: &mut usize| if prev { *idx = idx.saturating_sub(1) } else { *idx += 1 };

    let mut anchor = origin.anchor;
    let mut head = origin.head;
    let mut prev_ch = if prev { chars.get(pos).copied() } else { pos.checked_sub(1).map(|i| chars[i]) };

    // Skip any initial newline characters.
    loop {
        let save = pos;
        match next_char(&mut pos) {
            Some(c) if is_eol(c) => {
                prev_ch = Some(c);
                advance(&mut head);
            }
            Some(_) => {
                pos = save;
                break;
            }
            None => break,
        }
    }
    if prev_ch.is_some_and(is_eol) {
        anchor = head;
    }

    let head_start = head;
    while let Some(next_ch) = next_char(&mut pos) {
        if prev_ch.is_none_or(|p| reached_target(target, p, next_ch)) {
            if head == head_start {
                anchor = head;
            } else {
                break;
            }
        }
        prev_ch = Some(next_ch);
        advance(&mut head);
    }
    Range::new(anchor, head)
}

/// `word_move` from helix: the range selecting the next word in `target`'s direction.
pub fn word_move(doc: &Doc, r: Range, count: usize, target: Word) -> Range {
    let len = doc.len();
    if (target.is_prev() && r.head == 0) || (!target.is_prev() && r.head == len) {
        return r;
    }
    let next = |i: usize| (i + 1).min(len);
    let prev = |i: usize| i.saturating_sub(1);
    let start = if target.is_prev() {
        if r.anchor < r.head { Range::new(r.head, prev(r.head)) } else { Range::new(next(r.head), r.head) }
    } else if r.anchor < r.head {
        Range::new(prev(r.head), r.head)
    } else {
        Range::new(r.head, next(r.head))
    };
    let mut range = start;
    for _ in 0..count {
        let n = range_to_target(&doc.chars, target, range);
        if n == range {
            break;
        }
        range = n;
    }
    range
}

/// `f`/`t`/`F`/`T`: select up to (`till`) or through the `count`th `ch` on the line.
pub fn find_char(doc: &Doc, r: Range, ch: char, forward: bool, till: bool, count: usize, extend: bool) -> Option<Range> {
    let cursor = r.cursor();
    let (start, end) = (doc.line_start(cursor), doc.line_end(cursor));
    let mut hits = if forward {
        Box::new((cursor + 1 + till as usize..end).filter(|&i| doc.chars[i] == ch)) as Box<dyn Iterator<Item = usize>>
    } else {
        Box::new((start..cursor.saturating_sub(till as usize)).rev().filter(|&i| doc.chars[i] == ch))
    };
    let mut at = hits.nth(count - 1)?;
    if till {
        at = if forward { at - 1 } else { at + 1 };
    }
    Some(if extend {
        r.put_cursor(doc, at, true)
    } else if forward {
        Range::new(cursor, at + 1)
    } else {
        Range::new(cursor + 1, at)
    })
}

/// `x`: select the whole line, or the next one if lines are already selected.
pub fn extend_line_below(doc: &Doc, r: Range, count: usize) -> Range {
    let (from, to) = (r.from(), r.to());
    let start = doc.line_start(from);
    let full = from == start && to > from && (to == doc.len() || doc.chars[to - 1] == '\n');
    let next_line = |i: usize| (doc.line_end(i) + 1).min(doc.len());
    let mut head = if full { to } else { next_line(if to > from { to - 1 } else { to }) };
    for _ in 0..if full { count } else { count - 1 } {
        if head >= doc.len() {
            break;
        }
        head = next_line(head);
    }
    Range::new(start, head)
}

/// The pair a character names in `m` commands: brackets by either side,
/// anything else (quotes included) pairs with itself.
pub fn pair_of(c: char) -> (char, char) {
    match c {
        '(' | ')' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        c => (c, c),
    }
}

const BRACKETS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// `mm`: the bracket matching the one under the cursor.
pub fn matching_bracket(doc: &Doc, at: usize) -> Option<usize> {
    let c = *doc.chars.get(at)?;
    let (open, close) = BRACKETS.into_iter().find(|(o, c2)| *o == c || *c2 == c)?;
    let mut depth = 0i32;
    if c == open {
        for i in at..doc.len() {
            match doc.chars[i] {
                x if x == open => depth += 1,
                x if x == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    } else {
        for i in (0..=at).rev() {
            match doc.chars[i] {
                x if x == close => depth += 1,
                x if x == open => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// The nearest `open`…`close` pair around chars `from..to`: the positions of
/// the open and close chars. Same-char pairs (quotes) look on the line.
pub fn surrounding(doc: &Doc, from: usize, to: usize, (open, close): (char, char)) -> Option<(usize, usize)> {
    if open == close {
        let (start, end) = (doc.line_start(from), doc.line_end(to.saturating_sub(1).max(from)));
        let left = (start..from).rev().find(|&i| doc.chars[i] == open)?;
        let right = (to.max(from + usize::from(from == to))..end).find(|&i| doc.chars[i] == close)?;
        return Some((left, right));
    }
    let mut depth = 0i32;
    let mut left = None;
    for i in (0..from).rev() {
        match doc.chars[i] {
            x if x == close => depth += 1,
            x if x == open && depth == 0 => {
                left = Some(i);
                break;
            }
            x if x == open => depth -= 1,
            _ => {}
        }
    }
    let left = left?;
    let right = matching_bracket(doc, left)?;
    (right >= to.saturating_sub(1).max(from)).then_some((left, right))
}

/// The innermost bracket pair of any kind around `from..to` (`mim` / `mam`).
pub fn nearest_pair(doc: &Doc, from: usize, to: usize) -> Option<(usize, usize)> {
    BRACKETS.into_iter().filter_map(|p| surrounding(doc, from, to, p)).max_by_key(|(l, _)| *l)
}

/// `mi`/`ma` text objects: a word, WORD, paragraph, or a pair.
pub fn text_object(doc: &Doc, r: Range, object: char, around: bool) -> Option<Range> {
    let cursor = r.cursor();
    match object {
        'w' | 'W' => {
            let long = object == 'W';
            let is_word = |c: char| if long { !c.is_whitespace() } else { c.is_alphanumeric() || c == '_' };
            let at = *doc.chars.get(cursor)?;
            if !is_word(at) {
                return None;
            }
            let mut start = cursor;
            while start > 0 && is_word(doc.chars[start - 1]) {
                start -= 1;
            }
            let mut end = cursor + 1;
            while end < doc.len() && is_word(doc.chars[end]) {
                end += 1;
            }
            if around {
                while end < doc.len() && matches!(doc.chars[end], ' ' | '\t') {
                    end += 1;
                }
            }
            Some(Range::new(start, end))
        }
        'p' => {
            let blank = |line_start: usize| doc.line_end(line_start) == doc.first_non_whitespace(line_start);
            let mut start = doc.line_start(cursor);
            while start > 0 && !blank(doc.line_start(start - 1)) {
                start = doc.line_start(start - 1);
            }
            let mut end = (doc.line_end(cursor) + 1).min(doc.len());
            while end < doc.len() && !blank(end) {
                end = (doc.line_end(end) + 1).min(doc.len());
            }
            if around {
                while end < doc.len() && blank(end) {
                    end = (doc.line_end(end) + 1).min(doc.len());
                }
            }
            Some(Range::new(start, end))
        }
        c => {
            let (l, rr) = if c == 'm' { nearest_pair(doc, r.from(), r.to())? } else { surrounding(doc, r.from(), r.to(), pair_of(c))? };
            Some(if around { Range::new(l, rr + 1) } else { Range::new(l + 1, rr) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, r: Range, t: Word) -> Range {
        word_move(&Doc::new(text), r, 1, t)
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
    fn vertical_keeps_column() {
        let doc = Doc::new("abcdef\nab\nabcdef");
        let (r, col) = move_vertically(&doc, Range::point(4), true, 1, false, None);
        assert_eq!(r, Range::point(9));
        let (r, _) = move_vertically(&doc, r, true, 1, false, Some(col));
        assert_eq!(r, Range::point(14));
    }

    #[test]
    fn line_selection_extends() {
        let doc = Doc::new("one\ntwo\nthree");
        let r = extend_line_below(&doc, Range::point(1), 1);
        assert_eq!(r, Range::new(0, 4));
        assert_eq!(extend_line_below(&doc, r, 1), Range::new(0, 8));
        assert_eq!(extend_line_below(&doc, Range::new(0, 8), 1), Range::new(0, 13));
    }

    #[test]
    fn detects_indent() {
        assert_eq!(detect_indent("def f():\n    x = 1\n    if x:\n        y\n"), (4, false));
        assert_eq!(detect_indent("{\n  a: 1,\n  b: {\n    c\n  }\n}\n"), (2, false));
        assert_eq!(detect_indent("fn f() {\n\tx();\n}\n"), (4, true));
        assert_eq!(detect_indent("no indent here\n"), (4, false));
    }

    #[test]
    fn find_char_on_line() {
        let doc = Doc::new("let x = foo(bar);");
        assert_eq!(find_char(&doc, Range::point(0), '(', true, false, 1, false), Some(Range::new(0, 12)));
        assert_eq!(find_char(&doc, Range::point(0), '(', true, true, 1, false), Some(Range::new(0, 11)));
        assert_eq!(find_char(&doc, Range::point(0), 'z', true, false, 1, false), None);
    }

    #[test]
    fn extend_put_cursor_flips_over_anchor() {
        let doc = Doc::new("abcdef");
        let r = Range::new(2, 3);
        assert_eq!(r.put_cursor(&doc, 4, true), Range::new(2, 5));
        assert_eq!(r.put_cursor(&doc, 0, true), Range::new(3, 0));
    }

    #[test]
    fn match_mode_brackets_and_pairs() {
        let doc = Doc::new("f(a, [b, c], \"q\")");
        assert_eq!(matching_bracket(&doc, 1), Some(16));
        assert_eq!(matching_bracket(&doc, 16), Some(1));
        assert_eq!(matching_bracket(&doc, 5), Some(10));
        // Inside the [...]: the nearest pair is the brackets, then the parens.
        assert_eq!(nearest_pair(&doc, 6, 7), Some((5, 10)));
        assert_eq!(surrounding(&doc, 6, 7, pair_of('(')), Some((1, 16)));
        assert_eq!(surrounding(&doc, 14, 15, pair_of('"')), Some((13, 15)));
        assert_eq!(text_object(&doc, Range::point(6), '[', false), Some(Range::new(6, 10)));
        assert_eq!(text_object(&doc, Range::point(6), ']', true), Some(Range::new(5, 11)));
        assert_eq!(text_object(&doc, Range::point(6), 'm', false), Some(Range::new(6, 10)));
    }

    #[test]
    fn word_and_paragraph_objects() {
        let doc = Doc::new("one two_x  three\n\nnext para\n");
        assert_eq!(text_object(&doc, Range::point(5), 'w', false), Some(Range::new(4, 9)));
        assert_eq!(text_object(&doc, Range::point(5), 'w', true), Some(Range::new(4, 11)));
        assert_eq!(text_object(&doc, Range::point(2), 'p', false), Some(Range::new(0, 17)));
        assert_eq!(text_object(&doc, Range::point(2), 'p', true), Some(Range::new(0, 18)));
    }
}
