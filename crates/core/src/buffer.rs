//! Text, as a rope, with undo history: an edit costs as much wherever it is,
//! however big the file.
//!
//! Edits are grouped into transactions, each one undo step. A transaction
//! opens with [`Buffer::begin`] (or the first edit) and closes with
//! [`Buffer::commit`]; it remembers the selection on either side so undo and
//! redo put it back. Every change is remembered as a syntax tree takes it,
//! so what's worked out from an older version can be moved on to this one.

use std::borrow::Cow;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;
use tree_sitter::{InputEdit, Point};

use crate::selection::Range;
use crate::text::Text as _;

#[derive(Clone, Debug)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
struct Transaction {
    changes: Vec<Change>,
    before: Range,
    after: Range,
}

/// An edit as a language server takes it: the lines and UTF-16 columns it
/// replaced, in the text as it was then, and what went there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub text: String,
}

/// Edits recorded for a language server at most; more, and it's sent the whole text.
const MAX_EDITS: usize = 1_000;

/// Edits remembered for what's kept of an older version (a syntax tree, colors); more, and it's made again.
const MAX_LOG: usize = 10_000;

#[derive(Debug)]
pub struct Buffer {
    text: Rope,
    /// Which buffer this is, for caches of more than one (a syntax tree).
    id: u64,
    /// Bumped on every change, for caches (highlights, language servers).
    version: u64,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    open: Option<Transaction>,
    /// Commits wait: several edits, each committing as it goes, are to be one undo.
    held: bool,
    /// The edits since [`Buffer::take_edits`], or `None` if it's simpler to send the whole text.
    edits: Option<Vec<Edit>>,
    /// Every change since the version it starts at, and the version each made.
    log: (u64, Vec<(u64, InputEdit)>),
    /// The text as one string, once something's asked for it whole (see [`Buffer::whole_text`]).
    whole: OnceLock<String>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl std::fmt::Display for Buffer {
    /// All of the text, as one string: for what needs it whole (saving, a request).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.text.chunks().try_for_each(|chunk| f.write_str(chunk))
    }
}

impl Buffer {
    pub fn new(text: impl AsRef<str>) -> Self {
        static IDS: AtomicU64 = AtomicU64::new(0);
        Self {
            text: Rope::from_str(text.as_ref()),
            id: IDS.fetch_add(1, Ordering::Relaxed),
            version: 0,
            undo: vec![],
            redo: vec![],
            open: None,
            held: false,
            edits: Some(vec![]),
            log: (0, vec![]),
            whole: OnceLock::new(),
        }
    }

    /// The text, as it is now. Cloning it is cheap: a snapshot to read elsewhere.
    pub fn rope(&self) -> &Rope {
        &self.text
    }

    /// The whole text as one string, for what can only read it so (a search's
    /// pattern, placing a model's steps): made when it's first asked for after a
    /// change, and kept till the next. What reads a line or a range reads the rope.
    pub fn whole_text(&self) -> &str {
        self.whole.get_or_init(|| self.to_string())
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn len(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The text in `range` (clamped): borrowed if it's all in one piece of the rope, else copied.
    pub fn slice(&self, range: std::ops::Range<usize>) -> Cow<'_, str> {
        let (start, end) = (range.start.min(self.len()), range.end.min(self.len()));
        let slice = self.text.byte_slice(start..end.max(start));
        slice.as_str().map_or_else(|| Cow::Owned(slice.to_string()), Cow::Borrowed)
    }

    pub fn char_at(&self, at: usize) -> Option<char> {
        if at >= self.len() {
            return None;
        }
        let (chunk, start, _, _) = self.text.chunk_at_byte(at);
        chunk.get(at - start..)?.chars().next()
    }

    /// The chars from `at` on, and where each is.
    pub fn chars_at(&self, at: usize) -> impl Iterator<Item = (usize, char)> + '_ {
        let mut at = at.min(self.len());
        self.text.chars_at(self.text.byte_to_char(at)).map(move |c| {
            at += c.len_utf8();
            (at - c.len_utf8(), c)
        })
    }

