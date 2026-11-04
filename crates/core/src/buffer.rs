//! Text with a line index and undo history.
//!
//! Edits are grouped into transactions, each one undo step. A transaction
//! opens with [`Buffer::begin`] (or the first edit) and closes with
//! [`Buffer::commit`]; it remembers the selection on either side so undo and
//! redo put it back.

use crate::selection::Range;

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

#[derive(Debug)]
pub struct Buffer {
    text: String,
    /// Where each line starts; a trailing newline starts an empty last line.
    line_starts: Vec<usize>,
    /// Bumped on every change, for caches (highlights, language servers).
    version: u64,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    open: Option<Transaction>,
    /// Commits wait: several edits, each committing as it goes, are to be one undo.
    held: bool,
    /// The edits since [`Buffer::take_edits`], or `None` if it's simpler to send the whole text.
    edits: Option<Vec<Edit>>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl Buffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut buffer = Self { line_starts: vec![], text, version: 0, undo: vec![], redo: vec![], open: None, held: false, edits: Some(vec![]) };
        buffer.reindex();
        buffer
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn slice(&self, range: std::ops::Range<usize>) -> &str {
        &self.text[range.start.min(self.len())..range.end.min(self.len())]
    }

    pub fn char_at(&self, at: usize) -> Option<char> {
        self.text.get(at..)?.chars().next()
    }

    pub fn next(&self, at: usize) -> usize {
        crate::next_char(&self.text, at.min(self.len()))
    }

    pub fn prev(&self, at: usize) -> usize {
        crate::prev_char(&self.text, at.min(self.len()))
    }

    // Lines.

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The last line holding text: a trailing newline doesn't start one.
    pub fn last_line(&self) -> usize {
        let lines = self.line_count();
        if lines > 1 && self.text.ends_with('\n') { lines - 2 } else { lines - 1 }
    }

    /// The zero-based line `at` is on.
    pub fn line_of(&self, at: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= at).saturating_sub(1)
    }

    /// Start of `line`, clamped to the last line.
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line.min(self.line_count() - 1)]
    }

    /// End of `line`, before its line break.
    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.line_count() - 1);
        self.line_starts.get(line + 1).map_or(self.len(), |next| next - 1)
    }

    /// Char `column` of `line` (as a session keeps the cursor), clamped to the line.
    pub fn at_line_col(&self, line: usize, column: usize) -> usize {
        let start = self.line_start(line);
        self.line(line).char_indices().nth(column).map_or(self.line_end(line), |(i, _)| start + i)
    }

    /// `line` without its line break.
    pub fn line(&self, line: usize) -> &str {
        &self.text[self.line_start(line)..self.line_end(line)]
    }

    /// Bytes of whole `lines`, line breaks included.
    pub fn lines_range(&self, lines: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let at = |line: usize| self.line_starts.get(line).copied().unwrap_or(self.len());
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
        start + self.text[start..end].find(|c| c != ' ' && c != '\t').unwrap_or(end - start)
    }

    pub fn indent_at(&self, at: usize) -> &str {
        &self.text[self.line_start_at(at)..self.first_non_blank_at(at)]
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
        let removed = self.text[range.clone()].to_string();
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
        let old = self.text.as_str();
        let mut prefix = old.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(prefix) || !text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let room = old.len().min(text.len()) - prefix;
        let mut suffix = old.bytes().rev().zip(text.bytes().rev()).take(room).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(old.len() - suffix) || !text.is_char_boundary(text.len() - suffix) {
            suffix -= 1;
        }
        let (end, new_end) = (old.len() - suffix, text.len() - suffix);
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

    /// Where `at` is as a language server counts: its line, and UTF-16 units into it.
    fn lsp_position(&self, at: usize) -> (usize, usize) {
        let line = self.line_of(at);
        (line, self.text[self.line_starts[line]..at].encode_utf16().count())
    }

    /// Replace everything, forgetting the history.
    pub fn reset(&mut self, text: impl Into<String>) {
        self.edits = None;
        self.text = text.into();
        self.undo.clear();
        self.redo.clear();
        self.open = None;
        self.changed();
    }

    fn changed(&mut self) {
        self.version += 1;
        self.reindex();
    }

    /// Replace `len` bytes at `at` with `text`, keeping the line index up
    /// to date by what changed (not by reading the whole text again).
    fn replace(&mut self, at: usize, len: usize, text: &str) {
        if self.edits.as_ref().is_some_and(|e| e.len() < MAX_EDITS) {
            let (start, end) = (self.lsp_position(at), self.lsp_position(at + len));
            self.edits.get_or_insert_default().push(Edit { start, end, text: text.to_string() });
        } else {
            self.edits = None;
        }
        self.text.replace_range(at..at + len, text);
        // Lines starting inside what was replaced go; those after move; the new text's come in.
        let (from, to) = (self.line_starts.partition_point(|&s| s <= at), self.line_starts.partition_point(|&s| s <= at + len));
        let new: Vec<usize> = text.match_indices('\n').map(|(i, _)| at + i + 1).collect();
        let added = new.len();
        self.line_starts.splice(from..to, new);
        let shift = text.len() as isize - len as isize;
        for start in &mut self.line_starts[from + added..] {
            *start = (*start as isize + shift) as usize;
        }
        self.version += 1;
    }

    fn reindex(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        self.line_starts.extend(self.text.match_indices('\n').map(|(i, _)| i + 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_everything_is_one_undo_step() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        let at = Range::point(b.text().find("three").unwrap());
        let after = b.replace_all("one\n2\nthree\n", at);
        assert_eq!(b.text(), "one\n2\nthree\n");
        assert_eq!(after, Range::point(6), "the cursor stays on its word");
        assert_eq!(b.undo(), Some(at));
        assert_eq!(b.text(), "one\ntwo\nthree\n");
        assert_eq!(b.replace_all("one\ntwo\nthree\n", at), at, "the same text: nothing to undo");
        assert_eq!(b.replace_all("é", Range::point(0)), Range::point(0));
        assert_eq!(b.replace_all("è", Range::point(0)), Range::point(0), "never inside a char");
    }

    #[test]
    fn the_line_index_follows_every_edit() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        let check = |b: &Buffer| {
            let fresh = Buffer::new(b.text().to_string());
            assert_eq!(b.line_starts, fresh.line_starts, "{:?}", b.text());
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
        assert_eq!(b.text(), "Xbc!");
        assert_eq!(b.undo(), Some(Range::point(0)));
        assert_eq!(b.text(), "abc");
        assert_eq!(b.redo(), Some(Range::point(4)));
        assert_eq!(b.text(), "Xbc!");
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
    fn undo_closes_an_open_transaction_first() {
        let mut b = Buffer::new("abc");
        b.edit(1..2, "");
        assert!(b.has_uncommitted());
        b.undo();
        assert_eq!(b.text(), "abc");
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
