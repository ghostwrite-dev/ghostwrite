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
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl Buffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut buffer = Self { line_starts: vec![], text, version: 0, undo: vec![], redo: vec![], open: None };
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
        self.text.replace_range(range.clone(), text);
        let change = Change { at: range.start, removed, inserted: text.to_string() };
        self.open.as_mut().expect("begun above").changes.push(change);
        self.redo.clear();
        self.changed();
    }

    /// Close the open transaction: one undo step, unless it changed nothing.
    pub fn commit(&mut self, selection: Range) {
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

    /// Undo the last transaction; returns the selection from before it.
    pub fn undo(&mut self) -> Option<Range> {
        if let Some(open) = &self.open {
            let after = open.after;
            self.commit(after);
        }
        let tx = self.undo.pop()?;
        for change in tx.changes.iter().rev() {
            self.text.replace_range(change.at..change.at + change.inserted.len(), &change.removed);
        }
        self.changed();
        let before = tx.before;
        self.redo.push(tx);
        Some(before)
    }

    /// Redo the last undone transaction; returns the selection from after it.
    pub fn redo(&mut self) -> Option<Range> {
        let tx = self.redo.pop()?;
        for change in &tx.changes {
            self.text.replace_range(change.at..change.at + change.removed.len(), &change.inserted);
        }
        self.changed();
        let after = tx.after;
        self.undo.push(tx);
        Some(after)
    }

    /// Replace everything (a reload from disk), forgetting the history.
    pub fn reset(&mut self, text: impl Into<String>) {
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
    fn undo_closes_an_open_transaction_first() {
        let mut b = Buffer::new("abc");
        b.edit(1..2, "");
        assert!(b.has_uncommitted());
        b.undo();
        assert_eq!(b.text(), "abc");
    }
}