    /// The chars before `at`, nearest first, and where each is.
    pub fn chars_before(&self, at: usize) -> impl Iterator<Item = (usize, char)> + '_ {
        let mut at = at.min(self.len());
        self.text.chars_at(self.text.byte_to_char(at)).reversed().map(move |c| {
            at -= c.len_utf8();
            (at, c)
        })
    }

    pub fn is_char_boundary(&self, at: usize) -> bool {
        at == self.len() || (at < self.len() && (self.text.byte(at) & 0xC0) != 0x80)
    }

    pub fn next(&self, at: usize) -> usize {
        self.text.next_boundary(at)
    }

    pub fn prev(&self, at: usize) -> usize {
        self.text.prev_boundary(at)
    }

    // Lines.

    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    /// The last line holding text: a trailing newline doesn't start one.
    pub fn last_line(&self) -> usize {
        let lines = self.line_count();
        if lines > 1 && self.text.byte(self.len() - 1) == b'\n' { lines - 2 } else { lines - 1 }
    }

    /// The zero-based line `at` is on.
    pub fn line_of(&self, at: usize) -> usize {
        self.text.byte_to_line(at.min(self.len()))
    }

    /// Start of `line`, clamped to the last line.
    pub fn line_start(&self, line: usize) -> usize {
        self.text.line_to_byte(line.min(self.line_count() - 1))
    }

    /// End of `line`, before its line break.
    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.line_count() - 1);
        if line + 1 < self.line_count() { self.text.line_to_byte(line + 1) - 1 } else { self.len() }
    }

    /// Char `column` of `line` (as a session keeps the cursor), clamped to the line.
    pub fn at_line_col(&self, line: usize, column: usize) -> usize {
        let (start, end) = (self.line_start(line), self.line_end(line));
        self.chars_at(start).take_while(|(at, _)| *at < end).nth(column).map_or(end, |(at, _)| at)
    }

    /// `line` without its line break.
    pub fn line(&self, line: usize) -> Cow<'_, str> {
        self.slice(self.line_start(line)..self.line_end(line))
    }

    /// Bytes of whole `lines`, line breaks included.
    pub fn lines_range(&self, lines: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let at = |line: usize| if line < self.line_count() { self.text.line_to_byte(line) } else { self.len() };
        at(lines.start)..at(lines.end).max(at(lines.start))
    }

    pub fn line_start_at(&self, at: usize) -> usize {
        self.line_start(self.line_of(at))
    }

    pub fn line_end_at(&self, at: usize) -> usize {
        self.line_end(self.line_of(at))
    }

    pub fn first_non_blank_at(&self, at: usize) -> usize {
        let (start, end) = (self.line_start_at(at), self.line_end_at(at));
        start + self.text.byte_slice(start..end).bytes().position(|b| b != b' ' && b != b'\t').unwrap_or(end - start)
    }

    pub fn indent_at(&self, at: usize) -> Cow<'_, str> {
        self.slice(self.line_start_at(at)..self.first_non_blank_at(at))
    }

    // Editing.

    /// Open a transaction (if none is open) that undoes back to `selection`.
    pub fn begin(&mut self, selection: Range) {
        if self.open.is_none() {
            self.open = Some(Transaction { changes: vec![], before: selection, after: selection });
        }
    }

    /// Replace `range` with `text`, as part of the open transaction.
    pub fn edit(&mut self, range: std::ops::Range<usize>, text: &str) {
        let range = range.start.min(self.len())..range.end.min(self.len());
        if range.is_empty() && text.is_empty() {
            return;
        }
        self.begin(Range::point(range.start));
        let removed = self.slice(range.clone()).into_owned();
        self.replace(range.start, removed.len(), text);
        let change = Change { at: range.start, removed, inserted: text.to_string() };
        self.open.as_mut().expect("begun above").changes.push(change);
        self.redo.clear();
    }
    /// Until [`Buffer::release`], commits wait, and all that's done is one undo step.
    pub fn hold(&mut self) {
        self.held = true;
    }

    /// Commit what's been held.
    pub fn release(&mut self, selection: Range) {
        self.held = false;
        self.commit(selection);
    }

    /// Stop holding, leaving the transaction open (typing goes on).
    pub fn hold_no_longer(&mut self) {
        self.held = false;
    }

    /// Close the open transaction: one undo step, unless it changed nothing.
    pub fn commit(&mut self, selection: Range) {
        if self.held {
            return;
        }
        if let Some(mut tx) = self.open.take()
            && !tx.changes.is_empty()
        {
            tx.after = selection;
            self.undo.push(tx);
        }
    }

    pub fn has_uncommitted(&self) -> bool {
        self.open.as_ref().is_some_and(|tx| !tx.changes.is_empty())
    }

    /// Where the latest change still in the text ended (`g.`): the last one
    /// typed so far, or else the last of the step `u` would undo next.
    pub fn last_edit(&self) -> Option<usize> {
        let open = self.open.as_ref().filter(|tx| !tx.changes.is_empty());
        let change = open.or(self.undo.last())?.changes.last()?;
        Some((change.at + change.inserted.len()).min(self.len()))
    }

    /// Undo the last transaction; returns the selection from before it.
    pub fn undo(&mut self) -> Option<Range> {
        if let Some(open) = &self.open {
            let after = open.after;
            self.commit(after);
        }
        let tx = self.undo.pop()?;
        for change in tx.changes.iter().rev() {
            self.replace(change.at, change.inserted.len(), &change.removed);
        }
        let before = tx.before;
        self.redo.push(tx);
        Some(before)
    }

    /// Redo the last undone transaction; returns the selection from after it.
    pub fn redo(&mut self) -> Option<Range> {
        let tx = self.redo.pop()?;
        for change in &tx.changes {
            self.replace(change.at, change.removed.len(), &change.inserted);
        }
        let after = tx.after;
        self.undo.push(tx);
        Some(after)
    }

    /// Become `text` (a reload from disk) as one undo step: only the part
    /// that differs is replaced, so undo takes it back. Returns where
    /// `selection` is in the new text.
    pub fn replace_all(&mut self, text: &str, selection: Range) -> Range {
        let old = &self.text;
        let mut prefix = old.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        while !self.is_char_boundary(prefix) || !text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let room = self.len().min(text.len()) - prefix;
        let mut suffix = old.bytes_at(self.len()).reversed().zip(text.bytes().rev()).take(room).take_while(|(a, b)| a == b).count();
        while !self.is_char_boundary(self.len() - suffix) || !text.is_char_boundary(text.len() - suffix) {
            suffix -= 1;
        }
        let (end, new_end) = (self.len() - suffix, text.len() - suffix);
        if prefix == end && prefix == new_end {
            return selection;
        }
        // Where a byte of the old text is now: before the change, the same;
        // after it, moved; in it, as far into what replaced it (or its end).
        let inside = |at: usize| {
            let mut at = at.min(new_end);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        let map = |at: usize| {
            if at <= prefix {
                at
            } else if at >= end {
                at - end + new_end
            } else {
                inside(at)
            }
        };
        let after = Range::new(map(selection.anchor), map(selection.head));
        self.commit(selection);
        self.begin(selection);
        self.edit(prefix..end, &text[prefix..new_end]);
        self.commit(after);
        after
    }

    /// The edits since this was last asked, in order, for a language server;
    /// `None` if the whole text should be sent instead.
    pub fn take_edits(&mut self) -> Option<Vec<Edit>> {
        self.edits.replace(vec![])
    }

    /// The changes since `version`, in order, as a syntax tree takes them; `None` if
    /// they aren't all remembered (the text was replaced, or there were too many).
    pub fn changes_since(&self, version: u64) -> Option<Vec<InputEdit>> {
        let (from, log) = &self.log;
        if version < *from || version > self.version {
            return None;
        }
        let first = log.partition_point(|(made, _)| *made <= version);
        Some(log[first..].iter().map(|(_, edit)| *edit).collect())
    }

    /// Where `at` is as a language server counts: its line, and UTF-16 units into it.
    fn lsp_position(&self, at: usize) -> (usize, usize) {
        let line = self.line_of(at);
        (line, self.text.byte_slice(self.line_start(line)..at).chars().map(char::len_utf16).sum())
    }

    /// Where `at` is as a syntax tree counts: its line, and bytes into it.
    fn point(&self, at: usize) -> Point {
        let line = self.line_of(at);
        Point::new(line, at - self.line_start(line))
    }

    /// Replace everything, forgetting the history.
    pub fn reset(&mut self, text: impl AsRef<str>) {
        self.edits = None;
        self.text = Rope::from_str(text.as_ref());
        self.whole = OnceLock::new();
        self.undo.clear();
        self.redo.clear();
        self.open = None;
        self.version += 1;
        self.log = (self.version, vec![]);
    }

    /// Replace `len` bytes at `at` with `text`, noting the change for a
    /// language server and for what's kept of older versions.
    fn replace(&mut self, at: usize, len: usize, text: &str) {
        if self.edits.as_ref().is_some_and(|e| e.len() < MAX_EDITS) {
            let (start, end) = (self.lsp_position(at), self.lsp_position(at + len));
            self.edits.get_or_insert_default().push(Edit { start, end, text: text.to_string() });
        } else {
            self.edits = None;
        }
        let (start_position, old_end_position) = (self.point(at), self.point(at + len));
        let new_end_position = match text.rfind('\n') {
            Some(last) => Point::new(start_position.row + text.matches('\n').count(), text.len() - last - 1),
            None => Point::new(start_position.row, start_position.column + text.len()),
        };
        let from = self.text.byte_to_char(at);
        self.text.remove(from..self.text.byte_to_char(at + len));
        self.text.insert(from, text);
        self.whole = OnceLock::new();
        self.version += 1;
        let edit = InputEdit { start_byte: at, old_end_byte: at + len, new_end_byte: at + text.len(), start_position, old_end_position, new_end_position };
        let (from, log) = &mut self.log;
        if log.len() >= MAX_LOG {
            *from = log[MAX_LOG / 2].0;
            log.drain(..=MAX_LOG / 2);
        }
        log.push((self.version, edit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_everything_is_one_undo_step() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        let at = Range::point(b.to_string().find("three").unwrap());
        let after = b.replace_all("one\n2\nthree\n", at);
        assert_eq!(b.to_string(), "one\n2\nthree\n");
        assert_eq!(after, Range::point(6), "the cursor stays on its word");
        assert_eq!(b.undo(), Some(at));
        assert_eq!(b.to_string(), "one\ntwo\nthree\n");
        assert_eq!(b.replace_all("one\ntwo\nthree\n", at), at, "the same text: nothing to undo");
        assert_eq!(b.replace_all("é", Range::point(0)), Range::point(0));
        assert_eq!(b.replace_all("è", Range::point(0)), Range::point(0), "never inside a char");
    }

    #[test]
    fn the_line_index_follows_every_edit() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        let check = |b: &Buffer| {
            let fresh = Buffer::new(b.to_string());
            let starts = |b: &Buffer| (0..b.line_count()).map(|line| b.line_start(line)).collect::<Vec<_>>();
            assert_eq!(starts(b), starts(&fresh), "{:?}", b.to_string());
        };
        for (range, text) in [(4..7, "2\n2\n"), (0..0, "\n"), (3..9, ""), (0..0, "x"), (2..2, "a\nb\nc"), (0..1, "")] {
            let range = range.start.min(b.len())..range.end.min(b.len());
            b.edit(range, text);
            check(&b);
        }
        b.commit(Range::point(0));
        while b.undo().is_some() {
            check(&b);
        }
        while b.redo().is_some() {
            check(&b);
        }
    }

    #[test]
    fn edits_are_recorded_as_a_language_server_counts() {
        let mut b = Buffer::new("aé\nb\n");
        b.edit(3..3, "😀");
        b.edit(0..1, "");
        assert_eq!(
            b.take_edits(),
            Some(vec![Edit { start: (0, 2), end: (0, 2), text: "😀".into() }, Edit { start: (0, 0), end: (0, 1), text: String::new() }])
        );
        assert_eq!(b.take_edits(), Some(vec![]));
        b.reset("x");
        assert_eq!(b.take_edits(), None, "everything's new: send it whole");
    }

    #[test]
    fn changes_are_remembered_as_a_syntax_tree_takes_them() {
        let mut b = Buffer::new("ab\ncd\n");
        let v = b.version();
        b.edit(4..4, "x\ny");
        b.edit(0..1, "");
        let changes = b.changes_since(v).unwrap();
        assert_eq!(changes.len(), 2);
        let first = changes[0];
        assert_eq!((first.start_byte, first.old_end_byte, first.new_end_byte), (4, 4, 7));
        assert_eq!((first.start_position, first.new_end_position), (Point::new(1, 1), Point::new(2, 1)));
        assert_eq!(b.changes_since(b.version()), Some(vec![]));
        b.reset("new");
        assert_eq!(b.changes_since(v), None, "replaced: they're no use");
    }

    #[test]
    fn lines() {
        let b = Buffer::new("one\ntwo\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.last_line(), 1);
        assert_eq!(b.line(1), "two");
        assert_eq!(b.line_of(4), 1);
        assert_eq!(b.line_of(3), 0, "the line break belongs to its line");
        assert_eq!(b.lines_range(0..1), 0..4);
        assert_eq!(b.lines_range(1..5), 4..8);
        assert_eq!(b.line_end(2), 8);
    }

    #[test]
    fn transactions_undo_and_redo_as_one() {
        let mut b = Buffer::new("abc");
        b.begin(Range::point(0));
        b.edit(0..1, "X");
        b.edit(3..3, "!");
        b.commit(Range::point(4));
        assert_eq!(b.to_string(), "Xbc!");
        assert_eq!(b.undo(), Some(Range::point(0)));
        assert_eq!(b.to_string(), "abc");
        assert_eq!(b.redo(), Some(Range::point(4)));
        assert_eq!(b.to_string(), "Xbc!");
        assert_eq!(b.undo(), Some(Range::point(0)));
        assert_eq!(b.undo(), None);
    }

    #[test]
    fn the_last_edit_is_where_the_last_change_ended() {
        let mut b = Buffer::new("abc");
        assert_eq!(b.last_edit(), None);
        b.edit(1..1, "xy");
        assert_eq!(b.last_edit(), Some(3));
        b.commit(Range::point(3));
        b.undo();
        assert_eq!(b.last_edit(), None, "undone: what's left is from before");
    }

    #[test]
    fn undo_and_redo_bring_back_the_text_and_selections_after_many_edits() {
        // A seeded walk: transactions of a few edits each, over several chunks of the rope.
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut below = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        };
        let start = "fn main() {\n    println!(\"é😀\");\n}\n".repeat(200);
        let mut b = Buffer::new(&start);
        let mut states = vec![(start.clone(), Range::point(0))];
        for step in 0..60 {
            let before = states.last().unwrap().1;
            b.begin(before);
            for _ in 0..1 + below(4) {
                let at = b.floor_boundary(below(b.len() + 1));
                let to = b.floor_boundary((at + below(30)).min(b.len()));
                b.edit(at..to, ["", "x", "\n", "é", "😀 y\n"][below(5)]);
            }
            let after = Range::new(b.floor_boundary(below(b.len() + 1)), b.floor_boundary(below(b.len() + 1)));
            b.commit(after);
            states.push((b.to_string(), after));
            assert_eq!(b.line_count(), b.to_string().split('\n').count(), "step {step}: the lines are counted");
        }
        for i in (1..states.len()).rev() {
            assert_eq!(b.undo(), Some(states[i - 1].1), "undo to step {i}: the selection before it");
            assert_eq!(b.to_string(), states[i - 1].0);
        }
        assert_eq!(b.undo(), None);
        for state in &states[1..] {
            assert_eq!(b.redo(), Some(state.1), "redo: the selection after it");
            assert_eq!(b.to_string(), state.0);
        }
    }

    #[test]
    fn undo_closes_an_open_transaction_first() {
        let mut b = Buffer::new("abc");
        b.edit(1..2, "");
        assert!(b.has_uncommitted());
        b.undo();
        assert_eq!(b.to_string(), "abc");
    }
}

#[cfg(test)]
mod speed {
    use super::*;

    /// `cargo test -p ghostwrite-core --release speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn how_long_an_edit_takes() {
        for mb in [1, 5, 20, 100] {
            let text = "a line of about forty characters of code\n".repeat(mb * 1024 * 1024 / 41);
            let mut buffer = Buffer::new(text);
            let middle = buffer.len() / 2;
            let start = std::time::Instant::now();
            for i in 0..100 {
                buffer.begin(Range::point(middle));
                buffer.edit(middle + i..middle + i, "x");
                buffer.commit(Range::point(middle));
            }
            println!("{mb} MB: {:?} an edit", start.elapsed() / 100);
        }
    }
}
